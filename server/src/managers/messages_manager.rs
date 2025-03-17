use crate::{
    availability_listener::AvailabilityListener, database::SignalDatabase,
    message_cache::MessageCache,
};
use anyhow::{Ok, Result};
use common::signalservice::Envelope;
use libsignal_core::ProtocolAddress;
use std::{any::Any, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::manager::Manager;

#[derive(Debug)]
pub struct MessagesManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener + 'static,
{
    db: T,
    message_cache: MessageCache<U>,
}

impl<T, U> Clone for MessagesManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            message_cache: self.message_cache.clone(),
        }
    }
}

impl<T, U> Manager for MessagesManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<T, U> MessagesManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    pub fn new(db: T, message_cache: MessageCache<U>) -> Self {
        Self { db, message_cache }
    }
    /// Add message to cache
    pub async fn insert(&self, address: &ProtocolAddress, envelope: &mut Envelope) -> Result<u64> {
        self.message_cache
            .insert(address, envelope, &Uuid::new_v4().to_string())
            .await
    }

    /// Check if user has persisted messages
    pub async fn may_have_persisted_messages(
        &self,
        address: &ProtocolAddress,
    ) -> Result<(bool, &str)> {
        let cache_has_messages = self.message_cache.has_messages(address).await?;
        let db_has_messages = self.has_messages(address).await?;

        let outcome = if cache_has_messages && db_has_messages {
            "both"
        } else if cache_has_messages {
            "cached"
        } else if db_has_messages {
            "persisted"
        } else {
            "none"
        };

        Ok((cache_has_messages || db_has_messages, outcome))
    }

    /// Get all messages for a user
    pub async fn get_messages_for_device(
        &self,
        address: &ProtocolAddress,
        cached_msg_only: bool,
    ) -> Result<Vec<Envelope>> {
        let cached_messages = self.message_cache.get_all_messages(address).await?;

        let db_messages = if !cached_msg_only {
            self.db.get_messages(address).await?
        } else {
            vec![]
        };

        Ok([cached_messages, db_messages].concat())
    }

    /// Delete messages from cache and DB
    pub async fn delete(
        &self,
        address: &ProtocolAddress,
        message_guids: Vec<String>,
    ) -> Result<Vec<Envelope>> {
        let cache_removed_messages = self.message_cache.remove(address, message_guids).await?;
        let db_removed_messages = self.db.delete_messages(address).await?;

        Ok([cache_removed_messages, db_removed_messages].concat())
    }

    /// Remove messages from cache and store in DB
    pub async fn persist_messages(
        &self,
        address: &ProtocolAddress,
        messages: Vec<Envelope>,
    ) -> Result<usize> {
        let message_guids: Vec<String> = messages
            .iter()
            .map(|m| m.server_guid().to_string())
            .collect();

        self.db.push_message_queue(address, messages).await?;

        Ok(self
            .message_cache
            .remove(address, message_guids)
            .await?
            .len())
    }

    pub async fn add_message_availability_listener(
        &mut self,
        address: &ProtocolAddress,
        listener: Arc<Mutex<U>>,
    ) {
        self.message_cache
            .add_message_availability_listener(address, listener)
            .await;
    }

    pub async fn remove_message_availability_listener(&mut self, address: &ProtocolAddress) {
        self.message_cache
            .remove_message_availability_listener(address)
            .await;
    }
}

impl<T, U> MessagesManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    async fn has_messages(&self, address: &ProtocolAddress) -> Result<bool> {
        Ok(self.db.count_messages(address).await? > 0)
    }
}

#[cfg(test)]
pub mod message_manager_tests {
    use super::*;
    use crate::{
        message_cache::MessageCache,
        postgres::PostgresDatabase,
        test_utils::{
            database::database_connect,
            message_cache::{teardown, MockWebSocketConnection},
            user::{new_account_and_address, new_protocol_address},
        },
    };

    async fn init_manager() -> MessagesManager<PostgresDatabase, MockWebSocketConnection> {
        MessagesManager::<PostgresDatabase, MockWebSocketConnection> {
            message_cache: MessageCache::connect(),
            db: database_connect().await,
        }
    }

    #[tokio::test]
    async fn test_may_have_cached_persisted_messages() {
        let msg_manager = init_manager().await;
        let address = new_protocol_address();
        let mut envelope = Envelope::default();

        // Cache
        msg_manager.insert(&address, &mut envelope).await.unwrap();

        // Act
        let may_have_messages = msg_manager
            .may_have_persisted_messages(&address)
            .await
            .unwrap();

        // Teardown cache
        teardown(
            &msg_manager.message_cache.test_key,
            msg_manager.message_cache.get_connection().await.unwrap(),
        )
        .await;

        assert_eq!(may_have_messages, (true, "cached"));
    }

    #[tokio::test]
    async fn test_may_have_persisted_persisted_messages() {
        let msg_manager = init_manager().await;
        let (account, address) = new_account_and_address();
        let envelope = Envelope::default();

        // DB
        msg_manager.db.add_account(&account).await.unwrap();

        msg_manager
            .db
            .push_message_queue(&address, vec![envelope])
            .await
            .unwrap();

        // Act
        let may_have_messages = msg_manager
            .may_have_persisted_messages(&address)
            .await
            .unwrap();

        // Teardown DB
        msg_manager
            .db
            .delete_account(&account.aci().into())
            .await
            .unwrap();

        assert_eq!(may_have_messages, (true, "persisted"));
    }

    #[tokio::test]
    async fn test_may_have_both_cached_and_db_persisted_messages() {
        let msg_manager = init_manager().await;
        let (account, address) = new_account_and_address();
        let mut envelope = Envelope::default();

        // Cache
        msg_manager.insert(&address, &mut envelope).await.unwrap();

        // DB
        msg_manager.db.add_account(&account).await.unwrap();

        msg_manager
            .db
            .push_message_queue(&address, vec![envelope])
            .await
            .unwrap();

        // Act
        let may_have_messages = msg_manager
            .may_have_persisted_messages(&address)
            .await
            .unwrap();

        // Teardown DB and cache
        msg_manager
            .db
            .delete_account(&account.aci().into())
            .await
            .unwrap();

        teardown(
            &msg_manager.message_cache.test_key,
            msg_manager.message_cache.get_connection().await.unwrap(),
        )
        .await;

        assert_eq!(may_have_messages, (true, "both"));
    }

    #[tokio::test]
    async fn test_count_messages() {
        let msg_manager = init_manager().await;
        let (account, address) = new_account_and_address();
        let envelope = Envelope::default();

        // DB
        msg_manager.db.add_account(&account).await.unwrap();

        msg_manager
            .db
            .push_message_queue(&address, vec![envelope])
            .await
            .unwrap();

        // Act
        let count = msg_manager.db.count_messages(&address).await.unwrap();

        // Teardown DB
        msg_manager
            .db
            .delete_account(&account.aci().into())
            .await
            .unwrap();

        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn test_get_messages_for_device() {
        let msg_manager = init_manager().await;
        let (account, address) = new_account_and_address();
        let mut envelope1 = Envelope::default();
        let mut envelope2 = Envelope::default();

        // Cache
        msg_manager.insert(&address, &mut envelope1).await.unwrap();

        msg_manager.insert(&address, &mut envelope2).await.unwrap();

        // DB
        msg_manager.db.add_account(&account).await.unwrap();

        msg_manager
            .db
            .push_message_queue(&address, vec![envelope1, envelope2])
            .await
            .unwrap();

        let messages_for_device_cache_and_db = msg_manager
            .get_messages_for_device(&address, false)
            .await
            .unwrap();

        // Teardown DB and cache
        msg_manager
            .db
            .delete_account(&account.aci().into())
            .await
            .unwrap();

        teardown(
            &msg_manager.message_cache.test_key,
            msg_manager.message_cache.get_connection().await.unwrap(),
        )
        .await;

        assert_eq!(messages_for_device_cache_and_db.len(), 4);
    }

    #[tokio::test]
    async fn test_get_cache_only_messages_for_device() {
        let msg_manager = init_manager().await;
        let (account, address) = new_account_and_address();
        let mut envelope = Envelope::default();

        // Cache
        msg_manager.insert(&address, &mut envelope).await.unwrap();

        // DB
        msg_manager.db.add_account(&account).await.unwrap();

        msg_manager
            .db
            .push_message_queue(&address, vec![envelope])
            .await
            .unwrap();

        // Act
        let messages_for_device_cache_only = msg_manager
            .get_messages_for_device(&address, true)
            .await
            .unwrap();

        let messages_for_device_db_and_cache = msg_manager
            .get_messages_for_device(&address, false)
            .await
            .unwrap();

        // Teardown DB and cache
        msg_manager
            .db
            .delete_account(&account.aci().into())
            .await
            .unwrap();

        teardown(
            &msg_manager.message_cache.test_key,
            msg_manager.message_cache.get_connection().await.unwrap(),
        )
        .await;

        assert_eq!(messages_for_device_cache_only.len(), 1);
        assert_eq!(messages_for_device_db_and_cache.len(), 2);
    }

    #[tokio::test]
    async fn test_delete_messages() {
        let msg_manager = init_manager().await;
        let (account, address) = new_account_and_address();
        let mut envelope1 = Envelope::default();
        let envelope2 = Envelope::default();

        // Cache
        msg_manager.insert(&address, &mut envelope1).await.unwrap();

        // DB
        msg_manager.db.add_account(&account).await.unwrap();

        msg_manager
            .db
            .push_message_queue(&address, vec![envelope1.clone(), envelope2.clone()])
            .await
            .unwrap();

        // Act
        let messages_for_device_db_and_cache = msg_manager
            .get_messages_for_device(&address, false)
            .await
            .unwrap();

        let deleted_messages = msg_manager
            .delete(
                &address,
                vec![
                    envelope1.server_guid().to_string(),
                    envelope2.server_guid().to_string(),
                ],
            )
            .await
            .unwrap();

        // Teardown DB and cache
        msg_manager
            .db
            .delete_account(&account.aci().into())
            .await
            .unwrap();

        teardown(
            &msg_manager.message_cache.test_key,
            msg_manager.message_cache.get_connection().await.unwrap(),
        )
        .await;

        assert_eq!(messages_for_device_db_and_cache.len(), 3);
        assert_eq!(deleted_messages.len(), 3);
    }

    #[tokio::test]
    async fn test_persist_messages() {
        let msg_manager = init_manager().await;
        let (account, address) = new_account_and_address();
        let mut envelope1 = Envelope::default();
        let mut envelope2 = Envelope::default();

        // Cache
        msg_manager.insert(&address, &mut envelope1).await.unwrap();

        msg_manager.insert(&address, &mut envelope2).await.unwrap();

        // DB
        msg_manager.db.add_account(&account).await.unwrap();

        // Act
        let messages_in_cache = msg_manager
            .get_messages_for_device(&address, true)
            .await
            .unwrap();

        let messages_in_db = msg_manager.db.get_messages(&address).await.unwrap();

        let count_persisted_in_db = msg_manager
            .persist_messages(&address, vec![envelope1, envelope2])
            .await
            .unwrap();

        // Teardown DB and cache
        msg_manager
            .db
            .delete_account(&account.aci().into())
            .await
            .unwrap();

        teardown(
            &msg_manager.message_cache.test_key,
            msg_manager.message_cache.get_connection().await.unwrap(),
        )
        .await;

        assert_eq!(messages_in_cache.len(), 2);
        assert_eq!(messages_in_db.len(), 0);
        assert_eq!(count_persisted_in_db, 2);
    }
}
