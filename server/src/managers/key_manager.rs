use super::manager::Manager;
use crate::{account::AuthenticatedDevice, error::ApiError, storage::database::SignalDatabase};
use anyhow::Result;
use axum::http::StatusCode;
use common::web_api::{PreKeyResponse, PreKeyResponseItem, SetKeyRequest, UploadSignedPreKey};
use libsignal_core::{ProtocolAddress, ServiceId, ServiceIdKind};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Default)]
pub struct KeyManager<T>
where
    T: SignalDatabase,
{
    db: T,
}

impl<T> Manager for KeyManager<T>
where
    T: SignalDatabase,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T: SignalDatabase> KeyManager<T> {
    pub fn new(db: T) -> Self {
        Self { db }
    }
    pub async fn handle_put_keys(
        &self,
        auth_device: &AuthenticatedDevice,
        bundle: SetKeyRequest,
        kind: ServiceIdKind,
    ) -> Result<(), ApiError> {
        let address = auth_device.get_protocol_address(kind);
        let identity_key = match kind {
            ServiceIdKind::Aci => auth_device.account().aci_identity_key(),
            ServiceIdKind::Pni => auth_device.account().pni_identity_key(),
        };

        let verify_key = |prekey: &UploadSignedPreKey| -> Result<(), ApiError> {
            if !identity_key
                .public_key()
                .verify_signature(&prekey.public_key, &prekey.signature)
                .unwrap()
            {
                return Err(ApiError {
                    status_code: StatusCode::UNPROCESSABLE_ENTITY,
                    body: "Invalid signature".to_owned(),
                });
            }
            Ok(())
        };

        bundle
            .signed_pre_key
            .as_ref()
            .map(|key| verify_key(key))
            .transpose()?;
        bundle
            .pq_last_resort_pre_key
            .as_ref()
            .map(|key| verify_key(key))
            .transpose()?;
        bundle
            .pq_pre_key
            .as_ref()
            .map(|keys| {
                keys.iter()
                    .map(|key| verify_key(key))
                    .collect::<Result<(), ApiError>>()
            })
            .transpose()?;

        if let Some(prekeys) = bundle.pre_key {
            self.db
                .store_one_time_ec_pre_keys(prekeys, &address)
                .await
                .map_err(|_| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Database fault".into(),
                })?;
        }

        if let Some(ref prekey) = bundle.signed_pre_key {
            self.db
                .store_signed_pre_key(prekey, &address)
                .await
                .map_err(|_| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Database fault".to_owned(),
                })?;
        }

        if let Some(prekeys) = bundle.pq_pre_key {
            self.db
                .store_one_time_pq_pre_keys(prekeys, &address)
                .await
                .map_err(|_| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Database fault".to_owned(),
                })?;
        }

        if let Some(ref prekey) = bundle.pq_last_resort_pre_key {
            self.db
                .store_pq_signed_pre_key(prekey, &address)
                .await
                .map_err(|_| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Database fault".to_owned(),
                })?;
        }

        Ok(())
    }

    pub async fn handle_get_keys_id_device_id<S: SignalDatabase>(
        &self,
        database: &S,
        auth_device: &AuthenticatedDevice,
        target_service_id: ServiceId,
        target_device_id: String,
    ) -> Result<PreKeyResponse, ApiError> {
        async fn get_key<S: SignalDatabase>(
            database: &S,
            service_id: &ServiceId,
            address: &ProtocolAddress,
            registration_id: u32,
        ) -> Result<PreKeyResponseItem, ApiError> {
            let bundle = database
                .get_key_bundle(address)
                .await
                .map_err(|err| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: format!("Could not fetch user key bundle: {}", err),
                })?;
            let (pq_pre_key, signed_pre_key) = match service_id {
                ServiceId::Aci(_) => (bundle.aci_pq_pre_key, bundle.aci_signed_pre_key),
                ServiceId::Pni(_) => (bundle.pni_pq_pre_key, bundle.pni_signed_pre_key),
            };

            let prekey = database
                .get_one_time_ec_pre_key(address)
                .await
                .map_err(|_| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Could not fetch user pre key".to_owned(),
                })?;

            Ok(PreKeyResponseItem::new(
                address.device_id(),
                registration_id,
                prekey,
                pq_pre_key,
                signed_pre_key,
            ))
        }

        database
            .get_account(&auth_device.account().aci().into())
            .await
            .map_err(|_| ApiError {
                status_code: StatusCode::UNAUTHORIZED,
                body: "".to_owned(),
            })?;

        let target_account = database
            .get_account(&target_service_id)
            .await
            .map_err(|_| ApiError {
                status_code: StatusCode::NOT_FOUND,
                body: format!(
                    "Could not find account for service id: {}",
                    target_service_id.service_id_string()
                ),
            })?;

        let devices = match target_device_id.parse::<u32>() {
            Ok(device_id) => {
                vec![database
                    .get_device(&ProtocolAddress::new(
                        target_service_id.service_id_string(),
                        device_id.into(),
                    ))
                    .await
                    .map_err(|_| ApiError {
                        status_code: StatusCode::NOT_FOUND,
                        body: format!("Device id does not exist: {}", device_id),
                    })?]
            }
            _ if target_device_id == "*" => database
                .get_all_devices(&target_service_id)
                .await
                .map_err(|_| ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    body: "Could not get all targets devices".into(),
                })?,
            _ => {
                return Err(ApiError {
                    status_code: StatusCode::UNPROCESSABLE_ENTITY,
                    body: "Target device id is not a u32 or '*'".into(),
                })
            }
        };

        let mut keys = Vec::new();
        for device in devices.iter() {
            keys.push(
                get_key(
                    database,
                    &target_service_id,
                    &ProtocolAddress::new(
                        target_service_id.service_id_string(),
                        device.device_id(),
                    ),
                    device.registration_id(),
                )
                .await?,
            );
        }

        Ok(PreKeyResponse::new(
            match target_service_id {
                ServiceId::Aci(_) => target_account.aci_identity_key(),
                ServiceId::Pni(_) => target_account.pni_identity_key(),
            },
            keys,
        ))
    }

    // The Signal endpoint /v2/keys/check says that a u64 id is needed, however their ids, such as
    // KyperPreKeyID only supports u32. Here only a u32 is used and therefore only a 4 byte size
    // instead of the sugested u64.
    pub async fn handle_post_keycheck(
        &self,
        auth_device: &AuthenticatedDevice,
        kind: ServiceIdKind, // In Signal this is called IdentityType
        usr_digest: [u8; 32],
    ) -> Result<bool, ApiError> {
        let service_id = match kind {
            ServiceIdKind::Aci => auth_device.account().aci().into(),
            ServiceIdKind::Pni => auth_device.account().pni().into(),
        };
        let address = auth_device.get_protocol_address(kind);
        let bundle = self
            .db
            .get_key_bundle(&address)
            .await
            .map_err(|_| ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                body: "Could not fetch user key bundle".to_owned(),
            })?;

        let mut digest = Sha256::new();
        match service_id {
            ServiceId::Aci(_) => {
                digest.update(
                    auth_device
                        .account()
                        .aci_identity_key()
                        .public_key()
                        .public_key_bytes()
                        .map_err(|_| ApiError {
                            status_code: StatusCode::INTERNAL_SERVER_ERROR,
                            body: "Could not convert key to bytes".to_owned(),
                        })?,
                );
                digest.update(bundle.aci_signed_pre_key.key_id.to_be_bytes());
                digest.update(&bundle.aci_signed_pre_key.public_key);
                digest.update(bundle.aci_pq_pre_key.key_id.to_be_bytes());
                digest.update(&bundle.aci_pq_pre_key.public_key);
            }
            ServiceId::Pni(_) => {
                digest.update(
                    auth_device
                        .account()
                        .pni_identity_key()
                        .public_key()
                        .public_key_bytes()
                        .map_err(|_| ApiError {
                            status_code: StatusCode::INTERNAL_SERVER_ERROR,
                            body: "Could not convert key to bytes".to_owned(),
                        })?,
                );
                digest.update(bundle.pni_signed_pre_key.key_id.to_be_bytes());
                digest.update(&bundle.pni_signed_pre_key.public_key);
                digest.update(bundle.pni_pq_pre_key.key_id.to_be_bytes());
                digest.update(&bundle.pni_pq_pre_key.public_key);
            }
        }

        let server_digest: [u8; 32] = digest.finalize().into();

        Ok(server_digest == usr_digest)
    }

    pub async fn get_one_time_pre_key_count(&self, service_id: &ServiceId) -> Result<(u32, u32)> {
        Ok((
            self.db.get_one_time_ec_pre_key_count(service_id).await?,
            self.db.get_one_time_pq_pre_key_count(service_id).await?,
        ))
    }
}

#[cfg(test)]
mod key_manager_tests {
    use super::*;
    use crate::test_utils::{database::*, user::*};
    use crate::{
        account::Account,
        test_utils::key::{
            new_device_pre_key_bundle, new_upload_pre_keys, new_upload_signed_pre_key,
        },
    };
    use libsignal_protocol::{IdentityKey, KeyPair};
    use rand::rngs::OsRng;
    use sha2::{Digest, Sha256};

    pub fn new_account_from_identity_key(identity_key: IdentityKey) -> Account {
        Account::new(
            new_pni(),
            identity_key,
            identity_key,
            new_device(),
            new_uuid().into(),
        )
    }

    #[tokio::test]
    async fn get_keys_test() {
        let database = database_connect().await;
        let km = KeyManager::new(database.clone());

        let (target, target_device, target_address) = new_account_and_device_and_address();

        let key_bundle = new_device_pre_key_bundle();
        let one_time = new_upload_pre_keys(1);

        database.add_account(&target).await.unwrap();

        database
            .store_key_bundle(&key_bundle, &target_address)
            .await
            .unwrap();
        database
            .store_one_time_ec_pre_keys(one_time.clone(), &target_address)
            .await
            .unwrap();

        let auth_device1 = new_authenticated_device();
        database.add_account(auth_device1.account()).await.unwrap();

        let keys = km
            .handle_get_keys_id_device_id(
                &database,
                &auth_device1,
                target.aci().into(),
                target_device.device_id().to_string(),
            )
            .await
            .unwrap();

        let device_bundle = keys.devices();

        database.delete_account(&target.aci().into()).await.unwrap();

        assert_eq!(
            IdentityKey::decode(keys.identity_key()).unwrap(),
            target.aci_identity_key()
        );
        assert!(device_bundle.len() == 1);

        assert_eq!(
            device_bundle[0].device_id().clone(),
            target_device.device_id()
        );
        assert_eq!(
            device_bundle[0].registration_id(),
            target_device.registration_id()
        );
        assert_eq!(
            device_bundle[0].pre_key().clone(),
            Some(one_time[0].clone())
        );
        assert_eq!(
            device_bundle[0].signed_pre_key().clone(),
            key_bundle.pni_signed_pre_key
        );
        assert_eq!(
            device_bundle[0].pq_pre_key().clone(),
            key_bundle.pni_pq_pre_key
        );
    }

    #[tokio::test]
    async fn get_all_keys_test() {
        let database = database_connect().await;
        let km = KeyManager::new(database.clone());

        let (target, target_device, target_address) = new_account_and_device_and_address();
        let device2 = new_device();
        let address2 = ProtocolAddress::new(target_address.name().into(), device2.device_id());

        let key_bundle = new_device_pre_key_bundle();
        let one_time = new_upload_pre_keys(1);

        database.add_account(&target).await.unwrap();
        database
            .add_device(&target.aci().into(), &device2)
            .await
            .unwrap();

        database
            .store_key_bundle(&key_bundle, &target_address)
            .await
            .unwrap();
        database
            .store_one_time_ec_pre_keys(one_time.clone(), &target_address)
            .await
            .unwrap();
        database
            .store_key_bundle(&key_bundle, &address2)
            .await
            .unwrap();
        database
            .store_one_time_ec_pre_keys(one_time.clone(), &address2)
            .await
            .unwrap();

        let auth_device1 = new_authenticated_device();
        database.add_account(auth_device1.account()).await.unwrap();

        let keys = km
            .handle_get_keys_id_device_id(&database, &auth_device1, target.aci().into(), "*".into())
            .await
            .unwrap();

        let device_bundle = keys.devices();

        database.delete_account(&target.aci().into()).await.unwrap();

        assert_eq!(
            IdentityKey::decode(keys.identity_key()).unwrap(),
            target.aci_identity_key()
        );
        assert!(device_bundle.len() == 2);

        assert_eq!(
            device_bundle[0].device_id().clone(),
            target_device.device_id()
        );
        assert_eq!(
            device_bundle[0].registration_id(),
            target_device.registration_id()
        );
        assert_eq!(
            device_bundle[0].pre_key().clone(),
            Some(one_time[0].clone())
        );
        assert_eq!(
            device_bundle[0].signed_pre_key().clone(),
            key_bundle.pni_signed_pre_key
        );
        assert_eq!(
            device_bundle[0].pq_pre_key().clone(),
            key_bundle.pni_pq_pre_key
        );
        assert_eq!(device_bundle[1].device_id().clone(), device2.device_id());
        assert_eq!(
            device_bundle[1].registration_id(),
            device2.registration_id()
        );
        assert_eq!(
            device_bundle[1].pre_key().clone(),
            Some(one_time[0].clone())
        );
        assert_eq!(
            device_bundle[1].signed_pre_key().clone(),
            key_bundle.pni_signed_pre_key
        );
        assert_eq!(
            device_bundle[1].pq_pre_key().clone(),
            key_bundle.pni_pq_pre_key
        );
    }

    #[tokio::test]
    async fn put_keys_test() {
        let database = database_connect().await;
        let km = KeyManager::new(database.clone());

        let mut csprng = OsRng;
        let identity_key = KeyPair::generate(&mut csprng);
        let account = new_account_from_identity_key(IdentityKey::from(identity_key.public_key));
        let device = account.devices()[0].clone();
        let auth_device = AuthenticatedDevice::new(account, device);
        let target_address = auth_device.get_protocol_address(ServiceIdKind::Aci);

        let prekey = new_upload_pre_keys(1);
        let signed_pre_key = new_upload_signed_pre_key(Some(identity_key.private_key));
        let pq_pre_key = new_upload_signed_pre_key(Some(identity_key.private_key));
        let pq_last_resort_pre_key = new_upload_signed_pre_key(Some(identity_key.private_key));

        let request = SetKeyRequest {
            pre_key: Some(prekey.clone()),
            signed_pre_key: Some(signed_pre_key.clone()),
            pq_pre_key: Some(vec![pq_pre_key.clone()]),
            pq_last_resort_pre_key: Some(pq_last_resort_pre_key.clone()),
        };

        database.add_account(auth_device.account()).await.unwrap();

        km.handle_put_keys(&auth_device, request.clone(), ServiceIdKind::Aci)
            .await
            .unwrap();

        let prekey_db = database
            .get_one_time_ec_pre_key(&target_address)
            .await
            .unwrap();
        let signed_pre_key_db = get_aci_signed_pre_key(
            &database,
            request.signed_pre_key.unwrap().key_id,
            &target_address,
        )
        .await
        .unwrap();
        let pq_pre_key_db = database
            .get_one_time_pq_pre_key(&target_address)
            .await
            .unwrap();
        let pq_last_resort_pre_key_db = get_aci_pq_last_resort_pre_key(
            &database,
            request.pq_last_resort_pre_key.unwrap().key_id,
            &target_address,
        )
        .await
        .unwrap();

        database
            .delete_account(&auth_device.account().pni().into())
            .await
            .unwrap();

        assert_eq!(Some(prekey[0].clone()), prekey_db);
        assert_eq!(signed_pre_key, signed_pre_key_db);
        assert_eq!(pq_pre_key, pq_pre_key_db);
        assert_eq!(pq_last_resort_pre_key, pq_last_resort_pre_key_db);
    }
    #[tokio::test]
    async fn check_keys_test() {
        let database = database_connect().await;
        let auth_device = new_authenticated_device();
        database.add_account(auth_device.account()).await.unwrap();

        let key_bundle = new_device_pre_key_bundle();
        let signed_pre_key = key_bundle.pni_signed_pre_key.clone();
        let pq_last_resort_pre_key = key_bundle.pni_pq_pre_key.clone();

        database
            .store_key_bundle(
                &key_bundle,
                &auth_device.get_protocol_address(ServiceIdKind::Pni),
            )
            .await
            .unwrap();

        let mut usr_digest = Sha256::new();
        usr_digest.update(
            auth_device
                .account()
                .pni_identity_key()
                .public_key()
                .public_key_bytes()
                .unwrap(),
        );
        usr_digest.update(signed_pre_key.key_id.to_be_bytes());
        usr_digest.update(signed_pre_key.public_key);
        usr_digest.update(pq_last_resort_pre_key.key_id.to_be_bytes());
        usr_digest.update(pq_last_resort_pre_key.public_key);

        let res = KeyManager::new(database.clone())
            .handle_post_keycheck(
                &auth_device,
                ServiceIdKind::Pni,
                usr_digest.finalize().into(),
            )
            .await
            .unwrap();

        database
            .delete_account(&auth_device.account().pni().into())
            .await
            .unwrap();

        assert!(res);
    }
}
