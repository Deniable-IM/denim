use crate::{
    account::{Account, AuthenticatedDevice, Device}, account_authenticator::SaltedTokenHash, availability_listener::AvailabilityListener, database::SignalDatabase, envelope::ToEnvelope, error::ApiError, managers::{
        message_persister::MessagePersister,
        state::SignalServerState,
        websocket::{
            connection::{UserIdentity, WebSocketConnection},
            signal_websocket::SignalWebSocket,
        },
    }, persister::Persister, postgres::PostgresDatabase, query::CheckKeysRequest, response::{LinkDeviceResponse, LinkDeviceToken, SendMessageResponse}, validators::{
        destination_device_validator::DestinationDeviceValidator,
        pre_key_signature_validator::PreKeySignatureValidator,
    }
};
use anyhow::Result;
use axum::{
    debug_handler,
    extract::{
        connect_info::ConnectInfo,
        ws::{Message, WebSocketUpgrade},
        Host, Path, Query, Request, State,
    },
    handler::HandlerWithoutStateExt,
    http::{
        header::{ACCEPT, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE, ORIGIN},
        HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri,
    },
    middleware::{from_fn, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{any, delete, get, post, put},
    BoxError, Json, Router,
};
use axum_extra::{headers, TypedHeader};
use axum_server::tls_rustls::RustlsConfig;
use base64::prelude::{Engine as _, BASE64_URL_SAFE, BASE64_URL_SAFE_NO_PAD};
use common::web_api::{
    authorization::BasicAuthorizationHeader, DenimMessages, DeviceCapabilityType,
    DevicePreKeyBundle, LinkDeviceRequest, PreKeyCount, PreKeyResponse, RegistrationRequest,
    RegistrationResponse, RegularPayload, SetKeyRequest, SignalMessage,
};
use common::websocket::wsstream::WSStream;
use futures_util::StreamExt;
use headers::authorization::Basic;
use headers::Authorization;
use hmac::{Hmac, Mac};
use libsignal_core::{ProtocolAddress, ServiceId, ServiceIdKind};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::{
    env,
    fmt::Debug,
    net::SocketAddr,
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower_http::compression::CompressionLayer;
use tower_http::cors::CorsLayer;

pub async fn handle_put_messages<T: SignalDatabase, U: WSStream<Message, axum::Error> + Debug>(
    state: &SignalServerState<T, U>,
    authenticated_device: &AuthenticatedDevice,
    destination_identifier: &ServiceId,
    payload: DenimMessages,
) -> Result<SendMessageResponse, ApiError> {
    if *destination_identifier == authenticated_device.account().pni() {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            body: "".to_owned(),
        });
    }

    let is_sync_message = *destination_identifier == authenticated_device.account().aci();
    let destination: Account = if is_sync_message {
        authenticated_device.account().clone()
    } else {
        state
            .account_manager
            .get_account(destination_identifier)
            .await
            .map_err(|_| ApiError {
                status_code: StatusCode::NOT_FOUND,
                body: "Destination account not found".to_owned(),
            })?
    };
    let exclude_device_ids: Vec<u32> = if is_sync_message {
        vec![authenticated_device.device().device_id().into()]
    } else {
        Vec::new()
    };

    let regular_messages: Vec<SignalMessage> = payload
        .messages
        .into_iter()
        .filter_map(|msg| match msg.regular_payload {
            RegularPayload::SignalMessage(signal_msg) => Some(signal_msg),
            _ => None,
        })
        .collect();

    let message_device_ids: Vec<u32> = regular_messages
        .iter()
        .map(|message| message.destination_device_id)
        .collect();
    DestinationDeviceValidator::validate_complete_device_list(
        &destination,
        &message_device_ids,
        &exclude_device_ids,
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::CONFLICT,
        body: serde_json::to_string(&err).expect("Can serialize device ids"),
    })?;

    DestinationDeviceValidator::validate_registration_id_from_messages(
        &destination,
        &regular_messages,
        destination_identifier.kind() == ServiceIdKind::Pni,
    )
    .map_err(|err| ApiError {
        status_code: StatusCode::GONE,
        body: serde_json::to_string(&err).expect("Can serialize device ids"),
    })?;

    for message in regular_messages {
        let mut envelope = message.to_envelope(
            destination_identifier,
            authenticated_device.account(),
            u32::from(authenticated_device.device().device_id()) as u8,
            payload.timestamp,
            false,
        );
        let address = ProtocolAddress::new(
            destination.aci().service_id_string(),
            message.destination_device_id.into(),
        );
        state
            .message_manager
            .insert(&address, &mut envelope)
            .await
            .map_err(|_| ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                body: "Could not insert message".to_owned(),
            })?;
    }

    //EXTRACT DENIABLE CHUNKS HERE AND PROCESS

    let needs_sync = !is_sync_message && authenticated_device.account().devices().len() > 1;
    Ok(SendMessageResponse { needs_sync })
}

pub async fn handle_keepalive<T: SignalDatabase, U: WSStream<Message, axum::Error> + Debug>(
    state: &SignalServerState<T, U>,
    authenticated_device: &AuthenticatedDevice,
) -> Result<(), ApiError> {
    //Check if present in presencemanager. If not present, close connection for device. Else return 200 Ok
    if !state
        .client_presence_manager
        .is_locally_present(&authenticated_device.get_protocol_address(ServiceIdKind::Aci))
    {
        if let Some(connection) = state
            .websocket_manager
            .get(&authenticated_device.get_protocol_address(ServiceIdKind::Aci))
            .await
        {
            let _ = connection
                .lock()
                .await
                .close_reason(1000, "OK")
                .await
                .map_err(|err| err.to_string());
        }
    }

    Ok(())
}

async fn handle_post_registration<T: SignalDatabase, U: WSStream<Message, axum::Error> + Debug>(
    state: SignalServerState<T, U>,
    auth_header: BasicAuthorizationHeader,
    registration: RegistrationRequest,
) -> Result<RegistrationResponse, ApiError> {
    let time_now = time_now()?;
    let phone_number = auth_header.username();
    let hash = SaltedTokenHash::generate_for(auth_header.password())?;
    let device = Device::builder()
        .device_id(1.into())
        .name(registration.account_attributes().name.clone())
        .last_seen(time_now)
        .created(time_now)
        .auth_token(hash.hash())
        .salt(hash.salt())
        .registration_id(registration.account_attributes().registration_id)
        .pni_registration_id(registration.account_attributes().pni_registration_id)
        .capabilities(registration.account_attributes().capabilities.clone())
        .build();

    let device_pre_key_bundle = DevicePreKeyBundle {
        aci_signed_pre_key: registration.aci_signed_pre_key().to_owned(),
        pni_signed_pre_key: registration.pni_signed_pre_key().to_owned(),
        aci_pq_pre_key: registration.aci_pq_last_resort_pre_key().to_owned(),
        pni_pq_pre_key: registration.pni_pq_last_resort_pre_key().to_owned(),
    };

    let account = state
        .account_manager
        .create_account(
            phone_number.to_owned(),
            registration.aci_identity_key().to_owned(),
            registration.pni_identity_key().to_owned(),
            device.clone(),
        )
        .await?;

    let aci = account.aci();
    let address = ProtocolAddress::new(aci.service_id_string(), device.device_id());

    // Store key bundle for new account
    state
        .account_manager
        .store_key_bundle(&device_pre_key_bundle, &address)
        .await
        .map_err(|err| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: err.to_string(),
        })?;

    Ok(RegistrationResponse {
        uuid: aci.into(),
        pni: account.pni().into(),
        number: phone_number.to_owned(),
        username_hash: None,
        storage_capable: true,
    })
}

async fn handle_get_link_device_token<
    T: SignalDatabase,
    U: WSStream<Message, axum::Error> + Debug,
>(
    _state: SignalServerState<T, U>,
    authenticated_device: AuthenticatedDevice,
) -> Result<LinkDeviceToken, ApiError> {
    if authenticated_device.device().device_id() != 1.into() {
        return Err(ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            body: "".to_owned(),
        });
    }

    let claims = format!(
        "{}.{}",
        authenticated_device.account().aci().service_id_string(),
        time_now()?
    );

    let link_device_secret =
        std::env::var("LINK_DEVICE_SECRET").expect("Unable to read LINK_DEVICE_SECRET .env var");
    let mut mac = Hmac::<Sha256>::new_from_slice(link_device_secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(claims.as_bytes());
    let signature = mac.finalize().into_bytes();
    let link_device_token = format!("{}:{}", claims, BASE64_URL_SAFE.encode(signature));

    let mut hasher = Sha256::new();
    hasher.update(link_device_token.as_bytes());
    let digest = hasher.finalize();
    let token_identifier = BASE64_URL_SAFE_NO_PAD.encode(digest);

    Ok(LinkDeviceToken {
        verification_code: link_device_token,
        token_identifier,
    })
}

async fn handle_post_link_device<T: SignalDatabase, U: WSStream<Message, axum::Error> + Debug>(
    state: SignalServerState<T, U>,
    auth_header: Basic,
    link_device_request: LinkDeviceRequest,
) -> Result<LinkDeviceResponse, ApiError> {
    let (claims, b64_signature) = link_device_request
        .verification_code
        .split_once(':')
        .ok_or(ApiError {
            status_code: StatusCode::FORBIDDEN,
            body: "".to_owned(),
        })?;

    let link_device_secret =
        std::env::var("LINK_DEVICE_SECRET").expect("Unable to read LINK_DEVICE_SECRET .env var");
    let mut mac = Hmac::<Sha256>::new_from_slice(link_device_secret.as_bytes())
        .expect("HMAC can take key of any size");
    mac.update(claims.as_bytes());
    let expected_signature = mac.finalize().into_bytes();
    let signature = BASE64_URL_SAFE
        .decode(b64_signature)
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "".to_owned(),
        })?;
    if expected_signature.as_slice() != signature {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            body: "".to_owned(),
        });
    }

    let (aci_str, timestamp_str) = claims.split_once('.').ok_or(ApiError {
        status_code: StatusCode::FORBIDDEN,
        body: "".to_owned(),
    })?;
    let aci = ServiceId::parse_from_service_id_string(aci_str).ok_or(ApiError {
        status_code: StatusCode::FORBIDDEN,
        body: "".to_owned(),
    })?;
    let timestamp = timestamp_str.parse().map_err(|_| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        body: "".to_owned(),
    })?;
    let time_then = Duration::from_millis(timestamp);
    let time_now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards");
    let elapsed_time = time_now - time_then;
    if elapsed_time.as_secs() > 600 {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            body: "".to_owned(),
        });
    }

    let account = state
        .account_manager
        .get_account(&aci)
        .await
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "".to_owned(),
        })?;

    let account_attributes = link_device_request.account_attributes;
    let device_activation_request = link_device_request.device_activation_request;

    let all_keys_valid = PreKeySignatureValidator::validate_pre_key_signatures(
        &account.aci_identity_key(),
        &[
            device_activation_request.aci_signed_pre_key,
            device_activation_request.aci_pq_last_resort_pre_key,
        ],
    ) && PreKeySignatureValidator::validate_pre_key_signatures(
        &account.pni_identity_key(),
        &[
            device_activation_request.pni_signed_pre_key,
            device_activation_request.pni_pq_last_resort_pre_key,
        ],
    );

    if !all_keys_valid {
        return Err(ApiError {
            status_code: StatusCode::UNPROCESSABLE_ENTITY,
            body: "".to_owned(),
        });
    }

    if !DeviceCapabilityType::VALUES
        .iter()
        .filter(|capability| {
            capability.value().prevent_downgrade && account.has_capability(capability)
        })
        .all(|required_capability| {
            account_attributes
                .capabilities
                .contains(required_capability)
        })
    {
        return Err(ApiError {
            status_code: StatusCode::CONFLICT,
            body: "".to_owned(),
        });
    }

    state
        .db
        .add_used_device_link_token(link_device_request.verification_code)
        .await
        .map_err(|_| ApiError {
            status_code: StatusCode::FORBIDDEN,
            body: "".to_owned(),
        })?;

    let new_device_id = account.get_next_device_id();
    let hash = SaltedTokenHash::generate_for(auth_header.password())?;
    let device = Device::builder()
        .device_id(new_device_id.into())
        .name(account_attributes.name)
        .last_seen(time_now.as_millis())
        .created(time_now.as_millis())
        .auth_token(hash.hash())
        .salt(hash.salt())
        .registration_id(account_attributes.registration_id)
        .pni_registration_id(account_attributes.pni_registration_id)
        .capabilities(account_attributes.capabilities)
        .build();
    state
        .account_manager
        .add_device(&aci, &device)
        .await
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "".to_owned(),
        })?;

    Ok(LinkDeviceResponse {
        aci: account.aci().service_id_string(),
        pni: account.pni().service_id_string(),
        device_id: new_device_id,
    })
}

async fn handle_delete_account<T: SignalDatabase, U: WSStream<Message, axum::Error> + Debug>(
    state: SignalServerState<T, U>,
    authenticated_device: AuthenticatedDevice,
) -> Result<(), ApiError> {
    state
        .account_manager
        .delete_account(&authenticated_device.account().aci().into())
        .await
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "".to_owned(),
        })
}

async fn handle_delete_device<T: SignalDatabase, U: WSStream<Message, axum::Error> + Debug>(
    state: SignalServerState<T, U>,
    device_id: u32,
    authenticated_device: AuthenticatedDevice,
) -> Result<(), ApiError> {
    if authenticated_device.device().device_id() != 1.into()
        && authenticated_device.device().device_id() != device_id.into()
    {
        return Err(ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            body: "".to_owned(),
        });
    }

    if device_id == 1 {
        return Err(ApiError {
            status_code: StatusCode::FORBIDDEN,
            body: "".to_owned(),
        });
    }

    state
        .account_manager
        .delete_device(&ProtocolAddress::new(
            authenticated_device.account().aci().service_id_string(),
            device_id.into(),
        ))
        .await
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "".to_owned(),
        })
}

// redirect from http to https. this is temporary
async fn redirect_http_to_https(addr: SocketAddr, http: u16, https: u16) -> Result<(), BoxError> {
    fn make_https(host: String, uri: Uri, http: u16, https: u16) -> Result<Uri, BoxError> {
        let mut parts = uri.into_parts();

        parts.scheme = Some(axum::http::uri::Scheme::HTTPS);

        if parts.path_and_query.is_none() {
            parts.path_and_query = Some("/".parse()?);
        }

        let https_host = host.replace(&http.to_string(), &https.to_string());
        parts.authority = Some(https_host.parse()?);

        Ok(Uri::from_parts(parts)?)
    }

    let redirect = move |Host(host): Host, uri: Uri| async move {
        match make_https(host, uri, http, https) {
            Ok(uri) => Ok(Redirect::permanent(&uri.to_string())),
            Err(_) => Err(StatusCode::BAD_REQUEST),
        }
    };

    let listener = tokio::net::TcpListener::bind(addr).await?;

    axum::serve(listener, redirect.into_make_service()).await?;
    Ok(())
}

fn parse_service_id(string: String) -> Result<ServiceId, ApiError> {
    ServiceId::parse_from_service_id_string(&string).ok_or_else(|| ApiError {
        status_code: StatusCode::BAD_REQUEST,
        body: "Could not parse service id".to_owned(),
    })
}

/// Handler for the GET v1/identifier/{phone_number} endpoint.
#[debug_handler]
async fn get_identifier_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    _authenticated_device: AuthenticatedDevice,
    Path(phone_number): Path<String>,
) -> Result<String, ApiError> {
    Ok(state
        .account_manager
        .get_account_from_phonenumber_without_devices(&phone_number)
        .await
        .map_err(|err| ApiError {
            status_code: StatusCode::BAD_REQUEST,
            body: format!("Could not get ACI: {}", err),
        })?
        .aci()
        .service_id_string())
}

/// Handler for the PUT v1/messages/{address} endpoint.
#[debug_handler]
async fn put_messages_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
    Path(destination_identifier): Path<String>,
    Json(payload): Json<DenimMessages>,
) -> Result<SendMessageResponse, ApiError> {
    let destination_identifier = parse_service_id(destination_identifier)?;
    handle_put_messages(
        &state,
        &authenticated_device,
        &destination_identifier,
        payload,
    )
    .await
}

/// Handler for the POST v1/registration endpoint.
#[debug_handler]
async fn post_registration_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    headers: HeaderMap,
    Json(registration): Json<RegistrationRequest>,
) -> Result<Json<RegistrationResponse>, ApiError> {
    let auth_header = headers
        .get("Authorization")
        .ok_or_else(|| ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            body: "Missing authorization header".to_owned(),
        })?
        .to_str()
        .map_err(|err| ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            body: format!(
                "Authorization header could not be parsed as string: {}",
                err
            ),
        })?
        .parse()
        .map_err(|err| ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            body: format!("Authorization header could not be parsed: {}", err),
        })?;

    handle_post_registration(state, auth_header, registration)
        .await
        .map(Json)
}

/// Handler for the GET /v2/keys/:identifier/:device_id endpoint.
#[debug_handler]
async fn get_keys_id_device_id(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
    Path((identifier, device_id)): Path<(String, String)>,
) -> Result<Json<PreKeyResponse>, ApiError> {
    state
        .key_manager
        .handle_get_keys_id_device_id(
            &state.db,
            &authenticated_device,
            ServiceId::parse_from_service_id_string(&identifier).ok_or_else(|| ApiError {
                status_code: StatusCode::BAD_REQUEST,
                body: "Identifier is not of right format".into(),
            })?,
            device_id,
        )
        .await
        .map(Json)
}

/// Handler for the GET /v2/keys endpoint.
#[debug_handler]
async fn get_keys(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
) -> Result<Json<PreKeyCount>, ApiError> {
    let (count, pq_count) = state
        .key_manager
        .get_one_time_pre_key_count(&authenticated_device.account().aci().into())
        .await
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "".to_string(),
        })?;
    Ok(axum::Json(PreKeyCount { count, pq_count }))
}

/// Handler for the POST v2/keys/check endpoint.
#[debug_handler]
async fn post_keycheck_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
    Json(check_keys_request): Json<CheckKeysRequest>,
) -> Result<(), ApiError> {
    state
        .key_manager
        .handle_post_keycheck(
            &authenticated_device,
            get_kind(check_keys_request.identity_type)?,
            check_keys_request.user_digest,
        )
        .await?
        .then_some(())
        .ok_or_else(|| ApiError {
            status_code: StatusCode::CONFLICT,
            body: "".into(),
        })
}

/// Handler for the PUT v2/keys endpoint.
#[debug_handler]
async fn put_keys_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
    Query(params): Query<HashMap<String, String>>,
    Json(set_keys_request): Json<SetKeyRequest>,
) -> Result<(), ApiError> {
    state
        .key_manager
        .handle_put_keys(
            &authenticated_device,
            set_keys_request,
            get_kind(params.get("identity").unwrap().to_owned())?,
        )
        .await
}

/// Handler for the DELETE v1/accounts/me endpoint.
#[debug_handler]
async fn delete_account_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
) -> Result<(), ApiError> {
    handle_delete_account(state, authenticated_device).await
}

/// Handler for the DELETE v1/devices/{device_id} endpoint.
#[debug_handler]
async fn delete_device_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    Path(device_id): Path<u32>,
    authenticated_device: AuthenticatedDevice,
) -> Result<(), ApiError> {
    handle_delete_device(state, device_id, authenticated_device).await
}

/// Handler for the GET v1/devices/provisioning/code endpoint.
#[debug_handler]
async fn get_link_device_token(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
) -> Result<LinkDeviceToken, ApiError> {
    handle_get_link_device_token(state, authenticated_device).await
}

/// Handler for the POST v1/devices/link endpoint.
#[debug_handler]
async fn post_link_device_endpoint(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    TypedHeader(Authorization(basic)): TypedHeader<Authorization<Basic>>,
    Json(link_device_request): Json<LinkDeviceRequest>,
) -> Result<LinkDeviceResponse, ApiError> {
    handle_post_link_device(state, basic, link_device_request).await
}

/// Websocket upgrade handler '/v1/websocket'
#[debug_handler]
async fn create_websocket_endpoint(
    State(mut state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
    ws: WebSocketUpgrade,
    user_agent: Option<TypedHeader<headers::UserAgent>>,
    ConnectInfo(socket_addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let user_agent = match user_agent {
        Some(TypedHeader(user_agent)) => user_agent.to_string(),
        None => "Unknown browser".to_string(),
    };

    println!("`{user_agent}` at {socket_addr} connected.");

    ws.on_upgrade(move |socket| {
        let mut websocket_manager = state.websocket_manager.clone();
        async move {
            let signal_websocket = SignalWebSocket::new(socket);
            let (sender, receiver) = signal_websocket.split();

            // Create websocket connection
            let websocket = WebSocketConnection::new(
                UserIdentity::AuthenticatedDevice(authenticated_device.into()),
                socket_addr,
                sender,
                state.clone(),
            );

            let address = websocket.protocol_address();

            // Listen for new messages
            websocket_manager.listen(websocket, receiver).await;

            // Check if webSocket upgrade was successful
            let Some(websocket_manager) = websocket_manager.get(&address).await else {
                println!("ws.on_upgrade: WebSocket does not exist in WebSocketManager");
                return;
            };

            // Send all persisted message to new connected device
            websocket_manager
                .lock()
                .await
                .send_persisted()
                .await;

            state
                .message_manager
                .add_message_availability_listener(&address, websocket_manager.clone())
                .await;

            let _ = state
                .client_presence_manager
                .set_present(&address, websocket_manager)
                .await;
        }
    })
}

#[debug_handler]
pub async fn get_keepalive(
    State(state): State<SignalServerState<PostgresDatabase, SignalWebSocket>>,
    authenticated_device: AuthenticatedDevice,
) -> impl IntoResponse {
    handle_keepalive(&state, &authenticated_device).await
}

async fn signal_time_middleware(req: Request, next: Next) -> Response {
    let mut response = next.run(req).await;

    response.headers_mut().insert(
        "x-signal-timestamp",
        HeaderValue::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Time went backwards")
                .as_millis() as u64,
        ),
    );

    response
}

/// To add a new endpoint:
///  * create an async router function: `<method>_<endpoint_name>_endpoint`.
///  * create an async handler function: `handle_<method>_<endpoint_name>`
///  * add the router function to the axum router below.
///  * call the handler function from the router function to handle the request.
pub async fn start_server(use_tls: bool) -> Result<(), Box<dyn std::error::Error>> {
    if use_tls {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
    }

    let cors = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .max_age(Duration::from_secs(5184000))
        .allow_credentials(true)
        .allow_headers([
            AUTHORIZATION,
            CONTENT_TYPE,
            CONTENT_LENGTH,
            ACCEPT,
            ORIGIN,
            HeaderName::from_static("x-requested-with"),
            HeaderName::from_static("x-signal-agent"),
        ]);

    let state = SignalServerState::<PostgresDatabase, SignalWebSocket>::new().await;

    // let message_persister = MessagePersister::start(
    //     state.message_manager.clone(),
    //     state.message_cache.clone(),
    //     state.db.clone(),
    //     state.account_manager.clone(),
    // );

    let message_persister = MessagePersister::<
        PostgresDatabase,
        WebSocketConnection<SignalWebSocket, PostgresDatabase>,
    >::listen(
        state.db.clone(),
        vec![
            Box::new(state.message_manager.clone()),
            Box::new(state.message_cache.clone()),
            Box::new(state.account_manager.clone()),
        ],
    );

    let app = Router::new()
        .route("/", get(|| async { "Hello from Signal Server" }))
        .route("/v1/identifier/:phone_number", get(get_identifier_endpoint))
        .route("/v1/messages/:destination", put(put_messages_endpoint))
        .route("/v1/registration", post(post_registration_endpoint))
        .route(
            "/v2/keys/:identifier/:device_id",
            get(get_keys_id_device_id),
        )
        .route("/v2/keys", get(get_keys))
        .route("/v2/keys/check", post(post_keycheck_endpoint))
        .route("/v2/keys", put(put_keys_endpoint))
        .route("/v1/accounts/me", delete(delete_account_endpoint))
        .route("/v1/devices/provisioning/code", get(get_link_device_token))
        .route("/v1/devices/link", post(post_link_device_endpoint))
        .route("/v1/devices/:device_id", delete(delete_device_endpoint))
        .route("/v1/websocket", any(create_websocket_endpoint))
        .route("/v1/keepalive", get(get_keepalive))
        .with_state(state)
        .layer(CompressionLayer::new().gzip(true))
        .layer(cors)
        .layer(from_fn(signal_time_middleware));

    let address = env::var("SERVER_ADDRESS")?;
    let https_port = env::var("HTTPS_PORT")?;
    let http_port = env::var("HTTP_PORT")?;

    let http_addr = SocketAddr::from_str(format!("{}:{}", address, http_port).as_str())?;
    let https_addr = SocketAddr::from_str(format!("{}:{}", address, https_port).as_str())?;

    // we should probably sometime in future a proxy or something to redirect instead

    if use_tls {
        tokio::spawn(redirect_http_to_https(
            http_addr,
            http_port.parse()?,
            https_port.parse()?,
        ));
        let config = RustlsConfig::from_pem_file("cert/server.crt", "cert/server.key").await?;
        axum_server::bind_rustls(https_addr, config)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    } else {
        axum_server::bind(http_addr)
            .serve(app.into_make_service_with_connect_info::<SocketAddr>())
            .await?;
    }

    message_persister.stop().await;

    Ok(())
}

fn time_now() -> Result<u128, ApiError> {
    Ok(SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "".into(),
        })?
        .as_millis())
}

fn get_kind(identity_string: String) -> Result<ServiceIdKind, ApiError> {
    match identity_string.as_str() {
        "aci" | "ACI" | "" => Ok(ServiceIdKind::Aci),
        "pni" | "PNI" => Ok(ServiceIdKind::Pni),
        _ => {
             Err(ApiError {
                status_code: StatusCode::BAD_REQUEST,
                body: "Identity type needs to be either of: aci | pni | ACI | PNI or none which will default to aci".into(),
            })
        }
    }
}

#[cfg(test)]
mod server_tests {
    #[ignore = "Not implemented"]
    #[tokio::test]
    async fn handle_register_account_registers_account() {
        todo!()
    }

    #[ignore = "Not implemented"]
    #[tokio::test]
    async fn handle_get_keys_gets_keys() {
        todo!();
    }

    #[ignore = "Not implemented"]
    #[tokio::test]
    async fn handle_post_keycheck_test() {
        todo!()
    }

    #[ignore = "Not implemented"]
    #[tokio::test]
    async fn handle_delete_account_deletes_account() {
        todo!()
    }

    #[ignore = "Not implemented"]
    #[tokio::test]
    async fn handle_post_link_device_registers_new_device() {
        todo!()
    }

    #[ignore = "Not implemented"]
    #[tokio::test]
    async fn handle_delete_device_deletes_device() {
        todo!()
    }
}
