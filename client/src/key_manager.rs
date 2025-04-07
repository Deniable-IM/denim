use crate::storage::database::ClientDB;
use crate::storage::generic::ProtocolStore;
use common::utils::time_now;
use common::web_api::{SetKeyRequest, UploadPreKey, UploadSignedPreKey};
use derive_more::derive::{Display, Error, From};
use libsignal_protocol::{
    kem, GenericSignedPreKey, IdentityKeyStore, KeyPair, KyberPreKeyRecord, KyberPreKeyStore,
    PreKeyRecord, PreKeyStore, SignalProtocolError, SignedPreKeyRecord, SignedPreKeyStore,
};
use rand::rngs::OsRng;
use rand::{CryptoRng, Rng};
use std::collections::HashMap;

#[derive(Debug, Clone, Hash, Eq, PartialEq, Display)]
pub enum PreKeyType {
    #[display("signed pre key")]
    Signed,
    #[display("kyber pre key")]
    Kyber,
    #[display("pre key")]
    OneTime,
}

#[derive(Debug, Display, From)]
pub enum KeyType {
    #[from]
    PreKey(PreKeyType),
    #[display("identity key")]
    IdentityKey,
}

pub struct KeyManager {
    key_incrementer_map: HashMap<PreKeyType, u32>,
}

#[derive(Debug, Display, Error)]
#[display("Could not {} {}: {}", err_type, key_type, error)]
pub struct KeyManagerError {
    pub err_type: KeyManagerErrorType,
    pub key_type: KeyType,
    pub error: SignalProtocolError,
}

#[derive(Debug, Display, Error)]
pub enum KeyManagerErrorType {
    #[display("generate")]
    Generate,
    #[display("store")]
    Store,
    #[display("sign")]
    Signature,
    #[display("get")]
    Get,
}

impl Default for KeyManager {
    fn default() -> Self {
        Self {
            key_incrementer_map: HashMap::from([
                (PreKeyType::Signed, 0u32),
                (PreKeyType::Kyber, 0u32),
                (PreKeyType::OneTime, 0u32),
            ]),
        }
    }
}

impl KeyManager {
    pub fn new(signed: u32, kyber: u32, one_time: u32) -> Self {
        Self {
            key_incrementer_map: HashMap::from([
                (PreKeyType::Signed, signed),
                (PreKeyType::Kyber, kyber),
                (PreKeyType::OneTime, one_time),
            ]),
        }
    }
    fn get_new_key_id(&mut self, key_type: PreKeyType) -> u32 {
        let id = *self.key_incrementer_map.get(&key_type).unwrap();
        *self.key_incrementer_map.get_mut(&key_type).unwrap() += 1u32;
        id
    }

    pub async fn generate_pre_key<R: Rng + CryptoRng, PK: PreKeyStore>(
        &mut self,
        pre_key_store: &mut PK,
        csprng: &mut R,
    ) -> Result<PreKeyRecord, KeyManagerError> {
        let id = self.get_new_key_id(PreKeyType::OneTime).into();

        let key_pair = KeyPair::generate(csprng);
        let record = PreKeyRecord::new(id, &key_pair);
        pre_key_store
            .save_pre_key(id, &record)
            .await
            .map_err(|error| KeyManagerError {
                key_type: PreKeyType::OneTime.into(),
                err_type: KeyManagerErrorType::Store,
                error,
            })?;
        Ok(record)
    }

    pub async fn generate_signed_pre_key<
        R: Rng + CryptoRng,
        IK: IdentityKeyStore,
        SPK: SignedPreKeyStore,
    >(
        &mut self,
        identity_key_store: &mut IK,
        signed_pre_key_store: &mut SPK,
        csprng: &mut R,
    ) -> Result<SignedPreKeyRecord, KeyManagerError> {
        let id = self.get_new_key_id(PreKeyType::Signed).into();
        let signed_pre_key_pair = KeyPair::generate(csprng);
        let signature = identity_key_store
            .get_identity_key_pair()
            .await
            .map_err(|error| KeyManagerError {
                key_type: PreKeyType::Signed.into(),
                err_type: KeyManagerErrorType::Get,
                error,
            })?
            .private_key()
            .calculate_signature(&signed_pre_key_pair.public_key.serialize(), csprng)
            .map_err(|error| KeyManagerError {
                key_type: PreKeyType::Signed.into(),
                err_type: KeyManagerErrorType::Signature,
                error,
            })?;

        let record = SignedPreKeyRecord::new(id, time_now(), &signed_pre_key_pair, &signature);

        signed_pre_key_store
            .save_signed_pre_key(id, &record)
            .await
            .map_err(|error| KeyManagerError {
                key_type: PreKeyType::Signed.into(),
                err_type: KeyManagerErrorType::Store,
                error,
            })?;

        Ok(record)
    }

    // always signed
    pub async fn generate_kyber_pre_key<IK: IdentityKeyStore, KPK: KyberPreKeyStore>(
        &mut self,
        identity_key_store: &mut IK,
        kyber_pre_key_store: &mut KPK,
    ) -> Result<KyberPreKeyRecord, KeyManagerError> {
        let id = self.get_new_key_id(PreKeyType::Kyber).into();
        let record = KyberPreKeyRecord::generate(
            kem::KeyType::Kyber1024,
            id,
            identity_key_store
                .get_identity_key_pair()
                .await
                .map_err(|error| KeyManagerError {
                    key_type: KeyType::IdentityKey,
                    err_type: KeyManagerErrorType::Get,
                    error,
                })?
                .private_key(),
        )
        .map_err(|error| KeyManagerError {
            key_type: PreKeyType::Kyber.into(),
            err_type: KeyManagerErrorType::Generate,
            error,
        })?;
        kyber_pre_key_store
            .save_kyber_pre_key(id, &record)
            .await
            .map_err(|error| KeyManagerError {
                key_type: PreKeyType::Kyber.into(),
                err_type: KeyManagerErrorType::Store,
                error,
            })?;
        Ok(record)
    }

    pub async fn generate_key_bundle<T: ClientDB>(
        &mut self,
        store: &mut ProtocolStore<T>,
    ) -> Result<SetKeyRequest, KeyManagerError> {
        let mut pre_keys: Vec<UploadPreKey> = Vec::new();
        let mut pq_signed_pre_keys: Vec<UploadSignedPreKey> = Vec::new();
        let mut rng = OsRng;

        for _ in 0..100 {
            pre_keys.push(UploadPreKey::from(
                self.generate_pre_key(store, &mut rng).await?,
            ));

            pq_signed_pre_keys.push(UploadSignedPreKey::from(
                self.generate_kyber_pre_key(
                    &mut store.identity_key_store,
                    &mut store.kyber_pre_key_store,
                )
                .await?,
            ));
        }

        let signed_pre_key = self
            .generate_signed_pre_key(
                &mut store.identity_key_store,
                &mut store.signed_pre_key_store,
                &mut rng,
            )
            .await?;
        let pq_last_resort_pre_key = self
            .generate_kyber_pre_key(
                &mut store.identity_key_store,
                &mut store.kyber_pre_key_store,
            )
            .await?;

        Ok(SetKeyRequest::new(
            Some(pre_keys),
            Some(UploadSignedPreKey::from(signed_pre_key)),
            Some(pq_signed_pre_keys),
            Some(UploadSignedPreKey::from(pq_last_resort_pre_key)),
        ))
    }
}

#[cfg(test)]
mod key_manager_tests {
    use std::sync::Arc;

    use crate::{
        key_manager::{KeyManager, PreKeyType},
        storage::{generic::ProtocolStore, in_memory::InMemory},
        test_utils::user::{new_aci, new_pni},
    };

    use async_std::sync::Mutex;
    use libsignal_protocol::{
        GenericSignedPreKey, IdentityKeyPair, KyberPreKeyStore, PreKeyStore, SignedPreKeyStore,
    };
    use rand::rngs::OsRng;

    fn store(reg: u32) -> ProtocolStore<InMemory> {
        ProtocolStore::new(Arc::new(Mutex::new(InMemory::new(
            "password".to_string(),
            new_aci(),
            new_pni(),
            IdentityKeyPair::generate(&mut OsRng),
            reg,
        ))))
    }

    #[test]
    fn get_id_test() {
        let mut manager = KeyManager::default();
        let id0 = manager.get_new_key_id(PreKeyType::OneTime);
        assert_eq!(id0, 0);
        let id1 = manager.get_new_key_id(PreKeyType::OneTime);
        assert_eq!(id1, 1);
    }

    #[tokio::test]
    async fn generate_kyper_key() {
        let mut store = store(0);
        let mut manager = KeyManager::default();
        let key = manager
            .generate_kyber_pre_key(
                &mut store.identity_key_store,
                &mut store.kyber_pre_key_store,
            )
            .await
            .unwrap();
        let stored_sign = store
            .kyber_pre_key_store
            .get_kyber_pre_key(key.id().unwrap())
            .await
            .unwrap()
            .signature()
            .unwrap();

        assert_eq!(key.signature().unwrap(), stored_sign);
    }

    #[tokio::test]
    async fn generate_signed_key() {
        let mut rng = OsRng;
        let mut store = store(0);
        let mut manager = KeyManager::default();
        let key = manager
            .generate_signed_pre_key(
                &mut store.identity_key_store,
                &mut store.signed_pre_key_store,
                &mut rng,
            )
            .await
            .unwrap();

        let stored_sign = store
            .signed_pre_key_store
            .get_signed_pre_key(key.id().unwrap())
            .await
            .unwrap()
            .signature()
            .unwrap();

        assert_eq!(key.signature().unwrap(), stored_sign);
    }

    #[tokio::test]
    async fn generate_onetime_key() {
        let mut rng = OsRng;
        let mut store = store(0);
        let mut manager = KeyManager::default();
        let key = manager
            .generate_pre_key(&mut store.pre_key_store, &mut rng)
            .await
            .unwrap();

        let stored_key_pair = store
            .pre_key_store
            .get_pre_key(key.id().unwrap())
            .await
            .unwrap()
            .key_pair()
            .unwrap();

        assert_eq!(key.public_key().unwrap(), stored_key_pair.public_key);
    }

    #[tokio::test]
    async fn generate_key_bundle() {
        let mut store = store(0);
        let mut manager = KeyManager::default();
        let keys = manager.generate_key_bundle(&mut store).await.unwrap();

        for (pre_key, pq_pre_key) in keys
            .pre_key
            .unwrap()
            .iter()
            .zip(keys.pq_pre_key.unwrap().iter())
        {
            assert_eq!(
                store
                    .pre_key_store
                    .get_pre_key(pre_key.key_id.into())
                    .await
                    .unwrap()
                    .public_key()
                    .unwrap()
                    .serialize(),
                pre_key.public_key
            );

            assert_eq!(
                store
                    .kyber_pre_key_store
                    .get_kyber_pre_key(pq_pre_key.key_id.into())
                    .await
                    .unwrap()
                    .public_key()
                    .unwrap()
                    .serialize(),
                pq_pre_key.public_key
            );

            assert_eq!(
                store
                    .kyber_pre_key_store
                    .get_kyber_pre_key(pq_pre_key.key_id.into())
                    .await
                    .unwrap()
                    .signature()
                    .unwrap(),
                pq_pre_key.signature.to_vec()
            );
        }

        let stored_signed_key = store
            .signed_pre_key_store
            .get_signed_pre_key(keys.signed_pre_key.as_ref().unwrap().key_id.into())
            .await
            .unwrap();

        assert_eq!(
            stored_signed_key.public_key().unwrap().serialize(),
            keys.signed_pre_key.as_ref().unwrap().public_key
        );
        assert_eq!(
            stored_signed_key.signature().unwrap(),
            keys.signed_pre_key.as_ref().unwrap().signature.to_vec()
        );
    }
}
