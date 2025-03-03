use crate::{
    account::{Account, Device},
    database::SignalDatabase,
};
use anyhow::{anyhow, bail, Result};
use axum::async_trait;
use common::{
    signalservice::Envelope,
    web_api::{DeviceCapabilityType, DevicePreKeyBundle, UploadPreKey, UploadSignedPreKey},
};
use libsignal_core::{Aci, Pni, ProtocolAddress, ServiceId};
use libsignal_protocol::{IdentityKey, PublicKey};
use sqlx::{postgres::PgPoolOptions, PgConnection, Pool, Postgres};

#[derive(Clone)]
pub struct PostgresDatabase {
    pool: Pool<Postgres>,
}

impl PostgresDatabase {
    pub async fn connect(database_url: String) -> Self {
        dotenv::dotenv().ok();
        let db_url = std::env::var(database_url).expect("Unable to read database url env var");
        Self {
            pool: PgPoolOptions::new()
                .max_connections(100)
                .connect(&db_url)
                .await
                .expect("Failed to connect to the database."),
        }
    }

    pub fn pool(&self) -> &Pool<Postgres> {
        &self.pool
    }

    async fn store_aci_signed_pre_key(
        &self,
        spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()> {
        let pool = &mut self.pool.acquire().await?;
        store_aci_signed_pre_key(pool, spk, address).await
    }

    async fn store_pni_signed_pre_key(
        &self,
        spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()> {
        let pool = &mut self.pool.acquire().await?;
        store_pni_signed_pre_key(pool, spk, address).await
    }

    async fn store_pq_aci_signed_pre_key(
        &self,
        pq_spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()> {
        let pool = &mut self.pool.acquire().await?;
        store_pq_aci_signed_pre_key(pool, pq_spk, address).await
    }

    async fn store_pq_pni_signed_pre_key(
        &self,
        pq_spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()> {
        let pool = &mut self.pool.acquire().await?;
        store_pq_pni_signed_pre_key(pool, pq_spk, address).await
    }
}

#[async_trait]
impl SignalDatabase for PostgresDatabase {
    async fn add_account(&self, account: &Account) -> Result<()> {
        if let Err(err) = sqlx::query!(
            r#"
            INSERT INTO accounts (aci, pni, aci_identity_key, pni_identity_key, phone_number)
            VALUES ($1, $2, $3, $4, $5)
            "#,
            account.aci().service_id_string(),
            account.pni().service_id_string(),
            &*account.aci_identity_key().serialize(),
            &*account.pni_identity_key().serialize(),
            account.phone_number(),
        )
        .execute(&self.pool)
        .await
        {
            bail!(err);
        }
        self.add_device(&account.aci().into(), &account.devices()[0])
            .await
    }

    async fn get_account(&self, service_id: &ServiceId) -> Result<Account> {
        let devices = self.get_all_devices(service_id).await?;

        sqlx::query!(
            r#"
            SELECT aci, 
                   pni, 
                   aci_identity_key, 
                   pni_identity_key, 
                   phone_number
            FROM accounts
            WHERE aci = $1 
               OR pni = $1
            "#,
            service_id.service_id_string(),
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| {
            Account::from_db(
                Aci::parse_from_service_id_string(&row.aci).unwrap(),
                Pni::parse_from_service_id_string(&row.pni).unwrap(),
                IdentityKey::new(PublicKey::deserialize(row.aci_identity_key.as_slice()).unwrap()),
                IdentityKey::new(PublicKey::deserialize(row.pni_identity_key.as_slice()).unwrap()),
                devices,
                row.phone_number,
            )
        })
        .map_err(|err| err.into())
    }

    async fn get_account_from_phonenumber_without_devices(&self, phone_number: &str) -> Result<Account> {
        sqlx::query!(
            r#"
            SELECT aci, 
                   pni, 
                   aci_identity_key, 
                   pni_identity_key, 
                   phone_number
            FROM accounts
            WHERE phone_number = $1
            "#,
            phone_number,
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| {
            Account::from_db(
                Aci::parse_from_service_id_string(&row.aci).unwrap(),
                Pni::parse_from_service_id_string(&row.pni).unwrap(),
                IdentityKey::new(PublicKey::deserialize(row.aci_identity_key.as_slice()).unwrap()),
                IdentityKey::new(PublicKey::deserialize(row.pni_identity_key.as_slice()).unwrap()),
                Vec::new(),
                row.phone_number,
            )
        })
        .map_err(|err| err.into())
    }

    async fn update_account_aci(&self, service_id: &ServiceId, new_aci: Aci) -> Result<()> {
        sqlx::query!(
            r#"
            UPDATE accounts
            SET aci = $2
            WHERE aci = $1 
               OR pni = $1
            "#,
            service_id.service_id_string(),
            new_aci.service_id_string()
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|err| err.into())
    }

    async fn update_account_pni(&self, service_id: &ServiceId, new_pni: Pni) -> Result<()> {
        sqlx::query!(
            r#"
            UPDATE accounts
            SET pni = $2
            WHERE aci = $1 
               OR pni = $1
            "#,
            service_id.service_id_string(),
            new_pni.service_id_string()
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|err| err.into())
    }

    async fn delete_account(&self, service_id: &ServiceId) -> Result<()> {
        sqlx::query!(
            r#"
            DELETE 
            FROM accounts
            WHERE aci = $1 
               OR pni = $1
            "#,
            service_id.service_id_string()
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|err| err.into())
    }

    async fn add_device(&self, service_id: &ServiceId, device: &Device) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO devices (owner, device_id, name, auth_token, salt, registration_id, pni_registration_id)
            SELECT id, 
                   $2, 
                   $3, 
                   $4, 
                   $5, 
                   $6, 
                   $7
            FROM accounts
            WHERE aci = $1 
               OR pni = $1
            "#,
            service_id.service_id_string(),
            device.device_id().to_string(),
            device.name().as_bytes(),
            device.auth_token(),
            device.salt(),
            device.registration_id().to_string(),
            device.pni_registration_id().to_string(),
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|err| err.into())
    }

    async fn get_all_devices(&self, service_id: &ServiceId) -> Result<Vec<Device>> {
        let device_capabilities: Vec<(i32, DeviceCapabilityType)> =
            self.get_all_device_capabilities(service_id).await?;

        sqlx::query!(
            r#"
            SELECT id,
                   device_id,
                   name,
                   auth_token,
                   salt,
                   registration_id,
                   pni_registration_id
            FROM devices
            WHERE owner =
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
            "#,
            service_id.service_id_string()
        )
        .fetch_all(&self.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| {
                    Device::builder()
                        .device_id(row.device_id.parse::<u32>().unwrap().into())
                        .name(std::str::from_utf8(&row.name).unwrap().to_string())
                        .last_seen(0)
                        .created(0)
                        .auth_token(row.auth_token)
                        .salt(row.salt)
                        .registration_id(row.registration_id.parse().unwrap())
                        .pni_registration_id(row.pni_registration_id.parse().unwrap())
                        .capabilities(
                            device_capabilities
                                .iter()
                                .filter(|device_capability| device_capability.0 == row.id)
                                .map(|device_capability| device_capability.1.clone())
                                .collect(),
                        )
                        .build()
                })
                .collect()
        })
        .map_err(|err| err.into())
    }

    async fn get_device(&self, address: &ProtocolAddress) -> Result<Device> {
        let device_capabilities = self.get_device_capabilities(address).await?;

        sqlx::query!(
            r#"
            SELECT device_id,
                   name,
                   auth_token,
                   salt,
                   registration_id,
                   pni_registration_id
            FROM devices
            WHERE owner =
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
              AND device_id = $2
            "#,
            address.name(),
            address.device_id().to_string()
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| {
            Device::builder()
                .device_id(row.device_id.parse::<u32>().unwrap().into())
                .name(std::str::from_utf8(&row.name).unwrap().to_string())
                .last_seen(0)
                .created(0)
                .auth_token(row.auth_token)
                .salt(row.salt)
                .registration_id(row.registration_id.parse().unwrap())
                .pni_registration_id(row.pni_registration_id.parse().unwrap())
                .capabilities(device_capabilities)
                .build()
        })
        .map_err(|err| err.into())
    }

    async fn delete_device(&self, address: &ProtocolAddress) -> Result<()> {
        sqlx::query!(
            r#"
            DELETE 
            FROM devices
            WHERE owner =
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
             AND device_id = $2
            "#,
            address.name(),
            address.device_id().to_string()
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|err| err.into())
    }

    async fn get_device_capabilities(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Vec<DeviceCapabilityType>> {
        sqlx::query!(
            r#"
            SELECT
                capability_type
            FROM
                device_capabilities
            WHERE
                owner = (
                    SELECT
                        id
                    FROM
                        devices
                    WHERE
                        owner = (
                            SELECT
                                id
                            FROM
                                accounts
                            WHERE
                                aci = $1 OR
                                pni = $1
                        )
                        AND device_id = $2
                )
            "#,
            address.name(),
            address.device_id().to_string()
        )
        .fetch_all(&self.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| DeviceCapabilityType::from(row.capability_type))
                .collect()
        })
        .map_err(|err| err.into())
    }

    async fn get_all_device_capabilities(
        &self,
        service_id: &ServiceId,
    ) -> Result<Vec<(i32, DeviceCapabilityType)>> {
        sqlx::query!(
            r#"
            SELECT
                owner,
                capability_type
            FROM
                device_capabilities
            WHERE
                owner IN (
                    SELECT
                        id
                    FROM
                        devices
                    WHERE
                        owner = (
                            SELECT
                                id
                            FROM
                                accounts
                            WHERE
                                aci = $1 OR
                                pni = $1
                        )
                )
            "#,
            service_id.service_id_string()
        )
        .fetch_all(&self.pool)
        .await
        .map(|rows| {
            rows.into_iter()
                .map(|row| (row.owner, DeviceCapabilityType::from(row.capability_type)))
                .collect()
        })
        .map_err(|err| err.into())
    }

    async fn add_used_device_link_token(&self, device_link_token: String) -> Result<()> {
        sqlx::query!(
            r#"
            INSERT INTO
                used_device_link_tokens (device_link_token)
            VALUES
                ($1)
            "#,
            device_link_token,
        )
        .execute(&self.pool)
        .await
        .map(|_| ())
        .map_err(|err| err.into())
    }

    async fn push_message_queue(
        &self,
        address: &ProtocolAddress,
        messages: Vec<Envelope>,
    ) -> Result<()> {
        for msg in messages {
            let data = bincode::serialize(&msg)?;
            sqlx::query!(
                r#"
                INSERT INTO msq_queue (receiver, msg)
                SELECT id, 
                       $1
                FROM devices
                WHERE owner = 
                        (SELECT id
                         FROM accounts
                         WHERE aci = $2 
                            OR pni = $2)
                  AND device_id = $3
                "#,
                data,
                address.name(),
                address.device_id().to_string()
            )
            .execute(&self.pool)
            .await
            .map(|_| ())
            .map_err(|err| anyhow!("{}", err))?;
        }
        Ok(())
    }

    async fn pop_msg_queue(&self, address: &ProtocolAddress) -> Result<Vec<Envelope>> {
        sqlx::query!(
            r#"
            SELECT msq_queue.msg
            FROM msq_queue
            INNER JOIN devices on devices.id = msq_queue.receiver
            WHERE devices.owner = 
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
              AND devices.device_id = $2
            "#,
            address.name(),
            address.device_id().to_string()
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .try_fold(vec![], |mut acc, msg| -> Result<Vec<Envelope>> {
            acc.push(bincode::deserialize(&msg.msg)?);
            Ok(acc)
        })
    }

    async fn store_signed_pre_key(
        &self,
        spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()> {
        let service_id = ServiceId::parse_from_service_id_string(address.name())
            .ok_or_else(|| anyhow!("Invalid service id"))?;
        match service_id {
            ServiceId::Aci(_) => self.store_aci_signed_pre_key(spk, address).await?,
            ServiceId::Pni(_) => self.store_pni_signed_pre_key(spk, address).await?,
        }
        Ok(())
    }
    async fn store_pq_signed_pre_key(
        &self,
        spk: &UploadSignedPreKey,
        address: &ProtocolAddress,
    ) -> Result<()> {
        let service_id = ServiceId::parse_from_service_id_string(address.name())
            .ok_or_else(|| anyhow!("Invalid service id"))?;
        match service_id {
            ServiceId::Aci(_) => self.store_pq_aci_signed_pre_key(spk, address).await?,
            ServiceId::Pni(_) => self.store_pq_pni_signed_pre_key(spk, address).await?,
        }
        Ok(())
    }

    async fn store_key_bundle(
        &self,
        data: &DevicePreKeyBundle,
        address: &ProtocolAddress,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        store_aci_signed_pre_key(&mut tx, &data.aci_signed_pre_key, address).await?;
        store_pni_signed_pre_key(&mut tx, &data.pni_signed_pre_key, address).await?;
        store_pq_aci_signed_pre_key(&mut tx, &data.aci_pq_pre_key, address).await?;
        store_pq_pni_signed_pre_key(&mut tx, &data.pni_pq_pre_key, address).await?;

        sqlx::query!(
            r#"
            INSERT INTO device_keys (owner, aci_signed_pre_key, pni_signed_pre_key, aci_pq_last_resort_pre_key, pni_pq_last_resort_pre_key)
            SELECT devices.id, 
                   aci_signed_pre_key_store.id, 
                   pni_signed_pre_key_store.id, 
                   aci_pq_last_resort_pre_key_store.id, 
                   pni_pq_last_resort_pre_key_store.id
            FROM devices 
            INNER JOIN aci_signed_pre_key_store ON aci_signed_pre_key_store.owner = devices.id
            INNER JOIN pni_signed_pre_key_store ON pni_signed_pre_key_store.owner = devices.id
            INNER JOIN aci_pq_last_resort_pre_key_store ON aci_pq_last_resort_pre_key_store.owner = devices.id
            INNER JOIN pni_pq_last_resort_pre_key_store ON pni_pq_last_resort_pre_key_store.owner = devices.id
            WHERE devices.owner = 
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1) 
              AND devices.device_id = $2
            "#,
            address.name(),
            address.device_id().to_string(),
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await.map_err(|err| err.into())
    }

    async fn get_key_bundle(&self, address: &ProtocolAddress) -> Result<DevicePreKeyBundle> {
        sqlx::query!(
            r#"
            SELECT aci_signed_pre_key_store.key_id AS aspk_id,
                   aci_signed_pre_key_store.public_key AS aspk,
                   aci_signed_pre_key_store.signature AS aspk_sig,
                   pni_signed_pre_key_store.key_id AS pspk_id,
                   pni_signed_pre_key_store.public_key AS pspk,
                   pni_signed_pre_key_store.signature AS pspk_sig,
                   aci_pq_last_resort_pre_key_store.key_id AS apqlrpk_id,
                   aci_pq_last_resort_pre_key_store.public_key AS apqlrpk,
                   aci_pq_last_resort_pre_key_store.signature AS apqlrpk_sig,
                   pni_pq_last_resort_pre_key_store.key_id AS ppqlrpk_id,
                   pni_pq_last_resort_pre_key_store.public_key AS ppqlrpk,
                   pni_pq_last_resort_pre_key_store.signature AS ppqlrpk_sig
            FROM device_keys
            INNER JOIN aci_signed_pre_key_store ON aci_signed_pre_key_store.id = device_keys.aci_signed_pre_key
            INNER JOIN pni_signed_pre_key_store ON pni_signed_pre_key_store.id = device_keys.pni_signed_pre_key
            INNER JOIN aci_pq_last_resort_pre_key_store ON aci_pq_last_resort_pre_key_store.id = device_keys.aci_pq_last_resort_pre_key
            INNER JOIN pni_pq_last_resort_pre_key_store ON pni_pq_last_resort_pre_key_store.id = device_keys.pni_pq_last_resort_pre_key
            INNER JOIN devices ON devices.id = device_keys.owner
            WHERE devices.owner = 
                    (SELECT id
                    FROM accounts
                    WHERE aci = $1 
                       OR pni = $1)
              AND devices.device_id = $2
            "#,
            address.name(),
            address.device_id().to_string(),
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| {
            DevicePreKeyBundle {
                aci_signed_pre_key: UploadSignedPreKey {
                    key_id: row.aspk_id.parse().unwrap(),
                    public_key: row.aspk.into(),
                    signature: row.aspk_sig.into()
                },
                pni_signed_pre_key: UploadSignedPreKey {
                    key_id: row.pspk_id.parse().unwrap(),
                    public_key: row.pspk.into(),
                    signature: row.pspk_sig.into()
                },
                aci_pq_pre_key: UploadSignedPreKey {
                    key_id: row.apqlrpk_id.parse().unwrap(),
                    public_key: row.apqlrpk.into(),
                    signature: row.apqlrpk_sig.into()
                },
                pni_pq_pre_key: UploadSignedPreKey {
                    key_id: row.ppqlrpk_id.parse().unwrap(),
                    public_key: row.ppqlrpk.into(),
                    signature: row.ppqlrpk_sig.into()
                }
            }
        })
        .map_err(|err| err.into())
    }

    async fn get_one_time_ec_pre_key_count(&self, service_id: &ServiceId) -> Result<u32> {
        sqlx::query!(
            r#"
            SELECT COUNT(*) AS pre_key_count
            FROM one_time_ec_pre_key_store
            INNER JOIN devices on devices.id = one_time_ec_pre_key_store.owner
            WHERE devices.owner = 
            (SELECT id
             FROM accounts
             WHERE aci = $1 
                OR pni = $1)
            "#,
            service_id.service_id_string()
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| row.pre_key_count.unwrap_or_default() as u32)
        .map_err(|err| err.into())
    }

    async fn get_one_time_pq_pre_key_count(&self, service_id: &ServiceId) -> Result<u32> {
        sqlx::query!(
            r#"
            SELECT COUNT(*) AS pre_key_count
            FROM one_time_pq_pre_key_store
            INNER JOIN devices on devices.id = one_time_pq_pre_key_store.owner
            WHERE devices.owner = 
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
            "#,
            service_id.service_id_string()
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| row.pre_key_count.unwrap_or_default() as u32)
        .map_err(|err| err.into())
    }

    async fn store_one_time_ec_pre_keys(
        &self,
        otpks: Vec<UploadPreKey>,
        owner: &ProtocolAddress,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            r#"
            DELETE 
            FROM one_time_ec_pre_key_store
            WHERE owner = 
                (SELECT id
                 FROM devices
                 WHERE owner = 
                         (SELECT id
                         FROM accounts
                         WHERE aci = $1 
                            OR pni = $1)
                   AND devices.device_id = $2)
            "#,
            owner.name(),
            owner.device_id().to_string(),
        )
        .execute(&mut *tx)
        .await?;

        for otpk in otpks {
            if let Err(err) = sqlx::query!(
                r#"
                INSERT INTO one_time_ec_pre_key_store (owner, key_id, public_key)
                SELECT id, 
                       $3, 
                       $4
                FROM devices
                WHERE owner = 
                        (SELECT id
                         FROM accounts
                         WHERE aci = $1 
                            OR pni = $1)
                  AND devices.device_id = $2
                "#,
                owner.name(),
                owner.device_id().to_string(),
                otpk.key_id.to_string(),
                &*otpk.public_key,
            )
            .execute(&mut *tx)
            .await
            {
                bail!("{}", err);
            }
        }

        tx.commit().await?;

        Ok(())
    }

    async fn store_one_time_pq_pre_keys(
        &self,
        otpks: Vec<UploadSignedPreKey>,
        owner: &ProtocolAddress,
    ) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query!(
            r#"
            DELETE 
            FROM one_time_pq_pre_key_store
            WHERE owner = 
                (SELECT id
                 FROM devices
                 WHERE owner = 
                        (SELECT id
                         FROM accounts
                         WHERE aci = $1 
                            OR pni = $1)
                   AND devices.device_id = $2)
            "#,
            owner.name(),
            owner.device_id().to_string(),
        )
        .execute(&mut *tx)
        .await?;

        for otpk in otpks {
            if let Err(err) = sqlx::query!(
                r#"
                INSERT INTO one_time_pq_pre_key_store (owner, key_id, public_key, signature)
                SELECT id, 
                       $3, 
                       $4, 
                       $5
                FROM devices
                WHERE owner = 
                        (SELECT id
                         FROM accounts
                         WHERE aci = $1 
                            OR pni = $1)
                  AND devices.device_id = $2
                "#,
                owner.name(),
                owner.device_id().to_string(),
                otpk.key_id.to_string(),
                &*otpk.public_key,
                &*otpk.signature,
            )
            .execute(&mut *tx)
            .await
            {
                bail!("{}", err);
            }
        }

        tx.commit().await?;

        Ok(())
    }

    async fn get_one_time_ec_pre_key(
        &self,
        owner: &ProtocolAddress,
    ) -> Result<Option<UploadPreKey>> {
        sqlx::query!(
            r#"
            WITH key AS
                (DELETE 
                 FROM one_time_ec_pre_key_store
                 WHERE id IN 
                    (SELECT one_time_ec_pre_key_store.id
                     FROM one_time_ec_pre_key_store
                     INNER JOIN devices on devices.id = one_time_ec_pre_key_store.owner
                     WHERE devices.owner =
                            (SELECT id
                             FROM accounts
                             WHERE aci = $1 
                                OR pni = $1)
                       AND devices.device_id = $2
                     LIMIT 1) RETURNING key_id, 
                                        public_key)
            SELECT key_id, 
                   public_key
            FROM key
            "#,
            owner.name(),
            owner.device_id().to_string()
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| {
            Some(UploadPreKey {
                key_id: row.key_id.parse().unwrap(),
                public_key: row.public_key.into(),
            })
        })
        .or_else(|err| match err {
            sqlx::Error::RowNotFound => Ok(None), // If there is no one-time prekey
            _err => Err(_err.into()),
        })
    }

    async fn get_one_time_pq_pre_key(&self, owner: &ProtocolAddress) -> Result<UploadSignedPreKey> {
        sqlx::query!(
            r#"
            WITH key AS 
            (DELETE 
             FROM one_time_pq_pre_key_store
             WHERE id IN
                (SELECT one_time_pq_pre_key_store.id
                 FROM one_time_pq_pre_key_store
                 INNER JOIN devices on devices.id = one_time_pq_pre_key_store.owner
                 WHERE devices.owner =
                        (SELECT id
                         FROM accounts
                         WHERE aci = $1 
                            OR pni = $1)
                   AND devices.device_id = $2
                 LIMIT 1) RETURNING key_id, 
                                    public_key, 
                                    signature)
            SELECT key_id, 
                   public_key, 
                   signature
            FROM key
            "#,
            owner.name(),
            owner.device_id().to_string()
        )
        .fetch_one(&self.pool)
        .await
        .map(|row| UploadSignedPreKey {
            key_id: row.key_id.parse().unwrap(),
            public_key: row.public_key.into(),
            signature: row.signature.unwrap().into(),
        })
        .map_err(|err| err.into())
    }

    async fn count_messages(&self, address: &ProtocolAddress) -> Result<u32> {
        let result = sqlx::query!(
            r#"
            SELECT COUNT(*)
            FROM msq_queue
            INNER JOIN devices on devices.id = msq_queue.receiver
            WHERE devices.owner =
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
              AND devices.device_id = $2
            "#,
            address.name(),
            address.device_id().to_string()
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(result.count.unwrap_or_default() as u32)
    }

    async fn get_messages(&self, address: &ProtocolAddress) -> Result<Vec<Envelope>> {
        sqlx::query!(
            r#"
            SELECT msq_queue.msg
            FROM msq_queue
            INNER JOIN devices on devices.id = msq_queue.receiver
            WHERE devices.owner =
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
              AND devices.device_id = $2
            "#,
            address.name(),
            address.device_id().to_string()
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .try_fold(vec![], |mut acc, msg| -> Result<Vec<Envelope>> {
            acc.push(bincode::deserialize(&msg.msg)?);
            Ok(acc)
        })
    }

    async fn delete_messages(&self, address: &ProtocolAddress) -> Result<Vec<Envelope>> {
        sqlx::query!(
            r#"
            DELETE 
            FROM msq_queue USING devices
            WHERE devices.id = msq_queue.receiver
              AND devices.owner =
                    (SELECT id
                     FROM accounts
                     WHERE aci = $1 
                        OR pni = $1)
              AND devices.device_id = $2 RETURNING msq_queue.msg
            "#,
            address.name(),
            address.device_id().to_string()
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .try_fold(vec![], |mut acc, msg| -> Result<Vec<Envelope>> {
            acc.push(bincode::deserialize(&msg.msg)?);
            Ok(acc)
        })
    }
}

async fn store_aci_signed_pre_key(
    tx: &mut PgConnection,
    spk: &UploadSignedPreKey,
    address: &ProtocolAddress,
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO aci_signed_pre_key_store (owner, key_id, public_key, signature)
        SELECT id, 
               $3, 
               $4, 
               $5
        FROM devices
        WHERE owner =
                (SELECT id
                 FROM accounts
                 WHERE aci = $1 
                    OR pni = $1) 
          AND device_id = $2 ON CONFLICT (key_id, 
                                            owner) DO 
            
            UPDATE 
            SET key_id = $3, 
                public_key = $4, 
                signature = $5;
        "#,
        address.name(),
        address.device_id().to_string(),
        spk.key_id.to_string(),
        &*spk.public_key,
        &*spk.signature
    )
    .execute(tx)
    .await
    .map(|_| ())
    .map_err(|err| err.into())
}

async fn store_pni_signed_pre_key(
    tx: &mut PgConnection,
    spk: &UploadSignedPreKey,
    address: &ProtocolAddress,
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO pni_signed_pre_key_store (owner, key_id, public_key, signature)
        SELECT id, 
               $3, 
               $4, 
               $5
        FROM devices
        WHERE owner =
                (SELECT id
                 FROM accounts
                 WHERE aci = $1 
                    OR pni = $1) 
          AND device_id = $2 ON CONFLICT (key_id, 
                                            owner) DO

            UPDATE 
            SET key_id = $3, 
                public_key = $4, 
                signature = $5;
        "#,
        address.name(),
        address.device_id().to_string(),
        spk.key_id.to_string(),
        &*spk.public_key,
        &*spk.signature
    )
    .execute(tx)
    .await
    .map(|_| ())
    .map_err(|err| err.into())
}

async fn store_pq_aci_signed_pre_key(
    tx: &mut PgConnection,
    pq_spk: &UploadSignedPreKey,
    address: &ProtocolAddress,
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO aci_pq_last_resort_pre_key_store (owner, key_id, public_key, signature)
        SELECT id, 
               $3, 
               $4, 
               $5
        FROM devices
        WHERE owner =
                (SELECT id
                 FROM accounts
                 WHERE aci = $1 
                    OR pni = $1) 
          AND device_id = $2 ON CONFLICT (key_id, 
                                            owner) DO
            
            UPDATE 
            SET key_id = $3, 
                public_key = $4, 
                signature = $5;

        "#,
        address.name(),
        address.device_id().to_string(),
        pq_spk.key_id.to_string(),
        &*pq_spk.public_key,
        &*pq_spk.signature
    )
    .execute(tx)
    .await
    .map(|_| ())
    .map_err(|err| err.into())
}

async fn store_pq_pni_signed_pre_key(
    tx: &mut PgConnection,
    pq_spk: &UploadSignedPreKey,
    address: &ProtocolAddress,
) -> Result<()> {
    sqlx::query!(
        r#"
        INSERT INTO pni_pq_last_resort_pre_key_store (owner, key_id, public_key, signature)
        SELECT id, 
               $3, 
               $4, 
               $5
        FROM devices
        WHERE owner =
                (SELECT id
                 FROM accounts
                 WHERE aci = $1 
                    OR pni = $1) 
          AND device_id = $2 ON CONFLICT (key_id, 
                                            owner) DO
            
            UPDATE 
            SET key_id = $3, 
            public_key = $4, 
            signature = $5;

        "#,
        address.name(),
        address.device_id().to_string(),
        pq_spk.key_id.to_string(),
        &*pq_spk.public_key,
        &*pq_spk.signature
    )
    .execute(tx)
    .await
    .map(|_| ())
    .map_err(|err| err.into())
}

#[cfg(test)]
mod db_tests {
    use common::signalservice::Envelope;
    use libsignal_core::{Aci, Pni, ProtocolAddress};
    use uuid::Uuid;

    use crate::{
        database::SignalDatabase,
        test_utils::{
            database::{
                database_connect, get_aci_signed_pre_key, get_pni_signed_pre_key,
                get_pq_aci_signed_pre_key, get_pq_pni_signed_pre_key,
            },
            key::{new_device_pre_key_bundle, new_upload_pre_keys, new_upload_signed_pre_key},
            user::{new_account, new_account_and_address, new_account_and_device, new_device},
        },
    };

    #[tokio::test]
    async fn test_add_and_get_user() {
        let db = database_connect().await;
        let account = new_account();

        db.add_account(&account).await.unwrap();
        let retrieved_account = db.get_account(&account.aci().into()).await.unwrap();

        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(account, retrieved_account);
    }

    #[tokio::test]
    async fn test_update_account_aci() {
        let db = database_connect().await;
        let account = new_account();
        let new_aci = Aci::from(Uuid::new_v4());

        db.add_account(&account).await.unwrap();
        db.update_account_aci(&account.aci().into(), new_aci)
            .await
            .unwrap();
        let retrieved_account = db.get_account(&new_aci.into()).await.unwrap();
        db.delete_account(&retrieved_account.aci().into())
            .await
            .unwrap();

        assert_ne!(account.aci(), retrieved_account.aci());
        assert_eq!(retrieved_account.aci(), new_aci);
    }

    #[tokio::test]
    async fn test_update_account_pni() {
        let db = database_connect().await;
        let account = new_account();
        let new_pni = Pni::from(Uuid::new_v4());

        db.add_account(&account).await.unwrap();
        db.update_account_pni(&account.pni().into(), new_pni)
            .await
            .unwrap();
        let retrieved_account = db.get_account(&new_pni.into()).await.unwrap();
        db.delete_account(&retrieved_account.pni().into())
            .await
            .unwrap();

        assert_ne!(account.pni(), retrieved_account.pni());
        assert_eq!(retrieved_account.pni(), new_pni);
    }

    #[tokio::test]
    async fn test_delete_account() {
        let db = database_connect().await;
        let account = new_account();

        db.add_account(&account).await.unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();
        db.get_account(&account.aci().into())
            .await
            .expect_err("The account should have been deleted");
    }

    #[tokio::test]
    async fn test_add_and_get_device() {
        let db = database_connect().await;

        let account = new_account();
        db.add_account(&account).await.unwrap();

        let secondary_device = new_device();
        db.add_device(&account.aci().into(), &secondary_device)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "Should be other device_id: {}, {}",
                    account.devices()[0].device_id(),
                    secondary_device.device_id()
                )
            });

        let retrieved_device = db
            .get_device(&ProtocolAddress::new(
                account.aci().service_id_string(),
                secondary_device.device_id(),
            ))
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap(); // secondary_device is also deleted due to cascading

        assert_eq!(secondary_device, retrieved_device);
    }

    #[tokio::test]
    async fn test_get_all_devices() {
        let db = database_connect().await;
        let mut account = new_account();
        let device = new_device();

        db.add_account(&account).await.unwrap();
        db.add_device(&account.aci().into(), &device)
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "Should be other device_id: {}, {}",
                    account.devices()[0].device_id(),
                    device.device_id()
                )
            });
        account.add_device(device).unwrap();
        let retrieved_devices = db.get_all_devices(&account.aci().into()).await.unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(account.devices(), retrieved_devices);
    }

    #[tokio::test]
    async fn test_delete_device() {
        let db = database_connect().await;
        let (account, device) = new_account_and_device();

        db.add_account(&account).await.unwrap();
        db.delete_device(&ProtocolAddress::new(
            account.aci().service_id_string(),
            device.device_id(),
        ))
        .await
        .unwrap();
        db.get_device(&ProtocolAddress::new(
            account.aci().service_id_string(),
            device.device_id(),
        ))
        .await
        .expect_err("This devices should have been deleted");
        db.delete_account(&account.aci().into()).await.unwrap();
    }

    #[tokio::test]
    async fn test_push_and_pop_message_queue() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let msg = Envelope::default();

        db.add_account(&account).await.unwrap();
        db.push_message_queue(&address, vec![msg.clone()])
            .await
            .unwrap();
        let retrieved_msg = db.pop_msg_queue(&address).await.unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(retrieved_msg.len(), 1);
        assert_eq!(msg, retrieved_msg[0]);
    }

    #[tokio::test]
    async fn test_store_aci_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_aci_signed_pre_key(&key, &address).await.unwrap();
        let retrieved_key = get_aci_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_update_aci_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let mut key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_aci_signed_pre_key(&key, &address).await.unwrap();
        key.public_key = Box::new([5, 6, 7, 8]);
        db.store_aci_signed_pre_key(&key, &address).await.unwrap();
        let retrieved_key = get_aci_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_store_pni_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_pni_signed_pre_key(&key, &address).await.unwrap();
        let retrieved_key = get_pni_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_update_pni_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let mut key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_pni_signed_pre_key(&key, &address).await.unwrap();
        key.public_key = Box::new([5, 6, 7, 8]);
        db.store_pni_signed_pre_key(&key, &address).await.unwrap();
        let retrieved_key = get_pni_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_store_pq_aci_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_pq_aci_signed_pre_key(&key, &address)
            .await
            .unwrap();
        let retrieved_key = get_pq_aci_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_update_pq_aci_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let mut key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_pq_aci_signed_pre_key(&key, &address)
            .await
            .unwrap();
        key.public_key = Box::new([5, 6, 7, 8]);
        db.store_pq_aci_signed_pre_key(&key, &address)
            .await
            .unwrap();
        let retrieved_key = get_pq_aci_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_store_pq_pni_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_pq_pni_signed_pre_key(&key, &address)
            .await
            .unwrap();
        let retrieved_key = get_pq_pni_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_update_pq_pni_signed_pre_key() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let mut key = new_upload_signed_pre_key(None);

        db.add_account(&account).await.unwrap();
        db.store_pq_pni_signed_pre_key(&key, &address)
            .await
            .unwrap();
        key.public_key = Box::new([5, 6, 7, 8]);
        db.store_pq_pni_signed_pre_key(&key, &address)
            .await
            .unwrap();
        let retrieved_key = get_pq_pni_signed_pre_key(&db, key.key_id, &address)
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key, retrieved_key);
    }

    #[tokio::test]
    async fn test_store_and_get_key_bundle() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let key_bundle = new_device_pre_key_bundle();

        db.add_account(&account).await.unwrap();
        db.store_key_bundle(&key_bundle, &address).await.unwrap();
        let retrieved_key_bundle = db.get_key_bundle(&address).await.unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(key_bundle, retrieved_key_bundle);
    }

    #[tokio::test]
    async fn test_get_one_time_ec_pre_key_count() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let otpks = new_upload_pre_keys(4);

        db.add_account(&account).await.unwrap();
        db.store_one_time_ec_pre_keys(otpks.clone(), &address)
            .await
            .unwrap();
        let count = db
            .get_one_time_ec_pre_key_count(&account.aci().into())
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(count, otpks.len() as u32);
    }

    #[tokio::test]
    async fn test_get_one_time_pq_pre_key_count() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let otpks = vec![
            new_upload_signed_pre_key(None),
            new_upload_signed_pre_key(None),
            new_upload_signed_pre_key(None),
            new_upload_signed_pre_key(None),
        ];

        db.add_account(&account).await.unwrap();
        db.store_one_time_pq_pre_keys(otpks.clone(), &address)
            .await
            .unwrap();
        let count = db
            .get_one_time_pq_pre_key_count(&account.aci().into())
            .await
            .unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(count, otpks.len() as u32);
    }

    #[tokio::test]
    async fn test_store_and_get_one_time_ec_pre_keys() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let otpks = new_upload_pre_keys(1);

        db.add_account(&account).await.unwrap();
        db.store_one_time_ec_pre_keys(otpks.clone(), &address)
            .await
            .unwrap();
        let retrieved_key = db.get_one_time_ec_pre_key(&address).await.unwrap().unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(otpks, vec![retrieved_key])
    }

    #[tokio::test]
    async fn test_store_one_time_pq_pre_keys() {
        let db = database_connect().await;
        let (account, address) = new_account_and_address();
        let otpks = vec![new_upload_signed_pre_key(None)];

        db.add_account(&account).await.unwrap();
        db.store_one_time_pq_pre_keys(otpks.clone(), &address)
            .await
            .unwrap();
        let retrieved_key = db.get_one_time_pq_pre_key(&address).await.unwrap();
        db.delete_account(&account.aci().into()).await.unwrap();

        assert_eq!(otpks, vec![retrieved_key])
    }
}
