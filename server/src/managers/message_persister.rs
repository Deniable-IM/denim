use crate::{
    account::{Account, Device},
    database::SignalDatabase,
    managers::{account_manager::AccountManager, messages_manager::MessagesManager},
    message_cache::{MessageAvailabilityListener, MessageCache},
    persister::{Persister, RunFlag},
};
use anyhow::{anyhow, Result};
use libsignal_core::{ProtocolAddress, ServiceId};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use super::manager::{self, Manager};

const QUEUE_BATCH_LIMIT: u8 = 100;
const MESSAGE_BATCH_LIMIT: u8 = 100;
const PERSIST_DELAY: u64 = 600;

#[derive(Debug)]
pub struct MessagePersister<T: SignalDatabase, U: MessageAvailabilityListener + Send> {
    run_flag: RunFlag,
    message_cache: MessageCache<U>,
    messages_manager: MessagesManager<T, U>,
    account_manager: AccountManager<T>,
    db: T,
}

impl<T, U> Clone for MessagePersister<T, U>
where
    T: SignalDatabase + Send + 'static + Sync,
    U: MessageAvailabilityListener + Send + 'static,
{
    fn clone(&self) -> Self {
        Self {
            run_flag: self.run_flag.clone(),
            message_cache: self.message_cache.clone(),
            messages_manager: self.messages_manager.clone(),
            account_manager: self.account_manager.clone(),
            db: self.db.clone(),
        }
    }
}

/*
 * Takes the message queues from the cache that is >10 minutes old,
 * removes them from the cache and puts them into the database using the MessagesManager
*/
impl<T, U> Persister<T, U> for MessagePersister<T, U>
where
    T: SignalDatabase + Send + 'static + Sync,
    U: MessageAvailabilityListener + Send + 'static,
{
    type Manager = Vec<Box<dyn Manager>>;

    fn listen(db: T, managers: Self::Manager) -> MessagePersister<T, U> {
        let messages_manager = manager::get::<MessagesManager<T, U>>(&managers)
            .unwrap()
            .clone();
        let account_manager = manager::get::<AccountManager<T>>(&managers)
            .unwrap()
            .clone();
        let message_cache = manager::get::<MessageCache<U>>(&managers).unwrap().clone();

        let mut message_persister = MessagePersister {
            run_flag: Arc::new(AtomicBool::new(true)),
            message_cache,
            messages_manager,
            account_manager,
            db,
        };

        let message_persister_clone = message_persister.clone();

        tokio::spawn(async move {
            while message_persister.run_flag.load(Ordering::Relaxed) {
                let _ = message_persister.persist().await;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        message_persister_clone
    }

    fn get_run_flag(&self) -> &RunFlag {
        &self.run_flag
    }

    // Finds the message queues where the oldest message is >10 minutes old.
    async fn persist(&mut self) -> Result<()> {
        let mut queues_to_persist;

        while {
            let time_in_secs: u64 = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("Failed to get time")
                .as_secs();

            queues_to_persist = self
                .message_cache
                .get_message_queues_to_persist(time_in_secs - PERSIST_DELAY, QUEUE_BATCH_LIMIT)
                .await?;

            !queues_to_persist.is_empty()
        } {
            for queue_key in &queues_to_persist {
                let (account_id, device_id) = self
                    .message_cache
                    .get_account_and_device_id_from_queue_key(queue_key);

                let service_id =
                    ServiceId::parse_from_service_id_string(&account_id).ok_or_else(|| {
                        anyhow!("Failed to parse service id from queue: {}", queue_key)
                    })?;

                let account = self.account_manager.get_account(&service_id).await?;

                let device_id = device_id.parse::<u32>()?;
                let device = account
                    .devices()
                    .iter()
                    .find(|device| device.device_id() == device_id.into())
                    .ok_or_else(|| anyhow!("Could not find device in account."))?;

                self.persist_queue(&account, device).await?;
            }
        }

        Ok(())
    }
}

impl<T, U> MessagePersister<T, U>
where
    T: SignalDatabase + Send + 'static + Sync,
    U: MessageAvailabilityListener + Send + 'static,
{
    /// Takes the message queues where the oldest message is >10 minutes old.
    async fn persist_queue(&mut self, account: &Account, device: &Device) -> Result<()> {
        let mut messages;
        let protocol_address =
            ProtocolAddress::new(account.aci().service_id_string(), device.device_id());

        self.message_cache
            .lock_queue_for_persistence(&protocol_address)
            .await?;

        while {
            messages = self
                .message_cache
                .get_messages_to_persist(&protocol_address, MESSAGE_BATCH_LIMIT as i32)
                .await?;

            !messages.is_empty()
        } {
            self.messages_manager
                .persist_messages(&protocol_address, messages)
                .await?;
        }

        self.message_cache
            .unlock_queue_for_persistence(&protocol_address)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod message_persister_tests {
    use crate::{
        database::SignalDatabase,
        managers::{account_manager::AccountManager, messages_manager::MessagesManager},
        message_cache::{self, MessageCache},
        persister::Persister,
        postgres::PostgresDatabase,
        test_utils::{
            database::database_connect,
            message_cache::{generate_envelope, generate_uuid, teardown, MockWebSocketConnection},
            user::new_account_and_address,
        },
    };
    use common::signalservice::Envelope;
    use libsignal_core::ProtocolAddress;
    use redis::cmd;
    use std::{
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    use tokio::sync::Mutex;

    use super::MessagePersister;

    fn time_now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    async fn insert_with_custom_timestamp(
        message_cache: &MessageCache<MockWebSocketConnection>,
        timestamp: u64,
        protocol_address: &ProtocolAddress,
        envelope: &mut Envelope,
        message_guid: &str,
    ) {
        let mut connection = message_cache.get_connection().await.unwrap();
        let queue_total_index_key = message_cache.get_queue_index_key();
        let queue_key = message_cache.get_message_queue_key(protocol_address);

        cmd("ZADD")
            .arg(&queue_total_index_key)
            .arg("NX")
            .arg(timestamp)
            .arg(&queue_key)
            .query_async::<()>(&mut connection)
            .await
            .unwrap();

        message_cache
            .insert(protocol_address, envelope, message_guid)
            .await
            .unwrap();
    }

    async fn check_queue_key_timestamp(
        message_cache: &MessageCache<MockWebSocketConnection>,
        max_time: u64,
        limit: u8,
    ) -> Vec<String> {
        let queue_index_key = message_cache.get_queue_index_key();
        let mut connection = message_cache.get_connection().await.unwrap();

        cmd("ZRANGE")
            .arg(&queue_index_key)
            .arg(0)
            .arg(max_time)
            .arg("BYSCORE")
            .arg("LIMIT")
            .arg(0)
            .arg(limit)
            .query_async::<Vec<String>>(&mut connection)
            .await
            .unwrap()
    }

    async fn has_messages(
        cache: &MessageCache<MockWebSocketConnection>,
        queue_keys: &Vec<String>,
    ) -> bool {
        let mut connection = cache.get_connection().await.unwrap();

        for queue_key in queue_keys {
            if cmd("ZCARD")
                .arg(queue_key)
                .query_async::<u64>(&mut connection)
                .await
                .unwrap()
                != 0
            {
                return false;
            }
        }
        true
    }

    async fn no_queue_persist_keys_in_cache(
        message_cache: &MessageCache<MockWebSocketConnection>,
        protocol_addresses: &Vec<ProtocolAddress>,
    ) -> bool {
        let mut connection = message_cache.get_connection().await.unwrap();

        for address in protocol_addresses {
            let queue_lock_key = message_cache.get_persist_in_progress_key(address);

            let locked = cmd("GET")
                .arg(&queue_lock_key)
                .query_async::<Option<String>>(&mut connection)
                .await
                .unwrap();

            if locked.is_some() {
                return false;
            }
        }
        true
    }

    async fn message_persister_test_setup_run(
        message_times: Vec<u64>,
    ) -> (bool, bool, bool, Vec<String>, Vec<String>) {
        let _ = dotenv::dotenv();
        let db = database_connect().await;
        let cache = MessageCache::connect();
        let mut message_manager = MessagesManager::new(db.clone(), cache.clone());
        let account_manager = AccountManager::new(db.clone());

        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));

        let now_timestamp = time_now_secs();

        let ten_minutes_past_timestamp = now_timestamp - 600;

        let mut accounts = Vec::new();
        let mut protocol_addresses = Vec::new();

        for message_time in message_times {
            let (account, protocol_address) = new_account_and_address();
            accounts.push(account.clone());
            protocol_addresses.push(protocol_address.clone());

            let envelope_uuid = generate_uuid();
            let mut envelope = generate_envelope(&envelope_uuid);

            db.add_account(&account).await.unwrap();

            insert_with_custom_timestamp(
                &cache,
                message_time,
                &protocol_address,
                &mut envelope,
                &envelope_uuid,
            )
            .await;
        }

        message_manager
            .add_message_availability_listener(
                protocol_addresses.first().unwrap(),
                websocket.clone(),
            )
            .await;

        let queue_keys_before_message_persister = check_queue_key_timestamp(
            &cache,
            now_timestamp, // now time
            10,
        )
        .await;

        let queues_over_ten_minutes = check_queue_key_timestamp(
            &cache,
            ten_minutes_past_timestamp, // ten minutes
            10,
        )
        .await;

        let message_persister: MessagePersister<PostgresDatabase, MockWebSocketConnection> =
            MessagePersister::listen(
                db.clone(),
                vec![
                    Box::new(message_manager.clone()),
                    Box::new(cache.clone()),
                    Box::new(account_manager.clone()),
                ],
            );

        tokio::time::sleep(Duration::from_millis(2000)).await;

        message_persister.stop().await;

        let queue_keys_after_message_persister =
            check_queue_key_timestamp(&cache, now_timestamp, 10).await;

        let message_queues_older_than_10_minutes_has_been_deleted =
            has_messages(&cache, &queues_over_ten_minutes).await;

        let no_queue_persist_keys_in_cache =
            no_queue_persist_keys_in_cache(&cache, &protocol_addresses).await;

        teardown(&cache.test_key, cache.get_connection().await.unwrap()).await;

        for account in accounts {
            db.delete_account(&account.aci().into()).await.unwrap();
        }

        let handle_persisted_messages_evoked =
            websocket.lock().await.evoked_handle_messages_persisted;

        (
            handle_persisted_messages_evoked,
            no_queue_persist_keys_in_cache,
            message_queues_older_than_10_minutes_has_been_deleted,
            queue_keys_before_message_persister,
            queue_keys_after_message_persister,
        )
    }

    #[tokio::test]
    async fn test_message_persister_late_msg() {
        let (
            handle_persisted_messages_evoked,
            no_queue_lock_keys_in_cache,
            old_msg_deleted,
            msg_before,
            msg_after,
        ) = message_persister_test_setup_run(vec![time_now_secs() - 660]).await;

        assert!(handle_persisted_messages_evoked);
        assert!(no_queue_lock_keys_in_cache);
        assert!(old_msg_deleted);
        assert_eq!(msg_before.len(), 1);
        assert_eq!(msg_after.len(), 0);
    }

    #[tokio::test]
    async fn test_message_persister_new_msg() {
        let (
            handle_persisted_messages_evoked,
            no_queue_lock_keys_in_cache,
            old_msg_deleted,
            msg_before,
            msg_after,
        ) = message_persister_test_setup_run(vec![time_now_secs() - 300]).await;

        assert!(!handle_persisted_messages_evoked);
        assert!(no_queue_lock_keys_in_cache);
        assert!(old_msg_deleted);
        assert_eq!(msg_before.len(), 1);
        assert_eq!(msg_after.len(), 1);
    }

    #[tokio::test]
    async fn test_message_persister_new_and_late_msg() {
        let message_time = time_now_secs() - 600;

        let message_times = vec![
            message_time - 100,
            message_time - 200,
            message_time + 100,
            message_time + 200,
            message_time + 300,
        ];

        let (
            handle_persisted_messages_evoked,
            no_queue_lock_keys_in_cache,
            old_msg_deleted,
            msg_before,
            msg_after,
        ) = message_persister_test_setup_run(message_times).await;

        assert!(handle_persisted_messages_evoked);
        assert!(no_queue_lock_keys_in_cache);
        assert!(old_msg_deleted);
        assert_eq!(msg_before.len(), 5);
        assert_eq!(msg_after.len(), 3);
    }
}
