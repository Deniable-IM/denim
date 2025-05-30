use crate::{
    contact_manager::ContactManager,
    encryption::{encrypt, pad_message},
    errors::{
        DatabaseError, ProcessPreKeyBundleError, ReceiveMessageError, Result, SignalClientError,
    },
    key_manager::KeyManager,
    server::{SignalServer, SignalServerAPI},
    storage::{
        database::ClientDB,
        device::Device,
        generic::{ProtocolStore, Storage},
    },
};
use async_std::sync::Mutex;
use axum::http::StatusCode;
use base64::{prelude::BASE64_STANDARD, Engine as _};
use bincode::{deserialize, serialize};
use common::{
    deniable::chunk::Chunker,
    envelope::ProcessedEnvelope,
    signalservice::{
        data_message::{contact::Name, Contact},
        envelope, Content, DataMessage, Envelope,
    },
    web_api::{
        AccountAttributes, DeniablePayload, DenimChunk, DenimMessage, DenimMessages, PreKeyRequest,
        RegistrationRequest, RegularPayload, SignalMessage,
    },
};
use core::str;
use include_dir::{include_dir, Dir};
use libsignal_core::{Aci, DeviceId, Pni, ProtocolAddress, ServiceId};
use libsignal_protocol::{
    process_prekey_bundle, CiphertextMessage, IdentityKeyPair, IdentityKeyStore, PreKeyBundle,
    SessionStore,
};
use prost::Message;
use rand::{rngs::OsRng, Rng};
use rusqlite::Connection;
use rusqlite_migration::Migrations;
use std::time::{SystemTime, UNIX_EPOCH};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock},
};

pub struct Client<T: ClientDB, U: SignalServerAPI> {
    pub alias: String,
    pub aci: Aci,
    #[allow(dead_code)]
    pub pni: Pni,
    contact_manager: ContactManager,
    server_api: U,
    #[allow(dead_code)]
    key_manager: KeyManager,
    pub storage: Storage<T>,
    pub chunker: Chunker,
}

const PROFILE_KEY_LENGTH: usize = 32;
const MASTER_KEY_LENGTH: usize = 32;
const PASSWORD_LENGTH: usize = 16;

static MIGRATIONS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/client_db/migrations");
static MIGRATIONS: LazyLock<Migrations<'static>> =
    LazyLock::new(|| Migrations::from_directory(&MIGRATIONS_DIR).unwrap());

impl<T: ClientDB, U: SignalServerAPI> Client<T, U> {
    fn new(
        alias: String,
        aci: Aci,
        pni: Pni,
        contact_manager: ContactManager,
        server_api: U,
        key_manager: KeyManager,
        storage: Storage<T>,
        chunker: Chunker,
    ) -> Self {
        Client {
            alias,
            aci,
            pni,
            contact_manager,
            server_api,
            key_manager,
            storage,
            chunker,
        }
    }

    async fn connect_to_db(database_url: &str) -> Result<Connection> {
        let mut conn = Connection::open(database_url).expect("Could not open database");

        MIGRATIONS
            .to_latest(&mut conn)
            .expect("Could not run migrations");

        Ok(conn)
    }

    /// Register a new account with the server.
    /// `phone_number` must be unique.
    pub async fn register(
        name: &str,
        phone_number: String,
        database_url: &str,
        server_url: &str,
        cert_path: &Option<String>,
        alias: String,
    ) -> Result<Client<Device, SignalServer>> {
        let mut csprng = OsRng;
        let aci_registration_id = OsRng.gen_range(1..16383);
        let pni_registration_id = OsRng.gen_range(1..16383);
        let aci_id_key_pair = IdentityKeyPair::generate(&mut csprng);
        let pni_id_key_pair = IdentityKeyPair::generate(&mut csprng);
        let conn = Client::<T, U>::connect_to_db(database_url).await?;
        let device = Arc::new(Mutex::new(Device::new(conn)));
        device
            .lock()
            .await
            .insert_account_key_information(aci_id_key_pair, aci_registration_id)
            .await
            .unwrap();

        let mut proto_storage = ProtocolStore::new(device.clone());
        let mut key_manager = KeyManager::default();

        let aci_signed_pk = key_manager
            .generate_signed_pre_key(
                &mut proto_storage.identity_key_store,
                &mut proto_storage.signed_pre_key_store,
                &mut csprng,
            )
            .await?;

        let pni_signed_pk = key_manager
            .generate_signed_pre_key(
                &mut proto_storage.identity_key_store,
                &mut proto_storage.signed_pre_key_store,
                &mut csprng,
            )
            .await?;

        let aci_pq_last_resort = key_manager
            .generate_kyber_pre_key(
                &mut proto_storage.identity_key_store,
                &mut proto_storage.kyber_pre_key_store,
            )
            .await?;

        let pni_pq_last_resort = key_manager
            .generate_kyber_pre_key(
                &mut proto_storage.identity_key_store,
                &mut proto_storage.kyber_pre_key_store,
            )
            .await?;

        let mut password = [0u8; PASSWORD_LENGTH];
        csprng.fill(&mut password);
        let password = BASE64_STANDARD.encode(password);
        let password = password[0..password.len() - 2].to_owned();

        let mut profile_key = [0u8; PROFILE_KEY_LENGTH];
        csprng.fill(&mut profile_key);

        let access_key = [0u8; 16]; // This should be derived from profile_key

        let mut master_key = [0u8; MASTER_KEY_LENGTH];
        csprng.fill(&mut master_key);

        let account_attributes = AccountAttributes::new(
            name.into(),
            true,
            aci_registration_id,
            pni_registration_id,
            Vec::new(),
            Box::new(access_key),
        );
        let mut server_api = SignalServer::new(cert_path, server_url);

        let req = RegistrationRequest::new(
            "".into(),
            "".into(),
            account_attributes,
            true, // Require atomic is always true
            true, // Skip device transfer is always true
            *aci_id_key_pair.identity_key(),
            *pni_id_key_pair.identity_key(),
            aci_signed_pk.into(),
            pni_signed_pk.into(),
            aci_pq_last_resort.into(),
            pni_pq_last_resort.into(),
            None,
            None,
        );

        let response = server_api
            .register_client(phone_number, password.clone(), req, None)
            .await?;

        let aci: Aci = response.uuid.into();
        let pni: Pni = response.pni.into();

        server_api.create_auth_header(aci, password.clone(), 1.into());

        let contact_manager = ContactManager::new();
        device
            .lock()
            .await
            .insert_account_information(aci, pni, password.clone())
            .await
            .map_err(DatabaseError::from)?;
        let mut storage = Storage::new(device.clone(), proto_storage);
        let key_bundle = key_manager
            .generate_key_bundle(&mut storage.protocol_store)
            .await?;

        server_api.publish_pre_key_bundle(key_bundle).await?;

        // println!("Connecting to {}...", server_url);
        let q_value = server_api
            .connect(&aci.service_id_string(), &password, server_url, cert_path)
            .await?
            .unwrap();
        // println!("Connected");

        Ok(Client::new(
            alias,
            aci,
            pni,
            contact_manager,
            server_api,
            key_manager,
            storage,
            Chunker::new(q_value),
        ))
    }

    pub async fn login(
        database_url: &str,
        cert_path: &Option<String>,
        server_url: &str,
        alias: String,
    ) -> Result<Client<Device, SignalServer>> {
        let conn = Client::<T, U>::connect_to_db(database_url).await?;
        let device = Arc::new(Mutex::new(Device::new(conn)));
        let contacts = device
            .lock()
            .await
            .load_contacts()
            .await
            .map(|contacts| {
                let mut c = HashMap::new();
                for contact in contacts {
                    c.insert(contact.service_id, contact);
                }
                c
            })
            .map_err(DatabaseError::from)?;
        let (one_time, signed, kyber) = device
            .lock()
            .await
            .get_key_ids()
            .await
            .map_err(DatabaseError::from)?;

        let password = device
            .lock()
            .await
            .get_password()
            .await
            .map_err(DatabaseError::from)?;
        let aci = device
            .lock()
            .await
            .get_aci()
            .await
            .map_err(DatabaseError::from)?;

        let mut server_api = SignalServer::new(cert_path, server_url);

        let q_value = server_api
            .connect(&aci.service_id_string(), &password, server_url, cert_path)
            .await?;

        server_api.create_auth_header(aci, password.clone(), 1.into());

        let aci = device
            .lock()
            .await
            .get_aci()
            .await
            .map_err(DatabaseError::from)?;
        let pni = device
            .lock()
            .await
            .get_pni()
            .await
            .map_err(DatabaseError::from)?;
        Ok(Client::new(
            alias,
            aci,
            pni,
            ContactManager::new_with_contacts(contacts),
            server_api,
            KeyManager::new(signed + 1, kyber + 1, one_time + 1), // Adds 1 to prevent reusing key ids
            Storage::new(device.clone(), ProtocolStore::new(device.clone())),
            Chunker::new(q_value.unwrap()),
        ))
    }

    pub async fn get_service_id_from_server(&mut self, phone_number: &str) -> Result<ServiceId> {
        self.server_api
            .get_service_id_from_server(phone_number)
            .await
    }

    pub async fn disconnect(&mut self) {
        self.server_api.disconnect().await;
    }

    pub async fn send_message(&mut self, message: &str, alias: &str) -> Result<()> {
        let service_id = self
            .storage
            .device
            .lock()
            .await
            .get_service_id_by_nickname(alias)
            .await
            .map_err(DatabaseError::from)?;

        let content = Content::builder()
            .data_message(
                DataMessage::builder()
                    .body(message.to_owned())
                    .contact(vec![Contact {
                        name: Some(Name {
                            given_name: None,
                            family_name: None,
                            prefix: None,
                            suffix: None,
                            middle_name: None,
                            display_name: Some(self.alias.to_owned()),
                        }),
                        number: vec![],
                        email: vec![],
                        address: vec![],
                        avatar: None,
                        organization: None,
                    }])
                    .body_ranges(vec![])
                    .preview(vec![])
                    .attachments(vec![])
                    .build(),
            )
            .build();

        let timestamp = SystemTime::now();

        let msgs = encrypt(
            &mut self.storage.protocol_store.identity_key_store,
            &mut self.storage.protocol_store.session_store,
            self.contact_manager.get_contact(&service_id)?,
            pad_message(content.encode_to_vec().as_ref()).as_ref(),
            timestamp,
        )
        .await?;

        let mut denim_messages = Vec::new();
        for (id, msg) in msgs {
            let regular_payload = RegularPayload::SignalMessage(SignalMessage {
                r#type: match msg.1 {
                    CiphertextMessage::SignalMessage(_) => envelope::Type::Ciphertext.into(),
                    CiphertextMessage::SenderKeyMessage(_) => envelope::Type::KeyExchange.into(),
                    CiphertextMessage::PreKeySignalMessage(_) => {
                        envelope::Type::PrekeyBundle.into()
                    }
                    CiphertextMessage::PlaintextContent(_) => {
                        envelope::Type::PlaintextContent.into()
                    }
                },
                destination_device_id: id.into(),
                destination_registration_id: msg.0,
                content: BASE64_STANDARD.encode(msg.1.serialize()),
                ..Default::default()
            });
            let regular_payload_size = serialize(&regular_payload)
                .expect("Should serialize payload")
                .len() as f32;
            let chunks = self
                .chunker
                .create_chunks(
                    regular_payload_size,
                    &mut self.storage.protocol_store.deniable_store,
                )
                .await
                .expect("Should create chunks");

            denim_messages.push(DenimMessage {
                regular_payload,
                chunks: chunks.0,
                counter: None,
                q: None,
                ballast: vec![0; chunks.1],
            });
        }

        let msgs = DenimMessages {
            messages: denim_messages,
            online: true,
            urgent: false,
            timestamp: timestamp
                .duration_since(UNIX_EPOCH)
                .expect("can get the time since epoch")
                .as_secs(),
        };

        match self.server_api.send_msg(&msgs, &service_id).await {
            Ok(_) => Ok(()),
            Err(_) => {
                let device_ids = self.get_new_device_ids(&service_id).await?;
                self.update_contact(alias, device_ids).await?;
                self.server_api.send_msg(&msgs, &service_id).await
            }
        }
    }

    pub async fn send_deniable_message(&mut self, message: &str, alias: &str) -> Result<()> {
        let service_id = self
            .storage
            .device
            .lock()
            .await
            .get_service_id_by_nickname(alias)
            .await
            .map_err(DatabaseError::from)?;

        let content = Content::builder()
            .data_message(
                DataMessage::builder()
                    .body(message.to_owned())
                    .contact(vec![Contact {
                        name: Some(Name {
                            given_name: None,
                            family_name: None,
                            prefix: None,
                            suffix: None,
                            middle_name: None,
                            display_name: Some(self.alias.to_owned()),
                        }),
                        number: vec![],
                        email: vec![],
                        address: vec![],
                        avatar: None,
                        organization: None,
                    }])
                    .body_ranges(vec![])
                    .preview(vec![])
                    .attachments(vec![])
                    .build(),
            )
            .build();

        let timestamp = SystemTime::now();

        let msgs = encrypt(
            &mut self.storage.protocol_store.deniable_identity_key_store,
            &mut self.storage.protocol_store.deniable_store,
            self.contact_manager.get_contact(&service_id)?,
            pad_message(content.encode_to_vec().as_ref()).as_ref(),
            timestamp,
        )
        .await?;

        for (id, msg) in msgs {
            let deniable_payload = DeniablePayload::SignalMessage(SignalMessage {
                r#type: match msg.1 {
                    CiphertextMessage::SignalMessage(_) => envelope::Type::Ciphertext.into(),
                    CiphertextMessage::SenderKeyMessage(_) => envelope::Type::KeyExchange.into(),
                    CiphertextMessage::PreKeySignalMessage(_) => {
                        envelope::Type::PrekeyBundle.into()
                    }
                    CiphertextMessage::PlaintextContent(_) => {
                        envelope::Type::PlaintextContent.into()
                    }
                },
                destination_device_id: id.into(),
                destination_service_id: Some(service_id.service_id_string()),
                destination_registration_id: msg.0,
                content: BASE64_STANDARD.encode(msg.1.serialize()),
            });
            let deniable_payload_serialized =
                serialize(&deniable_payload).expect("Should serialize payload");

            self.storage
                .device
                .lock()
                .await
                .store_deniable_payload(None, 0, deniable_payload_serialized)
                .await
                .map_err(DatabaseError::from)?;
        }
        Ok(())
    }

    pub async fn has_message(&mut self) -> bool {
        self.server_api.has_message().await
    }

    pub async fn receive_message(&mut self) -> Result<Vec<ProcessedEnvelope>> {
        // I get Envelope from Server.
        let request = self
            .server_api
            .get_message()
            .await
            .ok_or(ReceiveMessageError::NoMessageReceived)?;
        let denim_msg: DenimMessage = deserialize(request.body()).unwrap();
        self.chunker.set_q_value(
            denim_msg
                .q
                .expect("q value should always be populated by server"),
        );
        let envelope = match denim_msg.regular_payload {
            RegularPayload::Envelope(e) => e,
            _ => {
                self.server_api
                    .send_response(request, StatusCode::INTERNAL_SERVER_ERROR)
                    .await?;

                return Err(ReceiveMessageError::EnvelopeDecodeError)?;
            }
        };
        let mut processed = vec![
            Envelope::decrypt(
                envelope,
                &mut self.storage.protocol_store.session_store,
                &mut self.storage.protocol_store.identity_key_store,
                &mut self.storage.protocol_store.pre_key_store,
                &mut self.storage.protocol_store.signed_pre_key_store,
                &mut self.storage.protocol_store.kyber_pre_key_store,
                &mut OsRng,
            )
            .await?,
        ];

        let _ = self.server_api.send_response(request, StatusCode::OK).await;

        let chunks: Vec<DenimChunk> = denim_msg
            .chunks
            .into_iter()
            .filter(|chunk| !chunk.is_dummy())
            .collect();
        if !chunks.is_empty() {
            let deniable_payloads = self.handle_incoming_chunks(chunks).await?;
            for deniable_payload in deniable_payloads {
                match deniable_payload {
                    DeniablePayload::Envelope(envelope) => {
                        processed.push(
                            Envelope::decrypt(
                                envelope,
                                &mut self.storage.protocol_store.deniable_store,
                                &mut self.storage.protocol_store.deniable_identity_key_store,
                                &mut self.storage.protocol_store.pre_key_store,
                                &mut self.storage.protocol_store.signed_pre_key_store,
                                &mut self.storage.protocol_store.kyber_pre_key_store,
                                &mut OsRng,
                            )
                            .await?,
                        );
                    }
                    DeniablePayload::KeyResponse(pre_key_response) => {
                        let service_id =
                            ServiceId::parse_from_service_id_string(pre_key_response.service_id())
                                .expect("Should be service id");
                        let bundles: Vec<PreKeyBundle> =
                            Vec::<PreKeyBundle>::try_from(pre_key_response)
                                .map_err(|err| SignalClientError::KeyError(err.to_string()))?;
                        let device_ids = self
                            .initialize_sessions_from_bundle(&service_id, &bundles, true)
                            .await?;
                        let alias = self
                            .storage
                            .device
                            .lock()
                            .await
                            .try_get_key_request_sent(service_id.service_id_string())
                            .await
                            .map_err(DatabaseError::from)?
                            .expect("Should contain key request");
                        self.add_contact(&alias, &service_id, Some(device_ids))
                            .await?;
                        self.storage
                            .device
                            .lock()
                            .await
                            .remove_key_request_sent(service_id.service_id_string())
                            .await
                            .map_err(DatabaseError::from)?;
                        let messages = self
                            .storage
                            .device
                            .lock()
                            .await
                            .get_messages_awaiting_encryption(alias.to_owned())
                            .await
                            .map_err(DatabaseError::from)?;
                        for message in messages {
                            self.send_deniable_message(&message, &alias).await?;
                        }
                    }
                    _ => todo!(),
                }
            }
        }

        // The final message is stored within a DataMessage inside a Content.
        Ok(processed)
    }

    pub async fn handle_incoming_chunks(
        &mut self,
        new_chunks: Vec<DenimChunk>,
    ) -> Result<Vec<DeniablePayload>> {
        let mut chunks = if new_chunks.iter().any(|chunk| chunk.is_final()) {
            self.storage
                .device
                .lock()
                .await
                .get_and_remove_incoming_deniable_chunks()
                .await
                .map_err(DatabaseError::from)?
        } else {
            Vec::new()
        };

        let mut deniable_payloads = Vec::new();
        for chunk in new_chunks {
            if chunk.is_final() {
                chunks.sort();
                chunks.push(chunk.clone());
                let deniable_bytes: Vec<u8> = chunks.into_iter().flat_map(|c| c.chunk).collect();
                let deniable_payload =
                    deserialize(&deniable_bytes).expect("Should be deniable payload");
                deniable_payloads.push(deniable_payload);

                chunks = Vec::new();
            } else {
                chunks.push(chunk.clone());
            }
        }

        if !chunks.is_empty() {
            self.storage
                .device
                .lock()
                .await
                .store_incoming_deniable_chunks(chunks)
                .await
                .map_err(DatabaseError::from)?;
        }

        Ok(deniable_payloads)
    }

    pub async fn add_contact(
        &mut self,
        alias: &str,
        service_id: &ServiceId,
        device_ids: Option<Vec<DeviceId>>,
    ) -> Result<()> {
        if self.contact_manager.get_contact(&service_id).is_ok() {
            return Ok(());
        }
        self.contact_manager
            .add_contact(&service_id)
            .map_err(SignalClientError::ContactManagerError)?;

        let contact = self
            .contact_manager
            .get_contact(&service_id)
            .map_err(SignalClientError::ContactManagerError)?;

        self.storage
            .device
            .lock()
            .await
            .store_contact(contact)
            .await
            .map_err(DatabaseError::from)?;

        self.storage
            .device
            .lock()
            .await
            .insert_service_id_for_nickname(alias, &service_id)
            .await
            .map_err(|err| {
                SignalClientError::DatabaseError(DatabaseError::Custom(Box::new(err)))
            })?;

        let new_device_ids = if let Some(new_device_ids) = device_ids {
            new_device_ids
        } else {
            self.get_new_device_ids(&service_id).await?
        };
        self.update_contact(alias, new_device_ids).await
    }

    pub async fn add_deniable_contact_and_queue_message(
        &mut self,
        service_id: &ServiceId,
        text: &str,
        alias: &str,
    ) -> Result<()> {
        if self
            .storage
            .device
            .lock()
            .await
            .try_get_key_request_sent(service_id.service_id_string())
            .await
            .map_err(DatabaseError::from)?
            .is_none()
        {
            let deniable_keyrequest_payload = DeniablePayload::KeyRequest(PreKeyRequest {
                service_id: service_id.service_id_string(),
            });
            let deniable_payload_serialized =
                serialize(&deniable_keyrequest_payload).expect("Should serialize payload");

            self.storage
                .device
                .lock()
                .await
                .store_deniable_payload(None, 0, deniable_payload_serialized)
                .await
                .map_err(DatabaseError::from)?;
            self.storage
                .device
                .lock()
                .await
                .store_key_request_sent(service_id.service_id_string(), alias.to_owned())
                .await
                .map_err(DatabaseError::from)?;
        }

        self.storage
            .device
            .lock()
            .await
            .store_message_awaiting_encryption(text.to_owned(), alias.to_owned())
            .await
            .map_err(DatabaseError::from)?;
        Ok(())
    }

    #[allow(dead_code)]
    pub async fn remove_contact(&mut self, alias: &str) -> Result<()> {
        let service_id = self
            .storage
            .device
            .lock()
            .await
            .get_service_id_by_nickname(alias)
            .await
            .map_err(DatabaseError::from)?;

        self.contact_manager
            .remove_contact(&service_id)
            .map_err(SignalClientError::ContactManagerError)?;

        self.storage
            .device
            .lock()
            .await
            .remove_contact(&service_id)
            .await
            .map_err(|err| DatabaseError::Custom(Box::new(err)).into())
    }

    async fn update_contact(&mut self, alias: &str, device_ids: Vec<DeviceId>) -> Result<()> {
        let service_id = self
            .storage
            .device
            .lock()
            .await
            .get_service_id_by_nickname(alias)
            .await
            .map_err(DatabaseError::from)?;

        self.contact_manager
            .update_contact(&service_id, device_ids)
            .map_err(SignalClientError::ContactManagerError)?;

        let contact = self
            .contact_manager
            .get_contact(&service_id)
            .map_err(SignalClientError::ContactManagerError)?;

        self.storage
            .device
            .lock()
            .await
            .store_contact(contact)
            .await
            .map_err(|err| DatabaseError::Custom(Box::new(err)).into())
    }

    async fn get_new_device_ids(&mut self, service_id: &ServiceId) -> Result<Vec<DeviceId>> {
        let bundles = self.server_api.fetch_pre_key_bundles(service_id).await?;

        self.initialize_sessions_from_bundle(service_id, &bundles, false)
            .await
    }

    async fn initialize_sessions_from_bundle(
        &mut self,
        service_id: &ServiceId,
        bundles: &[PreKeyBundle],
        deniable: bool,
    ) -> Result<Vec<DeviceId>> {
        let mut device_ids = Vec::new();
        let time = SystemTime::now();
        let session_store: &mut dyn SessionStore = if deniable {
            &mut self.storage.protocol_store.deniable_store
        } else {
            &mut self.storage.protocol_store.session_store
        };
        let identity_key_store: &mut dyn IdentityKeyStore = if deniable {
            &mut self.storage.protocol_store.deniable_identity_key_store
        } else {
            &mut self.storage.protocol_store.identity_key_store
        };
        for ref bundle in bundles {
            let device_id = bundle.device_id().expect("Device id should be safe");
            device_ids.push(device_id);
            process_prekey_bundle(
                &ProtocolAddress::new(service_id.service_id_string(), device_id),
                session_store,
                identity_key_store,
                bundle,
                time,
                &mut OsRng,
            )
            .await
            .map_err(ProcessPreKeyBundleError)?;
        }

        Ok(device_ids)
    }
}
