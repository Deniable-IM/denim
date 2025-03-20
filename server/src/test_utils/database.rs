use crate::storage::postgres::PostgresDatabase;
use anyhow::Result;
use common::web_api::UploadSignedPreKey;
use libsignal_core::ProtocolAddress;

pub async fn database_connect() -> PostgresDatabase {
    PostgresDatabase::connect("DATABASE_URL_TEST".to_string()).await
}

#[allow(dead_code)]
pub async fn get_ec_pni_signed_pre_key(
    db: &PostgresDatabase,
    key_id: u32,
    address: &ProtocolAddress,
) -> Result<UploadSignedPreKey> {
    sqlx::query!(
        r#"
            SELECT key_id, 
                   public_key, 
                   signature
            FROM pni_signed_pre_key_store
            WHERE key_id = $1 
              AND owner =
                    (SELECT id
                     FROM devices
                     WHERE owner =
                            (SELECT id
                             FROM accounts
                             WHERE aci = $2 
                                OR pni = $2) 
                       AND device_id = $3)
            "#,
        key_id.to_string(),
        address.name(),
        address.device_id().to_string()
    )
    .fetch_one(db.pool())
    .await
    .map(|row| UploadSignedPreKey {
        key_id: row.key_id.parse().unwrap(),
        public_key: row.public_key.into(),
        signature: row.signature.into(),
    })
    .map_err(|err| err.into())
}

pub async fn get_aci_signed_pre_key(
    db: &PostgresDatabase,
    key_id: u32,
    address: &ProtocolAddress,
) -> Result<UploadSignedPreKey> {
    sqlx::query!(
        r#"
            SELECT key_id, 
                   public_key, 
                   signature
            FROM aci_signed_pre_key_store
            WHERE key_id = $1 
              AND owner =
                    (SELECT id
                     FROM devices
                     WHERE owner =
                            (SELECT id
                            FROM accounts
                            WHERE aci = $2 
                               OR pni = $2) 
                       AND device_id = $3)
            "#,
        key_id.to_string(),
        address.name(),
        address.device_id().to_string()
    )
    .fetch_one(db.pool())
    .await
    .map(|row| UploadSignedPreKey {
        key_id: row.key_id.parse().unwrap(),
        public_key: row.public_key.into(),
        signature: row.signature.into(),
    })
    .map_err(|err| err.into())
}

pub async fn get_pni_signed_pre_key(
    db: &PostgresDatabase,
    key_id: u32,
    address: &ProtocolAddress,
) -> Result<UploadSignedPreKey> {
    sqlx::query!(
        r#"
            SELECT key_id, 
                   public_key, 
                   signature
            FROM pni_signed_pre_key_store
            WHERE key_id = $1 
              AND owner =
                    (SELECT id
                     FROM devices
                     WHERE owner =
                            (SELECT id
                             FROM accounts
                             WHERE aci = $2 
                                OR pni = $2) 
                       AND device_id = $3)
            "#,
        key_id.to_string(),
        address.name(),
        address.device_id().to_string()
    )
    .fetch_one(db.pool())
    .await
    .map(|row| UploadSignedPreKey {
        key_id: row.key_id.parse().unwrap(),
        public_key: row.public_key.into(),
        signature: row.signature.into(),
    })
    .map_err(|err| err.into())
}

pub async fn get_pq_aci_signed_pre_key(
    db: &PostgresDatabase,
    key_id: u32,
    address: &ProtocolAddress,
) -> Result<UploadSignedPreKey> {
    sqlx::query!(
        r#"
            SELECT key_id, 
                   public_key, 
                   signature
            FROM aci_pq_last_resort_pre_key_store
            WHERE key_id = $1 
              AND owner =
                    (SELECT id
                     FROM devices
                     WHERE owner =
                            (SELECT id
                             FROM accounts
                             WHERE aci = $2 
                                OR pni = $2) 
                       AND device_id = $3)
            "#,
        key_id.to_string(),
        address.name(),
        address.device_id().to_string()
    )
    .fetch_one(db.pool())
    .await
    .map(|row| UploadSignedPreKey {
        key_id: row.key_id.parse().unwrap(),
        public_key: row.public_key.into(),
        signature: row.signature.into(),
    })
    .map_err(|err| err.into())
}
pub async fn get_pq_pni_signed_pre_key(
    db: &PostgresDatabase,
    key_id: u32,
    address: &ProtocolAddress,
) -> Result<UploadSignedPreKey> {
    sqlx::query!(
        r#"
            SELECT key_id, 
                   public_key, 
                   signature
            FROM pni_pq_last_resort_pre_key_store
            WHERE key_id = $1 
              AND owner =
                    (SELECT id
                     FROM devices
                     WHERE owner =
                            (SELECT id
                             FROM accounts
                             WHERE aci = $2
                                OR pni = $2) 
                       AND device_id = $3)
            "#,
        key_id.to_string(),
        address.name(),
        address.device_id().to_string()
    )
    .fetch_one(db.pool())
    .await
    .map(|row| UploadSignedPreKey {
        key_id: row.key_id.parse().unwrap(),
        public_key: row.public_key.into(),
        signature: row.signature.into(),
    })
    .map_err(|err| err.into())
}

#[allow(dead_code)]
pub async fn get_pq_last_resort_pre_key(
    db: &PostgresDatabase,
    key_id: u32,
    address: &ProtocolAddress,
) -> Result<UploadSignedPreKey> {
    sqlx::query!(
        r#"
            SELECT key_id, 
                   public_key, 
                   signature
            FROM pni_pq_last_resort_pre_key_store
            WHERE key_id = $1 
              AND owner =
                    (SELECT id
                     FROM devices
                     WHERE owner =
                            (SELECT id
                             FROM accounts
                             WHERE aci = $2 
                                OR pni = $2) 
                       AND device_id = $3)
            "#,
        key_id.to_string(),
        address.name(),
        address.device_id().to_string()
    )
    .fetch_one(db.pool())
    .await
    .map(|row| UploadSignedPreKey {
        key_id: row.key_id.parse().unwrap(),
        public_key: row.public_key.into(),
        signature: row.signature.into(),
    })
    .map_err(|err| err.into())
}

pub async fn get_aci_pq_last_resort_pre_key(
    db: &PostgresDatabase,
    key_id: u32,
    address: &ProtocolAddress,
) -> Result<UploadSignedPreKey> {
    sqlx::query!(
        r#"
            SELECT key_id,
                   public_key,
                   signature
            FROM aci_pq_last_resort_pre_key_store
            WHERE key_id = $1
              AND owner =
                    (SELECT id
                     FROM devices
                     WHERE owner =
                            (SELECT id
                             FROM accounts
                             WHERE aci = $2
                                OR pni = $2)
                       AND device_id = $3)
            "#,
        key_id.to_string(),
        address.name(),
        address.device_id().to_string()
    )
    .fetch_one(db.pool())
    .await
    .map(|row| UploadSignedPreKey {
        key_id: row.key_id.parse().unwrap(),
        public_key: row.public_key.into(),
        signature: row.signature.into(),
    })
    .map_err(|err| err.into())
}
