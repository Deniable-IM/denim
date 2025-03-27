use std::sync::Arc;

use super::database::{
    ClientDB, DeviceIdentityKeyStore, DeviceKyberPreKeyStore, DevicePreKeyStore,
    DeviceSenderKeyStore, DeviceSessionStore, DeviceSignedPreKeyStore,
};
use async_std::sync::Mutex;
use axum::async_trait;
use libsignal_core::ProtocolAddress;
use libsignal_protocol::{
    Direction, IdentityKey, IdentityKeyPair, IdentityKeyStore, KyberPreKeyId, KyberPreKeyRecord,
    KyberPreKeyStore, PreKeyId, PreKeyRecord, PreKeyStore, ProtocolStore as SignalProtocolStore,
    SenderKeyRecord, SenderKeyStore, SessionRecord, SessionStore, SignalProtocolError,
    SignedPreKeyId, SignedPreKeyRecord, SignedPreKeyStore,
};
use uuid::Uuid;

pub struct Storage<T: ClientDB> {
    pub device: Arc<Mutex<T>>,
    pub protocol_store: ProtocolStore<T>,
}

impl<T: ClientDB> Storage<T> {
    pub fn new(db: Arc<Mutex<T>>, protocol_store: ProtocolStore<T>) -> Self {
        Self {
            device: db,
            protocol_store,
        }
    }
}

pub struct ProtocolStore<T: ClientDB> {
    pub identity_key_store: DeviceIdentityKeyStore<T>,
    pub pre_key_store: DevicePreKeyStore<T>,
    pub signed_pre_key_store: DeviceSignedPreKeyStore<T>,
    pub kyber_pre_key_store: DeviceKyberPreKeyStore<T>,
    pub session_store: DeviceSessionStore<T>,
    pub sender_key_store: DeviceSenderKeyStore<T>,
}

impl<T: ClientDB> ProtocolStore<T> {
    pub fn new(device: Arc<Mutex<T>>) -> Self {
        Self {
            identity_key_store: DeviceIdentityKeyStore::new(device.clone()),
            pre_key_store: DevicePreKeyStore::new(device.clone()),
            signed_pre_key_store: DeviceSignedPreKeyStore::new(device.clone()),
            kyber_pre_key_store: DeviceKyberPreKeyStore::new(device.clone()),
            session_store: DeviceSessionStore::new(device.clone()),
            sender_key_store: DeviceSenderKeyStore::new(device.clone()),
        }
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> IdentityKeyStore for ProtocolStore<T> {
    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair, SignalProtocolError> {
        self.identity_key_store.get_identity_key_pair().await
    }

    async fn get_local_registration_id(&self) -> Result<u32, SignalProtocolError> {
        self.identity_key_store.get_local_registration_id().await
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, SignalProtocolError> {
        self.identity_key_store
            .save_identity(address, identity)
            .await
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        direction: Direction,
    ) -> Result<bool, SignalProtocolError> {
        self.identity_key_store
            .is_trusted_identity(address, identity, direction)
            .await
    }

    async fn get_identity(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<IdentityKey>, SignalProtocolError> {
        self.identity_key_store.get_identity(address).await
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> PreKeyStore for ProtocolStore<T> {
    async fn get_pre_key(&self, id: PreKeyId) -> Result<PreKeyRecord, SignalProtocolError> {
        self.pre_key_store.get_pre_key(id).await
    }

    async fn save_pre_key(
        &mut self,
        id: PreKeyId,
        record: &PreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.pre_key_store.save_pre_key(id, record).await
    }

    async fn remove_pre_key(&mut self, id: PreKeyId) -> Result<(), SignalProtocolError> {
        self.pre_key_store.remove_pre_key(id).await
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> SignedPreKeyStore for ProtocolStore<T> {
    async fn get_signed_pre_key(
        &self,
        id: SignedPreKeyId,
    ) -> Result<SignedPreKeyRecord, SignalProtocolError> {
        self.signed_pre_key_store.get_signed_pre_key(id).await
    }

    async fn save_signed_pre_key(
        &mut self,
        id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.signed_pre_key_store
            .save_signed_pre_key(id, record)
            .await
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> KyberPreKeyStore for ProtocolStore<T> {
    async fn get_kyber_pre_key(
        &self,
        kyber_prekey_id: KyberPreKeyId,
    ) -> Result<KyberPreKeyRecord, SignalProtocolError> {
        self.kyber_pre_key_store
            .get_kyber_pre_key(kyber_prekey_id)
            .await
    }

    async fn save_kyber_pre_key(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.kyber_pre_key_store
            .save_kyber_pre_key(kyber_prekey_id, record)
            .await
    }

    async fn mark_kyber_pre_key_used(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
    ) -> Result<(), SignalProtocolError> {
        self.kyber_pre_key_store
            .mark_kyber_pre_key_used(kyber_prekey_id)
            .await
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> SessionStore for ProtocolStore<T> {
    async fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, SignalProtocolError> {
        self.session_store.load_session(address).await
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), SignalProtocolError> {
        self.session_store.store_session(address, record).await
    }
}

#[async_trait(?Send)]
impl<T: ClientDB> SenderKeyStore for ProtocolStore<T> {
    async fn store_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
        record: &SenderKeyRecord,
    ) -> Result<(), SignalProtocolError> {
        self.sender_key_store
            .store_sender_key(sender, distribution_id, record)
            .await
    }

    async fn load_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
    ) -> Result<Option<SenderKeyRecord>, SignalProtocolError> {
        self.sender_key_store
            .load_sender_key(sender, distribution_id)
            .await
    }
}

impl<T: ClientDB> SignalProtocolStore for ProtocolStore<T> {}
