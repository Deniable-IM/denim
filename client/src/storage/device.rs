use std::collections::HashSet;

use super::database::ClientDB;
use crate::contact_manager::{Contact, ContactName};
use axum::async_trait;
use base64::{prelude::BASE64_STANDARD, Engine as _};
use common::web_api::DenimChunk;
use libsignal_core::{Aci, DeviceId, Pni, ProtocolAddress, ServiceId};
use libsignal_protocol::{
    Direction, GenericSignedPreKey as _, IdentityKey, IdentityKeyPair, KyberPreKeyId,
    KyberPreKeyRecord, PreKeyId, PreKeyRecord, PrivateKey, SenderKeyRecord, SessionRecord,
    SignalProtocolError, SignedPreKeyId, SignedPreKeyRecord,
};
use rusqlite::{params, Connection, OptionalExtension};
use uuid::Uuid;

#[derive(Debug)]
pub struct Device {
    conn: Connection,
}

impl Device {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    async fn insert_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<(), SignalProtocolError> {
        let addr = format!("{}", address);
        let key = BASE64_STANDARD.encode(identity.serialize());

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeviceIdentityKeyStore (address, identity_key)
            VALUES (?1, ?2)
            ON CONFLICT(address) DO UPDATE SET identity_key = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![addr, key, key])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }
}

#[async_trait(?Send)]
impl ClientDB for Device {
    type Error = SignalProtocolError;

    async fn insert_account_information(
        &self,
        aci: Aci,
        pni: Pni,
        password: String,
    ) -> Result<(), Self::Error> {
        let aci = aci.service_id_string();
        let pni = pni.service_id_string();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO Identity (aci, pni, password)
            VALUES (?1, ?2, ?3)
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![aci, pni, password])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn insert_account_key_information(
        &self,
        key_pair: IdentityKeyPair,
        registration_id: u32,
    ) -> Result<(), Self::Error> {
        let pk = BASE64_STANDARD.encode(key_pair.identity_key().serialize());
        let sk = BASE64_STANDARD.encode(key_pair.private_key().serialize());

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO IdentityKeys (public_key, private_key, registration_id)
            VALUES (?1, ?2, ?3)
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![pk, sk, registration_id])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_key_ids(&self) -> Result<(u32, u32, u32), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            WITH max_pre_key_id_table AS (
                SELECT
                    1 AS _id,
                    MAX(pre_key_id) AS max_pre_key_id
                FROM
                    DevicePreKeyStore
            ), max_signed_pre_key_id_table AS (
                SELECT
                    1 AS _id,
                    MAX(signed_pre_key_id) AS max_signed_pre_key_id
                FROM
                    DeviceSignedPreKeyStore
            ), max_kyber_pre_key_id_table AS (
                SELECT
                    1 AS _id,
                    MAX(kyber_pre_key_id) AS max_kyber_pre_key_id
                FROM
                    DeviceKyberPreKeyStore
            )
            SELECT
                CASE WHEN mpk.max_pre_key_id IS NOT NULL
                    THEN mpk.max_pre_key_id
                ELSE
                    0
                END AS mpkid,
                CASE WHEN spk.max_signed_pre_key_id IS NOT NULL
                    THEN spk.max_signed_pre_key_id
                ELSE
                    0
                END AS spkid,
                CASE WHEN kpk.max_kyber_pre_key_id IS NOT NULL
                    THEN kpk.max_kyber_pre_key_id
                ELSE
                    0
                END AS kpkid
            FROM
                max_pre_key_id_table mpk
                INNER JOIN max_signed_pre_key_id_table spk ON spk._id = mpk._id
                INNER JOIN max_kyber_pre_key_id_table kpk ON kpk._id = mpk._id
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: (u32, u32, u32) = stmt
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(row)
    }

    async fn store_contact(&self, contact: &Contact) -> Result<(), Self::Error> {
        let service_id = contact.service_id.service_id_string();
        let device_ids = contact
            .device_ids
            .iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(",");

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO Contacts(service_id, device_ids)
            VALUES(?1, ?2)
            ON CONFLICT(service_id) DO UPDATE SET device_ids = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![service_id, device_ids, device_ids])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn load_contacts(&self) -> Result<Vec<Contact>, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                service_id,
                device_ids
            FROM
                Contacts
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        let mut rows = stmt
            .query([])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let mut contacts = vec![];
        while let Some(row) = rows
            .next()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?
        {
            let mut device_ids = HashSet::new();
            let row_service_id: String = row
                .get(0)
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
            let row_device_ids: String = row
                .get(1)
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
            if row_device_ids != "" {
                for device_id in row_device_ids.split(",") {
                    device_ids.insert(DeviceId::from(device_id.parse::<u32>().map_err(|err| {
                        SignalProtocolError::InvalidArgument(format!(
                            "Could not parse device id: {err}"
                        ))
                    })?));
                }
            }
            contacts.push(Contact {
                service_id: ServiceId::parse_from_service_id_string(row_service_id.as_str())
                    .ok_or(SignalProtocolError::InvalidArgument(format!(
                        "Could not parse service_id: {}",
                        row_service_id
                    )))?,
                device_ids,
            });
        }

        Ok(contacts)
    }

    async fn remove_contact(&self, service_id: &ServiceId) -> Result<(), Self::Error> {
        let name = service_id.service_id_string();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            DELETE FROM
                Contacts
            WHERE
                service_id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![name])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_all_nicknames(&self) -> Result<Vec<ContactName>, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                name,
                service_id
            FROM
                Nicknames
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let rows = stmt
            .query_map([], |row| {
                Ok(ContactName {
                    name: row.get(0)?,
                    service_id: ServiceId::parse_from_service_id_string(
                        row.get::<_, String>(1)?.as_str(),
                    )
                    .expect("Should service id"),
                })
            })
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let mut contact_names = Vec::new();
        for contact_name in rows {
            contact_names.push(
                contact_name
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?,
            );
        }

        Ok(contact_names)
    }

    async fn insert_service_id_for_nickname(
        &self,
        nickname: &str,
        service_id: &ServiceId,
    ) -> Result<(), Self::Error> {
        let service_id = service_id.service_id_string();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO Nicknames(name, service_id)
            VALUES(?1, ?2)
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![nickname, service_id])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_service_id_by_nickname(&self, nickname: &str) -> Result<ServiceId, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                service_id
            FROM
                Nicknames
            WHERE
                name = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: String = stmt
            .query_row([nickname], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        ServiceId::parse_from_service_id_string(&row).ok_or(SignalProtocolError::InvalidArgument(
            format!("Could not parse service_id"),
        ))
    }

    async fn get_identity_key_pair(&self) -> Result<IdentityKeyPair, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                public_key, private_key
            FROM
                IdentityKeys 
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: (String, String) = stmt
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(IdentityKeyPair::new(
            IdentityKey::decode(
                &BASE64_STANDARD
                    .decode(row.0)
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))?,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))?,
            PrivateKey::deserialize(
                &BASE64_STANDARD
                    .decode(row.1)
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))?,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))?,
        ))
    }

    async fn get_local_registration_id(&self) -> Result<u32, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                registration_id
            FROM
                IdentityKeys
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: u32 = stmt
            .query_row([], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(row)
    }
    async fn save_identity(
        &mut self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
    ) -> Result<bool, Self::Error> {
        match self
            .get_identity(address)
            .await
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))?
        {
            Some(key) if key == *identity => Ok(false),
            Some(_key) => {
                self.insert_identity(address, identity).await?;
                Ok(false)
            }
            None => {
                self.insert_identity(address, identity).await?;
                Ok(true)
            }
        }
    }

    async fn is_trusted_identity(
        &self,
        address: &ProtocolAddress,
        identity: &IdentityKey,
        _direction: Direction,
    ) -> Result<bool, Self::Error> {
        match self
            .get_identity(address)
            .await
            .expect("This function cannot return err")
        {
            Some(i) => Ok(i == *identity),
            None => Ok(true),
        }
    }

    async fn get_identity(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<IdentityKey>, Self::Error> {
        let addr = format!("{}", address);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                identity_key
            FROM
                DeviceIdentityKeyStore
            WHERE
                address = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: Option<String> = stmt
            .query_row([addr], |row| Ok(row.get(0)?))
            .optional()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        match row {
            Some(identity_key) => Ok(Some(
                BASE64_STANDARD
                    .decode(identity_key)
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))?
                    .as_slice()
                    .try_into()?,
            )),
            None => Ok(None),
        }
    }

    async fn get_pre_key(&self, prekey_id: PreKeyId) -> Result<PreKeyRecord, Self::Error> {
        let id: u32 = prekey_id.into();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                pre_key_record
            FROM
                DevicePreKeyStore
            WHERE
                pre_key_id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: String = stmt
            .query_row([id], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        PreKeyRecord::deserialize(
            BASE64_STANDARD
                .decode(row)
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{err}")))?
                .as_slice(),
        )
        .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))
    }

    async fn save_pre_key(
        &mut self,
        prekey_id: PreKeyId,
        record: &PreKeyRecord,
    ) -> Result<(), Self::Error> {
        let id: u32 = prekey_id.into();
        let rec = BASE64_STANDARD.encode(record.serialize()?);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DevicePreKeyStore (pre_key_id, pre_key_record)
            VALUES (?1, ?2)
            ON CONFLICT(pre_key_id) DO UPDATE SET pre_key_record = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![id, rec, rec])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn remove_pre_key(&mut self, prekey_id: PreKeyId) -> Result<(), Self::Error> {
        let id: u32 = prekey_id.into();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            DELETE FROM
                DevicePreKeyStore
            WHERE
                pre_key_id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![id])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_signed_pre_key(
        &self,
        id: SignedPreKeyId,
    ) -> Result<SignedPreKeyRecord, Self::Error> {
        let sid: u32 = id.into();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                signed_pre_key_record
            FROM
                DeviceSignedPreKeyStore
            WHERE
                signed_pre_key_id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: String = stmt
            .query_row([sid], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        SignedPreKeyRecord::deserialize(
            BASE64_STANDARD
                .decode(row)
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?
                .as_slice(),
        )
        .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))
    }

    async fn save_signed_pre_key(
        &mut self,
        id: SignedPreKeyId,
        record: &SignedPreKeyRecord,
    ) -> Result<(), Self::Error> {
        let id: u32 = id.into();
        let rec = BASE64_STANDARD.encode(record.serialize()?);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeviceSignedPreKeyStore (signed_pre_key_id, signed_pre_key_record)
            VALUES (?1, ?2)
            ON CONFLICT(signed_pre_key_id) DO UPDATE SET signed_pre_key_record = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![id, rec, rec])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_kyber_pre_key(
        &self,
        kyber_prekey_id: KyberPreKeyId,
    ) -> Result<KyberPreKeyRecord, Self::Error> {
        let id: u32 = kyber_prekey_id.into();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                kyber_pre_key_record
            FROM
                DeviceKyberPreKeyStore
            WHERE
                kyber_pre_key_id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: String = stmt
            .query_row([id], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        KyberPreKeyRecord::deserialize(
            BASE64_STANDARD
                .decode(row)
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?
                .as_slice(),
        )
    }

    async fn save_kyber_pre_key(
        &mut self,
        kyber_prekey_id: KyberPreKeyId,
        record: &KyberPreKeyRecord,
    ) -> Result<(), Self::Error> {
        let id: u32 = kyber_prekey_id.into();
        let rec = BASE64_STANDARD.encode(record.serialize()?);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeviceKyberPreKeyStore (kyber_pre_key_id, kyber_pre_key_record)
            VALUES (?1, ?2)
            ON CONFLICT(kyber_pre_key_id) DO UPDATE SET kyber_pre_key_record = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![id, rec, rec])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn load_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, Self::Error> {
        let addr = format!("{}", address);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                session_record
            FROM
                DeviceSessionStore
            WHERE
                address = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: Option<String> = stmt
            .query_row([addr], |row| Ok(row.get(0)?))
            .optional()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        match row {
            Some(session_record) => SessionRecord::deserialize(
                BASE64_STANDARD
                    .decode(session_record)
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?
                    .as_slice(),
            )
            .map(Some),
            None => Ok(None),
        }
    }

    async fn load_deniable_session(
        &self,
        address: &ProtocolAddress,
    ) -> Result<Option<SessionRecord>, Self::Error> {
        let addr = format!("{}", address);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                session_record
            FROM
                DeniableDeviceSessionStore
            WHERE
                address = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: Option<String> = stmt
            .query_row([addr], |row| Ok(row.get(0)?))
            .optional()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        match row {
            Some(session_record) => SessionRecord::deserialize(
                BASE64_STANDARD
                    .decode(session_record)
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?
                    .as_slice(),
            )
            .map(Some),
            None => Ok(None),
        }
    }

    async fn store_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), Self::Error> {
        let addr = format!("{}", address);
        let rec = BASE64_STANDARD.encode(record.serialize()?);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeviceSessionStore (address, session_record)
            VALUES (?1, ?2)
            ON CONFLICT(address) DO UPDATE SET session_record = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![addr, rec, rec])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn store_deniable_session(
        &mut self,
        address: &ProtocolAddress,
        record: &SessionRecord,
    ) -> Result<(), Self::Error> {
        let addr = format!("{}", address);
        let rec = BASE64_STANDARD.encode(record.serialize()?);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeniableDeviceSessionStore (address, session_record)
            VALUES (?1, ?2)
            ON CONFLICT(address) DO UPDATE SET session_record = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![addr, rec, rec])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn store_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
        record: &SenderKeyRecord,
    ) -> Result<(), Self::Error> {
        let addr = format!("{}:{}", sender, distribution_id);
        let rec = BASE64_STANDARD.encode(record.serialize()?);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeviceSenderKeyStore (address, sender_key_record)
            VALUES (?1, ?2)
            ON CONFLICT(address) DO UPDATE SET sender_key_record = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![addr, rec, rec])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn load_sender_key(
        &mut self,
        sender: &ProtocolAddress,
        distribution_id: Uuid,
    ) -> Result<Option<SenderKeyRecord>, Self::Error> {
        let addr = format!("{}:{}", sender, distribution_id);

        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                sender_key_record
            FROM
                DeviceSenderKeyStore
            WHERE
                address = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: Option<String> = stmt
            .query_row([addr], |row| Ok(row.get(0)?))
            .optional()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        match row {
            Some(sender_key_record) => SenderKeyRecord::deserialize(
                BASE64_STANDARD
                    .decode(sender_key_record)
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?
                    .as_slice(),
            )
            .map(Some),
            None => Ok(None),
        }
    }

    async fn set_password(&mut self, new_password: String) -> Result<(), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            UPDATE Identity
            SET password = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![new_password])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_password(&self) -> Result<String, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                password
            FROM
                Identity
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: String = stmt
            .query_row([], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(row)
    }

    async fn set_aci(&mut self, new_aci: Aci) -> Result<(), Self::Error> {
        let new_aci = new_aci.service_id_string();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            UPDATE Identity
            SET aci = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![new_aci])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_aci(&self) -> Result<Aci, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                aci
            FROM
                Identity
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: String = stmt
            .query_row([], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(Aci::parse_from_service_id_string(row.as_str()).ok_or(
            SignalProtocolError::InvalidArgument(format!("Could not convert {} to aci", row)),
        )?)
    }

    async fn set_pni(&mut self, new_pni: Pni) -> Result<(), Self::Error> {
        let new_pni = new_pni.service_id_string();

        let mut stmt = self
            .conn
            .prepare(
                r#"
            UPDATE Identity
            SET pni = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![new_pni])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn get_pni(&self) -> Result<Pni, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                pni
            FROM
                Identity
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: String = stmt
            .query_row([], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(Pni::parse_from_service_id_string(row.as_str()).ok_or(
            SignalProtocolError::InvalidArgument(format!("Could not convert {} to pni", row)),
        )?)
    }

    async fn get_deniable_payload(&self) -> Result<(u32, Vec<u8>, i32), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                id, content, chunk_count
            FROM
                DeniablePayload
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: (u32, Vec<u8>, i32) = stmt
            .query_row([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        Ok(row)
    }

    async fn get_deniable_payload_by_id(
        &self,
        payload_id: u32,
    ) -> Result<(Vec<u8>, i32), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                content, chunk_count
            FROM
                DeniablePayload
            WHERE
                id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: (Vec<u8>, i32) = stmt
            .query_row([payload_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        Ok(row)
    }

    async fn store_deniable_payload(
        &self,
        payload_id: Option<u32>,
        chunk_count: i32,
        payload: Vec<u8>,
    ) -> Result<(), Self::Error> {
        if let Some(id) = payload_id {
            let mut stmt = self
                .conn
                .prepare(
                    r#"
                UPDATE DeniablePayload
                SET content = ?1, chunk_count = ?2
                WHERE id = ?3
                "#,
                )
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

            stmt.execute(params![payload, chunk_count, id])
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        } else {
            let mut stmt = self
                .conn
                .prepare(
                    r#"
                INSERT INTO DeniablePayload (content, chunk_count)
                VALUES (?1, 0)
                "#,
                )
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

            stmt.execute(params![payload])
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        }
        Ok(())
    }

    async fn remove_deniable_payload(&self, payload_id: u32) -> Result<(), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            DELETE FROM
                DeniablePayload
            WHERE
                id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![payload_id])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        Ok(())
    }

    async fn try_get_key_request_sent(
        &self,
        service_id: String,
    ) -> Result<Option<String>, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                alias
            FROM
                DeniableKeyRequestsSent
            WHERE
                service_id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let row: Option<String> = stmt
            .query_row([service_id], |row| Ok(row.get(0)?))
            .optional()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        Ok(row)
    }

    async fn store_key_request_sent(
        &self,
        service_id: String,
        alias: String,
    ) -> Result<(), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeniableKeyRequestsSent (service_id, alias)
            VALUES (?1, ?2)
            ON CONFLICT(service_id) DO UPDATE SET alias = ?3
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![service_id, alias, alias])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }

    async fn remove_key_request_sent(&self, service_id: String) -> Result<(), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            DELETE FROM
                DeniableKeyRequestsSent
            WHERE
                service_id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![service_id])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        Ok(())
    }

    async fn get_messages_awaiting_encryption(
        &self,
        alias: String,
    ) -> Result<Vec<String>, Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            SELECT
                message
            FROM
                DeniableMessageAwaitingEncryption
            WHERE
                alias = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let rows = stmt
            .query_map([alias], |row| Ok(row.get(0)?))
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        let mut messages = Vec::new();
        for message in rows {
            messages.push(
                message.map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?,
            );
        }

        Ok(messages)
    }

    async fn store_message_awaiting_encryption(
        &self,
        message: String,
        alias: String,
    ) -> Result<(), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            INSERT INTO DeniableMessageAwaitingEncryption (message, alias)
            VALUES (?1, ?2)
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![message, alias])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        Ok(())
    }

    async fn remove_message_awaiting_encryption(&self, message_id: u32) -> Result<(), Self::Error> {
        let mut stmt = self
            .conn
            .prepare(
                r#"
            DELETE FROM
                DeniableMessageAwaitingEncryption
            WHERE
                id = ?1
            "#,
            )
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        stmt.execute(params![message_id])
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        Ok(())
    }

    async fn get_and_remove_incoming_deniable_chunks(
        &mut self,
    ) -> Result<Vec<DenimChunk>, Self::Error> {
        let mut chunks = Vec::new();
        let tx = self
            .conn
            .transaction()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        {
            let mut stmt = tx
                .prepare(
                    r#"
                SELECT
                    id, chunk, flags
                FROM
                    IncomingDeniableChunk
                "#,
                )
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

            let mut delete_stmt = tx
                .prepare(
                    r#"
                DELETE FROM
                    IncomingDeniableChunk
                WHERE
                    id = ?1
                "#,
                )
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
            for row in rows {
                let chunk: (u32, Vec<u8>, i32) =
                    row.map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
                chunks.push(DenimChunk {
                    chunk: chunk.1,
                    flags: chunk.2,
                });

                delete_stmt
                    .execute(params![chunk.0])
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
            }
        }
        tx.commit()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(chunks)
    }

    async fn store_incoming_deniable_chunks(
        &mut self,
        chunks: Vec<DenimChunk>,
    ) -> Result<(), Self::Error> {
        let tx = self
            .conn
            .transaction()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
        {
            let mut stmt = tx
                .prepare(
                    r#"
                INSERT INTO IncomingDeniableChunk (chunk, flags)
                VALUES (?1, ?2)
                "#,
                )
                .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

            for chunk in chunks {
                stmt.execute(params![chunk.chunk, chunk.flags])
                    .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;
            }
        }
        tx.commit()
            .map_err(|err| SignalProtocolError::InvalidArgument(format!("{}", err)))?;

        Ok(())
    }
}

#[cfg(test)]
mod device_protocol_test {
    use crate::{
        key_manager::KeyManager,
        storage::{
            database::{
                ClientDB, DeviceIdentityKeyStore, DeviceKyberPreKeyStore, DevicePreKeyStore,
                DeviceSessionStore, DeviceSignedPreKeyStore,
            },
            device::Device,
        },
        test_utils::user::{new_contact, new_protocol_address, new_rand_number, new_service_id},
    };
    use async_std::sync::Mutex;
    use include_dir::{include_dir, Dir};
    use libsignal_protocol::{
        Direction, GenericSignedPreKey, IdentityKeyPair, IdentityKeyStore, KyberPreKeyStore,
        PreKeyStore, SessionRecord, SessionStore, SignedPreKeyStore,
    };
    use rand::rngs::OsRng;
    use rusqlite::Connection;
    use rusqlite_migration::Migrations;
    use std::{
        collections::HashMap,
        sync::{Arc, LazyLock},
    };

    static MIGRATIONS_DIR: Dir = include_dir!("$CARGO_MANIFEST_DIR/client_db/migrations");
    static MIGRATIONS: LazyLock<Migrations<'static>> =
        LazyLock::new(|| Migrations::from_directory(&MIGRATIONS_DIR).unwrap());

    async fn connect() -> Connection {
        let mut conn = Connection::open_in_memory().expect("Could not open database");

        MIGRATIONS
            .to_latest(&mut conn)
            .expect("Could not run migrations");

        conn
    }

    #[tokio::test]
    async fn save_and_get_identity_test() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));
        let mut device_identity_key_store = DeviceIdentityKeyStore::new(device);
        let address = new_protocol_address();
        let other_key_pair = IdentityKeyPair::generate(&mut OsRng);
        let new_other_key_pair = IdentityKeyPair::generate(&mut OsRng);

        // Test no identity exists
        assert_eq!(
            device_identity_key_store
                .get_identity(&address)
                .await
                .unwrap(),
            None
        );

        // Test that a new identity have been added
        assert!(device_identity_key_store
            .save_identity(&address, other_key_pair.identity_key())
            .await
            .unwrap());

        assert_eq!(
            device_identity_key_store
                .get_identity(&address)
                .await
                .unwrap()
                .unwrap(),
            *other_key_pair.identity_key()
        );

        // Test we did not overwrite our identity
        assert!(!device_identity_key_store
            .save_identity(&address, other_key_pair.identity_key())
            .await
            .unwrap());

        assert_eq!(
            device_identity_key_store
                .get_identity(&address)
                .await
                .unwrap()
                .unwrap(),
            *other_key_pair.identity_key()
        );

        // Test we overwrite our identity
        assert!(!device_identity_key_store
            .save_identity(&address, new_other_key_pair.identity_key())
            .await
            .unwrap());

        assert_eq!(
            device_identity_key_store
                .get_identity(&address)
                .await
                .unwrap()
                .unwrap(),
            *new_other_key_pair.identity_key()
        );
    }
    #[tokio::test]
    async fn is_trusted_identity_test() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));
        let mut device_identity_key_store = DeviceIdentityKeyStore::new(device);
        let address = new_protocol_address();
        let other_key_pair = IdentityKeyPair::generate(&mut OsRng);

        let random_address = new_protocol_address();
        let random_key_pair = IdentityKeyPair::generate(&mut OsRng);

        // First use
        assert!(device_identity_key_store
            .is_trusted_identity(&address, other_key_pair.identity_key(), Direction::Sending)
            .await
            .unwrap());

        // Added identity
        device_identity_key_store
            .save_identity(&address, other_key_pair.identity_key())
            .await
            .unwrap();

        assert!(device_identity_key_store
            .is_trusted_identity(&address, other_key_pair.identity_key(), Direction::Sending)
            .await
            .unwrap());

        // Not trusted
        device_identity_key_store
            .save_identity(&random_address, random_key_pair.identity_key())
            .await
            .unwrap();

        assert!(!device_identity_key_store
            .is_trusted_identity(&address, random_key_pair.identity_key(), Direction::Sending)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn save_and_get_pre_key_test() {
        let mut key_man = KeyManager::default();
        let device = Arc::new(Mutex::new(Device::new(connect().await)));
        let mut device_pre_key_store = DevicePreKeyStore::new(device);
        let pre_key_record = key_man
            .generate_pre_key(&mut device_pre_key_store, &mut OsRng)
            .await
            .unwrap();

        device_pre_key_store
            .save_pre_key(pre_key_record.id().unwrap(), &pre_key_record)
            .await
            .unwrap();

        let retrived_pre_key = device_pre_key_store
            .get_pre_key(pre_key_record.id().unwrap())
            .await
            .unwrap();

        assert_eq!(retrived_pre_key.id().unwrap(), pre_key_record.id().unwrap());

        assert_eq!(
            retrived_pre_key.public_key().unwrap(),
            pre_key_record.key_pair().unwrap().public_key
        );

        assert_eq!(
            retrived_pre_key.private_key().unwrap().serialize(),
            pre_key_record.key_pair().unwrap().private_key.serialize()
        );
    }
    #[tokio::test]
    async fn remove_pre_key_test() {
        let mut key_man = KeyManager::default();
        let device = Arc::new(Mutex::new(Device::new(connect().await)));
        let mut device_pre_key_store = DevicePreKeyStore::new(device);
        let pre_key_record = key_man
            .generate_pre_key(&mut device_pre_key_store, &mut OsRng)
            .await
            .unwrap();

        device_pre_key_store
            .save_pre_key(pre_key_record.id().unwrap(), &pre_key_record)
            .await
            .unwrap();

        let _ = device_pre_key_store
            .get_pre_key(pre_key_record.id().unwrap())
            .await
            .unwrap();

        device_pre_key_store
            .remove_pre_key(pre_key_record.id().unwrap())
            .await
            .unwrap();

        device_pre_key_store
            .get_pre_key(pre_key_record.id().unwrap())
            .await
            .expect_err("We should not be able to retrive the key after deletion");
    }

    #[tokio::test]
    async fn get_and_save_signed_pre_key_test() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));
        device
            .lock()
            .await
            .insert_account_key_information(
                IdentityKeyPair::generate(&mut OsRng),
                new_rand_number(),
            )
            .await
            .unwrap();

        let mut key_man = KeyManager::default();
        let mut device_identity_key_store = DeviceIdentityKeyStore::new(device.clone());
        let mut device_signed_pre_key_store = DeviceSignedPreKeyStore::new(device);
        let signed_pre_key_record = key_man
            .generate_signed_pre_key(
                &mut device_identity_key_store,
                &mut device_signed_pre_key_store,
                &mut OsRng,
            )
            .await
            .unwrap();
        device_signed_pre_key_store
            .save_signed_pre_key(signed_pre_key_record.id().unwrap(), &signed_pre_key_record)
            .await
            .unwrap();

        let retrived_record = device_signed_pre_key_store
            .get_signed_pre_key(signed_pre_key_record.id().unwrap())
            .await
            .unwrap();

        assert_eq!(
            retrived_record.id().unwrap(),
            signed_pre_key_record.id().unwrap()
        );
        assert_eq!(
            retrived_record.public_key().unwrap(),
            signed_pre_key_record.key_pair().unwrap().public_key
        );
        assert_eq!(
            retrived_record.private_key().unwrap().serialize(),
            signed_pre_key_record
                .key_pair()
                .unwrap()
                .private_key
                .serialize()
        );
    }

    #[tokio::test]
    async fn get_and_save_kyber_pre_key_test() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));

        device
            .lock()
            .await
            .insert_account_key_information(
                IdentityKeyPair::generate(&mut OsRng),
                new_rand_number(),
            )
            .await
            .unwrap();

        let mut key_man = KeyManager::default();
        let mut device_identity_key_store = DeviceIdentityKeyStore::new(device.clone());
        let mut device_kyber_pre_key_store = DeviceKyberPreKeyStore::new(device);
        let kyber_pre_key_record = key_man
            .generate_kyber_pre_key(
                &mut device_identity_key_store,
                &mut device_kyber_pre_key_store,
            )
            .await
            .unwrap();

        device_kyber_pre_key_store
            .save_kyber_pre_key(kyber_pre_key_record.id().unwrap(), &kyber_pre_key_record)
            .await
            .unwrap();

        let retrived_record = device_kyber_pre_key_store
            .get_kyber_pre_key(kyber_pre_key_record.id().unwrap())
            .await
            .unwrap();

        assert_eq!(
            retrived_record.id().unwrap(),
            kyber_pre_key_record.id().unwrap()
        );

        assert_eq!(
            retrived_record.public_key().unwrap().serialize(),
            kyber_pre_key_record
                .key_pair()
                .unwrap()
                .public_key
                .serialize()
        );

        assert_eq!(
            retrived_record.secret_key().unwrap().serialize(),
            kyber_pre_key_record
                .key_pair()
                .unwrap()
                .secret_key
                .serialize()
        );
    }

    #[tokio::test]
    async fn load_and_store_session_test() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));
        let mut device_session_store = DeviceSessionStore::new(device);
        let address = new_protocol_address();
        let record = SessionRecord::new_fresh();

        // Not stored yet
        assert!(device_session_store
            .load_session(&address)
            .await
            .unwrap()
            .is_none());

        // Stored
        device_session_store
            .store_session(&address, &record)
            .await
            .unwrap();

        assert_eq!(
            device_session_store
                .load_session(&address)
                .await
                .unwrap()
                .unwrap()
                .serialize()
                .unwrap(),
            record.serialize().unwrap()
        );
    }

    #[tokio::test]
    async fn insert_and_get_key_ids() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));

        device
            .lock()
            .await
            .insert_account_key_information(
                IdentityKeyPair::generate(&mut OsRng),
                new_rand_number(),
            )
            .await
            .unwrap();

        let mut key_man = KeyManager::default();
        let mut device_identity_key_store = DeviceIdentityKeyStore::new(device.clone());
        let mut device_pre_key_store = DevicePreKeyStore::new(device.clone());
        let mut device_signed_pre_key_store = DeviceSignedPreKeyStore::new(device.clone());
        let mut device_kyber_pre_key_store = DeviceKyberPreKeyStore::new(device.clone());
        let pre_key_record = key_man
            .generate_pre_key(&mut device_pre_key_store, &mut OsRng)
            .await
            .unwrap();
        let signed_pre_key_record1 = key_man
            .generate_signed_pre_key(
                &mut device_identity_key_store,
                &mut device_signed_pre_key_store,
                &mut OsRng,
            )
            .await
            .unwrap();
        let signed_pre_key_record2 = key_man
            .generate_signed_pre_key(
                &mut device_identity_key_store,
                &mut device_signed_pre_key_store,
                &mut OsRng,
            )
            .await
            .unwrap();
        let kyber_pre_key_record1 = key_man
            .generate_kyber_pre_key(
                &mut device_identity_key_store,
                &mut device_kyber_pre_key_store,
            )
            .await
            .unwrap();
        let kyber_pre_key_record2 = key_man
            .generate_kyber_pre_key(
                &mut device_identity_key_store,
                &mut device_kyber_pre_key_store,
            )
            .await
            .unwrap();
        let kyber_pre_key_record3 = key_man
            .generate_kyber_pre_key(
                &mut device_identity_key_store,
                &mut device_kyber_pre_key_store,
            )
            .await
            .unwrap();

        device_pre_key_store
            .save_pre_key(pre_key_record.id().unwrap(), &pre_key_record)
            .await
            .unwrap();

        device_signed_pre_key_store
            .save_signed_pre_key(
                signed_pre_key_record1.id().unwrap(),
                &signed_pre_key_record1,
            )
            .await
            .unwrap();
        device_signed_pre_key_store
            .save_signed_pre_key(
                signed_pre_key_record2.id().unwrap(),
                &signed_pre_key_record2,
            )
            .await
            .unwrap();

        device_kyber_pre_key_store
            .save_kyber_pre_key(kyber_pre_key_record1.id().unwrap(), &kyber_pre_key_record1)
            .await
            .unwrap();
        device_kyber_pre_key_store
            .save_kyber_pre_key(kyber_pre_key_record2.id().unwrap(), &kyber_pre_key_record2)
            .await
            .unwrap();
        device_kyber_pre_key_store
            .save_kyber_pre_key(kyber_pre_key_record3.id().unwrap(), &kyber_pre_key_record3)
            .await
            .unwrap();

        let (pkidmax, spkidmax, kpkidmax) = device.lock().await.get_key_ids().await.unwrap();

        assert_eq!(pkidmax, 0);
        assert_eq!(spkidmax, 1);
        assert_eq!(kpkidmax, 2);
    }

    #[tokio::test]
    async fn remove_key_and_get_ids_test() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));

        device
            .lock()
            .await
            .insert_account_key_information(
                IdentityKeyPair::generate(&mut OsRng),
                new_rand_number(),
            )
            .await
            .unwrap();

        let mut key_man = KeyManager::default();
        let mut device_identity_key_store = DeviceIdentityKeyStore::new(device.clone());
        let mut device_pre_key_store = DevicePreKeyStore::new(device.clone());
        let mut device_signed_pre_key_store = DeviceSignedPreKeyStore::new(device.clone());
        let mut device_kyber_pre_key_store = DeviceKyberPreKeyStore::new(device.clone());
        let pre_key_record1 = key_man
            .generate_pre_key(&mut device_pre_key_store, &mut OsRng)
            .await
            .unwrap();
        let pre_key_record2 = key_man
            .generate_pre_key(&mut device_pre_key_store, &mut OsRng)
            .await
            .unwrap();
        let signed_pre_key_record1 = key_man
            .generate_signed_pre_key(
                &mut device_identity_key_store,
                &mut device_signed_pre_key_store,
                &mut OsRng,
            )
            .await
            .unwrap();
        let signed_pre_key_record2 = key_man
            .generate_signed_pre_key(
                &mut device_identity_key_store,
                &mut device_signed_pre_key_store,
                &mut OsRng,
            )
            .await
            .unwrap();
        let kyber_pre_key_record1 = key_man
            .generate_kyber_pre_key(
                &mut device_identity_key_store,
                &mut device_kyber_pre_key_store,
            )
            .await
            .unwrap();
        let kyber_pre_key_record2 = key_man
            .generate_kyber_pre_key(
                &mut device_identity_key_store,
                &mut device_kyber_pre_key_store,
            )
            .await
            .unwrap();
        let kyber_pre_key_record3 = key_man
            .generate_kyber_pre_key(
                &mut device_identity_key_store,
                &mut device_kyber_pre_key_store,
            )
            .await
            .unwrap();

        device_pre_key_store
            .save_pre_key(pre_key_record1.id().unwrap(), &pre_key_record1)
            .await
            .unwrap();
        device_pre_key_store
            .remove_pre_key(pre_key_record1.id().unwrap())
            .await
            .unwrap();
        device_pre_key_store
            .save_pre_key(pre_key_record2.id().unwrap(), &pre_key_record2)
            .await
            .unwrap();

        device_signed_pre_key_store
            .save_signed_pre_key(
                signed_pre_key_record1.id().unwrap(),
                &signed_pre_key_record1,
            )
            .await
            .unwrap();
        device_signed_pre_key_store
            .save_signed_pre_key(
                signed_pre_key_record2.id().unwrap(),
                &signed_pre_key_record2,
            )
            .await
            .unwrap();

        device_kyber_pre_key_store
            .save_kyber_pre_key(kyber_pre_key_record1.id().unwrap(), &kyber_pre_key_record1)
            .await
            .unwrap();
        device_kyber_pre_key_store
            .save_kyber_pre_key(kyber_pre_key_record2.id().unwrap(), &kyber_pre_key_record2)
            .await
            .unwrap();
        device_kyber_pre_key_store
            .save_kyber_pre_key(kyber_pre_key_record3.id().unwrap(), &kyber_pre_key_record3)
            .await
            .unwrap();

        let (pkidmax, spkidmax, kpkidmax) = device.lock().await.get_key_ids().await.unwrap();

        assert_eq!(pkidmax, 1);
        assert_eq!(spkidmax, 1);
        assert_eq!(kpkidmax, 2);
    }

    #[tokio::test]
    async fn store_and_load_contact() {
        let device = Arc::new(Mutex::new(Device::new(connect().await)));

        let contacts = vec![new_contact(), new_contact(), new_contact()];

        device
            .lock()
            .await
            .store_contact(&contacts[0])
            .await
            .unwrap();
        device
            .lock()
            .await
            .store_contact(&contacts[1])
            .await
            .unwrap();
        device
            .lock()
            .await
            .store_contact(&contacts[2])
            .await
            .unwrap();

        let retrived_contacts = device.lock().await.load_contacts().await.unwrap();

        assert_eq!(contacts, retrived_contacts);
    }

    #[tokio::test]
    async fn insert_and_get_address_by_nickname() {
        let device = Arc::new(Device::new(connect().await));

        let nicknames = vec!["Alice", "Bob", "Charlie"];

        let nickname_map = HashMap::from([
            (nicknames[0], new_service_id()),
            (nicknames[1], new_service_id()),
            (nicknames[2], new_service_id()),
        ]);

        device
            .insert_service_id_for_nickname(nicknames[0], &nickname_map[nicknames[0]])
            .await
            .unwrap();
        device
            .insert_service_id_for_nickname(nicknames[1], &nickname_map[nicknames[1]])
            .await
            .unwrap();
        device
            .insert_service_id_for_nickname(nicknames[2], &nickname_map[nicknames[2]])
            .await
            .unwrap();

        assert_eq!(
            device
                .get_service_id_by_nickname(nicknames[0])
                .await
                .unwrap(),
            nickname_map[nicknames[0]]
        );
        assert_eq!(
            device
                .get_service_id_by_nickname(nicknames[1])
                .await
                .unwrap(),
            nickname_map[nicknames[1]]
        );
        assert_eq!(
            device
                .get_service_id_by_nickname(nicknames[2])
                .await
                .unwrap(),
            nickname_map[nicknames[2]]
        );
    }
}
