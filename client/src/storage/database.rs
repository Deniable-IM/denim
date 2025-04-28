use std::sync::Arc;

use async_std::sync::Mutex;
use axum::async_trait;
use common::{deniable::DeniableSendingBuffer, web_api::DenimChunk};
use libsignal_core::{Aci, Pni, ProtocolAddress, ServiceId};
use libsignal_protocol::{
    Direction, IdentityKey, IdentityKeyPair, IdentityKeyStore, KyberPreKeyId, KyberPreKeyRecord,
    KyberPreKeyStore, PreKeyId, PreKeyRecord, PreKeyStore, SenderKeyRecord, SenderKeyStore,
    SessionRecord, SessionStore, SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord,
    SignedPreKeyStore,
};
use uuid::Uuid;

use crate::contact_manager::{Contact, ContactName};

#[allow(dead_code)]
#[async_trait(?Send)]
pub trait ClientDB {
    type Error: std::error::Error + 'static;

    async fn insert_account_information(
        &self,
        aci: Aci,
        pni: Pni,
        password: String,
    ) -> Result<(), Self::Error>;
    async fn insert_account_key_information(
        &self,
        key_pair: IdentityKeyPair,
        registration_id: u32,
    ) -> Result<(), Self::Error>;
    async fn get_key_ids(&self) -> Result<(u32, u32, u32), Self::Error>;
    async fn store_contact(&self, contact: &Contact) -> Result<(), Self::Error>;
    async fn load_contacts(&self) -> Result<Vec<Contact>, Self::Error>;
    async fn remove_contact(&self, service_id: &ServiceId) -> Result<(), Self::Error>;
    async fn get_all_nicknames(&self) -> Result<Vec<ContactName>, Self::Error>;
    async fn insert_service_id_for_nickname(
        &self,
        nickname: &str,
        service_id: &ServiceId,
    ) -> Result<(), Self::Error>;
    async fn get_service_id_by_nickname(&self, nickname: &str) -> Result<ServiceId, Self::Error>;
    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair, Self::Error>;
    async fn get_local_registration_id(&self) -> Result<u32, Self::Error>;
    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, Self::Error>;
    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        direction: Direction,
    ) -> Result<bool, Self::Error>;
    async fn get_identity(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<IdentityKey>, Self::Error>;
    async fn get_pre_key(&self, prekey_id: PreKeyId) -> Result<PreKeyRecord, Self::Error>;
    async fn save_pre_key(
        &mut self,
        prekey_id: PreKeyId,
        record: &PreKeyRecord,
    ) -> Result<(), Self::Error>;
    async fn remove_pre_key(&mut self, prekey_id: PreKeyId) -> Result<(), Self::Error>;
    async fn get_signed_pre_key(
        &self,
        id: SignedPreKeyId,
    ) -> Result<SignedPreKeyRecord, Self::Error>;
    async fn save_signed_pre_key(
        &mut self,
        id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<(), Self::Error>;
    async fn get_kyber_pre_key(
        &self,
        kyber_prekey_id: KyberPreKeyId,
    ) -> Result<KyberPreKeyRecord, Self::Error>;
    async fn save_kyber_pre_key(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<(), Self::Error>;
    async fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, Self::Error>;
    async fn load_deniable_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, Self::Error>;
    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), Self::Error>;
    async fn store_deniable_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), Self::Error>;
    async fn store_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
        record: &SenderKeyRecord,
    ) -> Result<(), Self::Error>;
    async fn load_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
    ) -> Result<Option<SenderKeyRecord>, Self::Error>;
    async fn set_password(&mut self, new_password: String) -> Result<(), Self::Error>;
    async fn get_password(&self) -> Result<String, Self::Error>;
    async fn set_aci(&mut self, new_aci: Aci) -> Result<(), Self::Error>;
    async fn get_aci(&self) -> Result<Aci, Self::Error>;
    async fn set_pni(&mut self, new_pni: Pni) -> Result<(), Self::Error>;
    async fn get_pni(&self) -> Result<Pni, Self::Error>;
    async fn get_deniable_payload(&self) -> Result<(u32, Vec<u8>), Self::Error>;
    async fn get_deniable_payload_by_id(&self, payload_id: u32) -> Result<Vec<u8>, Self::Error>;
    async fn store_deniable_payload(
        &self,
        payload_id: Option<u32>,
        payload: Vec<u8>,
    ) -> Result<(), Self::Error>;
    async fn remove_deniable_payload(&self, payload_id: u32) -> Result<(), Self::Error>;
    async fn try_get_key_request_sent(
        &self,
        service_id: String,
    ) -> Result<Option<String>, Self::Error>;
    async fn store_key_request_sent(
        &self,
        service_id: String,
        alias: String,
    ) -> Result<(), Self::Error>;
    async fn remove_key_request_sent(&self, service_id: String) -> Result<(), Self::Error>;
    async fn get_messages_awaiting_encryption(
        &self,
        alias: String,
    ) -> Result<Vec<String>, Self::Error>;
    async fn store_message_awaiting_encryption(
        &self,
        message: String,
        alias: String,
    ) -> Result<(), Self::Error>;
    async fn remove_message_awaiting_encryption(&self, message_id: u32) -> Result<(), Self::Error>;
    async fn get_and_remove_incoming_deniable_chunks(
        &mut self,
    ) -> Result<Vec<DenimChunk>, Self::Error>;
    async fn store_incoming_deniable_chunks(
        &mut self,
        chunks: Vec<DenimChunk>,
    ) -> Result<(), Self::Error>;
}

pub struct DeviceIdentityKeyStore<T: ClientDB> {
    db: Arc<Mutex<T>>,
}

impl<T: ClientDB> DeviceIdentityKeyStore<T> {
    pub fn new(db: Arc<Mutex<T>>) -> Self {
        Self { db }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> IdentityKeyStore for DeviceIdentityKeyStore<T> {
    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair, SignalProtocolError> {
        self.db
            .lock()
            .await
            .get_identity_key_pair()
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn get_local_registration_id(&self) -> Result<u32, SignalProtocolError> {
        self.db
            .lock()
            .await
            .get_local_registration_id()
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, SignalProtocolError> {
        self.db
            .lock()
            .await
            .save_identity(address, identity)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        direction: Direction,
    ) -> Result<bool, SignalProtocolError> {
        self.db
            .lock()
            .await
            .is_trusted_identity(address, identity, direction)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn get_identity(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<IdentityKey>, SignalProtocolError> {
        self.db
            .lock()
            .await
            .get_identity(address)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }
}

pub struct DevicePreKeyStore<T: ClientDB> {
    db: Arc<Mutex<T>>,
}

impl<T: ClientDB> DevicePreKeyStore<T> {
    pub fn new(db: Arc<Mutex<T>>) -> Self {
        Self { db }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> PreKeyStore for DevicePreKeyStore<T> {
    async fn get_pre_key(&self, prekey_id: PreKeyId) -> Result<PreKeyRecord, SignalProtocolError> {
        self.db
            .lock()
            .await
            .get_pre_key(prekey_id)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn save_pre_key(
        &mut self,
        prekey_id: PreKeyId,
        record: &PreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .save_pre_key(prekey_id, record)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn remove_pre_key(&mut self, prekey_id: PreKeyId) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .remove_pre_key(prekey_id)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }
}

pub struct DeviceSignedPreKeyStore<T: ClientDB> {
    db: Arc<Mutex<T>>,
}

impl<T: ClientDB> DeviceSignedPreKeyStore<T> {
    pub fn new(db: Arc<Mutex<T>>) -> Self {
        Self { db }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> SignedPreKeyStore for DeviceSignedPreKeyStore<T> {
    async fn get_signed_pre_key(
        &self,
        id: SignedPreKeyId,
    ) -> Result<SignedPreKeyRecord, SignalProtocolError> {
        self.db
            .lock()
            .await
            .get_signed_pre_key(id)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn save_signed_pre_key(
        &mut self,
        id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .save_signed_pre_key(id, record)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }
}

pub struct DeviceKyberPreKeyStore<T: ClientDB> {
    db: Arc<Mutex<T>>,
}

impl<T: ClientDB> DeviceKyberPreKeyStore<T> {
    pub fn new(db: Arc<Mutex<T>>) -> Self {
        Self { db }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> KyberPreKeyStore for DeviceKyberPreKeyStore<T> {
    async fn get_kyber_pre_key(
        &self,
        kyber_prekey_id: KyberPreKeyId,
    ) -> Result<KyberPreKeyRecord, SignalProtocolError> {
        self.db
            .lock()
            .await
            .get_kyber_pre_key(kyber_prekey_id)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn save_kyber_pre_key(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .save_kyber_pre_key(kyber_prekey_id, record)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn mark_kyber_pre_key_used(
        &mut self,
        _kyber_prekey_id: KyberPreKeyId,
    ) -> Result<(), SignalProtocolError> {
        Ok(())
    }
}

pub struct DeviceSessionStore<T: ClientDB> {
    db: Arc<Mutex<T>>,
}

impl<T: ClientDB> DeviceSessionStore<T> {
    pub fn new(db: Arc<Mutex<T>>) -> Self {
        Self { db }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> SessionStore for DeviceSessionStore<T> {
    async fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, SignalProtocolError> {
        self.db
            .lock()
            .await
            .load_session(address)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .store_session(address, record)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }
}

pub struct DeviceSenderKeyStore<T: ClientDB> {
    db: Arc<Mutex<T>>,
}

impl<T: ClientDB> DeviceSenderKeyStore<T> {
    pub fn new(db: Arc<Mutex<T>>) -> Self {
        Self { db }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> SenderKeyStore for DeviceSenderKeyStore<T> {
    async fn store_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
        record: &SenderKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .store_sender_key(sender, distribution_id, record)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn load_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
    ) -> Result<Option<SenderKeyRecord>, SignalProtocolError> {
        self.db
            .lock()
            .await
            .load_sender_key(sender, distribution_id)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }
}

pub struct DeniableStore<T: ClientDB> {
    db: Arc<Mutex<T>>,
}

impl<T: ClientDB> DeniableStore<T> {
    pub fn new(db: Arc<Mutex<T>>) -> Self {
        Self { db }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> SessionStore for DeniableStore<T> {
    async fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, SignalProtocolError> {
        self.db
            .lock()
            .await
            .load_deniable_session(address)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .store_deniable_session(address, record)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> DeniableSendingBuffer for DeniableStore<T> {
    async fn get_outgoing_message(&mut self) -> Result<(u32, Vec<u8>), SignalProtocolError> {
        self.db
            .lock()
            .await
            .get_deniable_payload()
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn set_outgoing_message(
        &mut self,
        message_id: Option<u32>,
        outgoing_message: Vec<u8>,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .store_deniable_payload(message_id, outgoing_message)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }

    async fn remove_outgoing_message(
        &mut self,
        message_id: u32,
    ) -> Result<(), SignalProtocolError> {
        self.db
            .lock()
            .await
            .remove_deniable_payload(message_id)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))
    }
}
