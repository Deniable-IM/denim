use crate::{
    account::{Account, AuthenticatedDevice, Device},
    database::SignalDatabase,
    error::ApiError,
    managers::state::SignalServerState,
};
use axum::extract::ws::Message;
use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    Error,
};
use axum_extra::{
    headers::{authorization::Basic, Authorization},
    typed_header::TypedHeaderRejectionReason,
    TypedHeader,
};
use common::websocket::wsstream::WSStream;
use libsignal_core::ServiceId;
use rand::{rngs::OsRng, RngCore};
use std::fmt::Debug;

const SALT_SIZE: usize = 16;
const AUTH_TOKEN_HKDF_INFO: &[u8] = "authtoken".as_bytes();

#[async_trait]
impl<T, U> FromRequestParts<SignalServerState<T, U>> for AuthenticatedDevice
where
    T: SignalDatabase,
    U: WSStream<Message, Error> + Debug,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &SignalServerState<T, U>,
    ) -> Result<Self, Self::Rejection> {
        let TypedHeader(Authorization(basic)) =
            TypedHeader::<Authorization<Basic>>::from_request_parts(parts, state)
                .await
                .map_err(|err| match err.reason() {
                    TypedHeaderRejectionReason::Missing => ApiError {
                        status_code: StatusCode::UNAUTHORIZED,
                        body: "Authorization header is missing".to_owned(),
                    },
                    _ => ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        body: "Error parsing Authorization header".to_owned(),
                    },
                })?;

        if let Some(v) = basic.username().find('.') {
            let service_id: ServiceId = ServiceId::parse_from_service_id_string(
                &basic.username()[..v],
            )
            .ok_or(ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                body: "Error parsing service id".to_owned(),
            })?;
            let device_id = basic.username()[(v + 1)..]
                .parse::<u32>()
                .map_err(|_| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Error parsing device id".to_owned(),
                })?;
            authenticate_device(state, &service_id, device_id, basic.password()).await
        } else {
            let service_id: ServiceId = ServiceId::parse_from_service_id_string(basic.username())
                .ok_or(ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                body: "Error parsing service id".to_owned(),
            })?;
            authenticate_device(state, &service_id, 1, basic.password()).await
        }
    }
}

async fn authenticate_device<T: SignalDatabase, U: WSStream<Message, axum::Error> + Debug>(
    state: &SignalServerState<T, U>,
    service_id: &ServiceId,
    device_id: u32,
    password: &str,
) -> Result<AuthenticatedDevice, ApiError> {
    let account: Account = state
        .account_manager
        .get_account(service_id)
        .await
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: "Error getting account".to_owned(),
        })?;

    let device: Device = account
        .devices()
        .iter()
        .find(|&d| d.device_id() == device_id.into())
        .ok_or(ApiError {
            status_code: StatusCode::NOT_FOUND,
            body: "Device not found".to_owned(),
        })?
        .to_owned();

    let salted_token = SaltedTokenHash {
        hash: device.auth_token().to_owned(),
        salt: device.salt().to_owned(),
    };

    if salted_token.verify(password)? {
        Ok(AuthenticatedDevice::new(account, device))
    } else {
        Err(ApiError {
            status_code: StatusCode::UNAUTHORIZED,
            body: "Wrong password".to_owned(),
        })
    }
}

pub struct SaltedTokenHash {
    hash: String,
    salt: String,
}
impl SaltedTokenHash {
    pub fn generate_for(credentials: &str) -> Result<Self, ApiError> {
        fn generate_salt() -> String {
            let mut salt = [0u8; SALT_SIZE];
            OsRng.fill_bytes(&mut salt);
            hex::encode(salt)
        }

        let salt = generate_salt();
        let token = SaltedTokenHash::calculate(&salt, credentials)?;

        Ok(Self { salt, hash: token })
    }

    pub fn verify(&self, credentials: &str) -> Result<bool, ApiError> {
        let their_value = SaltedTokenHash::calculate(&self.salt, credentials)?;
        Ok(self.hash == their_value)
    }

    fn calculate(salt: &str, token: &str) -> Result<String, ApiError> {
        Ok(hex::encode(HKDF_DeriveSecrets(
            32,
            token.as_bytes(),
            Some(AUTH_TOKEN_HKDF_INFO),
            Some(salt.as_bytes()),
        )?))
    }

    pub fn hash(&self) -> String {
        self.hash.clone()
    }
    pub fn salt(&self) -> String {
        self.salt.clone()
    }
}

// Function taken from libsignal-bridge
#[allow(nonstandard_style)]
fn HKDF_DeriveSecrets(
    output_length: u32,
    ikm: &[u8],
    label: Option<&[u8]>,
    salt: Option<&[u8]>,
) -> Result<Vec<u8>, ApiError> {
    let label = label.unwrap_or(&[]);
    let mut buffer = vec![0; output_length as usize];
    hkdf::Hkdf::<sha2::Sha256>::new(salt, ikm)
        .expand(label, &mut buffer)
        .map_err(|_| ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            body: format!("output too long ({})", output_length),
        })?;
    Ok(buffer)
}
