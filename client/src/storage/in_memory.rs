use super::database::ClientDB;
use crate::contact_manager::{Contact, ContactName};
use axum::async_trait;
use libsignal_core::{Aci, Pni, ProtocolAddress, ServiceId};
use libsignal_protocol::{
    Direction, IdentityKey, IdentityKeyPair, IdentityKeyStore, InMemIdentityKeyStore,
    InMemKyberPreKeyStore, InMemPreKeyStore, InMemSenderKeyStore, InMemSessionStore,
    InMemSignedPreKeyStore, KyberPreKeyId, KyberPreKeyRecord, KyberPreKeyStore, PreKeyId,
    PreKeyRecord, PreKeyStore, SenderKeyRecord, SenderKeyStore, SessionRecord, SessionStore,
    SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord, SignedPreKeyStore,
};
use uuid::Uuid;

#[derive(Clone)]
pub struct InMemory {
    password: String,
    aci: Aci,
    pni: Pni,
    identity_key_store: InMemIdentityKeyStore,
    pre_key_store: InMemPreKeyStore,
    signed_pre_key_store: InMemSignedPreKeyStore,
    kyber_pre_key_store: InMemKyberPreKeyStore,
    session_store: InMemSessionStore,
    sender_key_store: InMemSenderKeyStore,
}

#[allow(dead_code)]
impl InMemory {
    pub fn new(
        password: String,
        aci: Aci,
        pni: Pni,
        key_pair: IdentityKeyPair,
        registration_id: u32,
    ) -> Self {
        Self {
            password,
            aci,
            pni,
            identity_key_store: InMemIdentityKeyStore::new(key_pair, registration_id),
            pre_key_store: InMemPreKeyStore::new(),
            signed_pre_key_store: InMemSignedPreKeyStore::new(),
            kyber_pre_key_store: InMemKyberPreKeyStore::new(),
            session_store: InMemSessionStore::new(),
            sender_key_store: InMemSenderKeyStore::new(),
        }
    }
}

#[async_trait(?Send)]
impl ClientDB for InMemory {
    type Error = SignalProtocolError;

    async fn insert_account_information(
        &self,
        _aci: Aci,
        _pni: Pni,
        _password: String,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    async fn insert_account_key_information(
        &self,
        _key_pair: IdentityKeyPair,
        _registration_id: u32,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    async fn get_key_ids(&self) -> Result<(u32, u32, u32), Self::Error> {
        todo!()
    }

    async fn store_contact(&self, _contact: &Contact) -> Result<(), Self::Error> {
        todo!()
    }

    async fn load_contacts(&self) -> Result<Vec<Contact>, Self::Error> {
        todo!()
    }

    async fn remove_contact(&self, _service_id: &ServiceId) -> Result<(), Self::Error> {
        todo!()
    }

    async fn insert_service_id_for_nickname(
        &self,
        _nickname: &str,
        _service_id: &ServiceId,
    ) -> Result<(), Self::Error> {
        todo!()
    }

    async fn get_all_nicknames(&self) -> Result<Vec<ContactName>, Self::Error> {
        todo!()
    }

    async fn get_service_id_by_nickname(&self, _nickname: &str) -> Result<ServiceId, Self::Error> {
        todo!()
    }

    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair, Self::Error> {
        self.identity_key_store.get_identity_key_pair().await
    }

    async fn get_local_registration_id(&self) -> Result<u32, Self::Error> {
        self.identity_key_store.get_local_registration_id().await
    }

    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, Self::Error> {
        self.identity_key_store
            .save_identity(address, identity)
            .await
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        direction: Direction,
    ) -> Result<bool, Self::Error> {
        self.identity_key_store
            .is_trusted_identity(address, identity, direction)
            .await
    }

    async fn get_identity(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<IdentityKey>, Self::Error> {
        self.identity_key_store.get_identity(address).await
    }

    async fn get_pre_key(&self, prekey_id: PreKeyId) -> Result<PreKeyRecord, Self::Error> {
        self.pre_key_store.get_pre_key(prekey_id).await
    }

    async fn save_pre_key(
        &mut self,
        prekey_id: PreKeyId,
        record: &PreKeyRecord,
    ) -> Result<(), Self::Error> {
        self.pre_key_store.save_pre_key(prekey_id, record).await
    }

    async fn remove_pre_key(&mut self, prekey_id: PreKeyId) -> Result<(), Self::Error> {
        self.pre_key_store.remove_pre_key(prekey_id).await
    }

    async fn get_signed_pre_key(
        &self,
        id: SignedPreKeyId,
    ) -> Result<SignedPreKeyRecord, Self::Error> {
        self.signed_pre_key_store.get_signed_pre_key(id).await
    }

    async fn save_signed_pre_key(
        &mut self,
        id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<(), Self::Error> {
        self.signed_pre_key_store
            .save_signed_pre_key(id, record)
            .await
    }

    async fn get_kyber_pre_key(
        &self,
        kyber_prekey_id: KyberPreKeyId,
    ) -> Result<KyberPreKeyRecord, Self::Error> {
        self.kyber_pre_key_store
            .get_kyber_pre_key(kyber_prekey_id)
            .await
    }

    async fn save_kyber_pre_key(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<(), Self::Error> {
        self.kyber_pre_key_store
            .save_kyber_pre_key(kyber_prekey_id, record)
            .await
    }

    async fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, Self::Error> {
        self.session_store.load_session(address).await
    }

    async fn load_deniable_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, Self::Error> {
        self.session_store.load_session(address).await
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), Self::Error> {
        self.session_store.store_session(address, record).await
    }

    async fn store_deniable_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), Self::Error> {
        self.session_store.store_session(address, record).await
    }

    async fn store_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
        record: &SenderKeyRecord,
    ) -> Result<(), Self::Error> {
        self.sender_key_store
            .store_sender_key(sender, distribution_id, record)
            .await
    }

    async fn load_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
    ) -> Result<Option<SenderKeyRecord>, Self::Error> {
        self.sender_key_store
            .load_sender_key(sender, distribution_id)
            .await
    }

    async fn set_password(&mut self, new_password: String) -> Result<(), Self::Error> {
        self.password = new_password;
        Ok(())
    }

    async fn get_password(&self) -> Result<String, Self::Error> {
        Ok(self.password.clone())
    }

    async fn set_aci(&mut self, new_aci: Aci) -> Result<(), Self::Error> {
        self.aci = new_aci;
        Ok(())
    }

    async fn get_aci(&self) -> Result<Aci, Self::Error> {
        Ok(self.aci)
    }

    async fn set_pni(&mut self, new_pni: Pni) -> Result<(), Self::Error> {
        self.pni = new_pni;
        Ok(())
    }

    async fn get_pni(&self) -> Result<Pni, Self::Error> {
        Ok(self.pni)
    }

    async fn get_deniable_message(&self) -> Result<(u32, Vec<u8>), Self::Error> {
        todo!()
    }

    async fn get_deniable_message_by_id(&self, _: u32) -> Result<Vec<u8>, Self::Error> {
        todo!()
    }

    async fn store_deniable_message(&self, _: Option<u32>, _: Vec<u8>) -> Result<(), Self::Error> {
        todo!()
    }

    async fn remove_deniable_message(&self, _: u32) -> Result<(), Self::Error> {
        todo!()
    }
}
