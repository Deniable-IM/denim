#[cfg(test)]
use crate::test_utils::random_string;
use crate::{availability_listener::AvailabilityListener, managers::manager::Manager};
use anyhow::Result;
use common::signalservice::Envelope;
use deadpool_redis::{redis::cmd, Config, Connection, Runtime};
use libsignal_core::ProtocolAddress;
use std::{
    any::Any,
    collections::HashMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::Mutex;

const PAGE_SIZE: u32 = 100;

type ListenerMap<T> = Arc<Mutex<HashMap<String, Arc<Mutex<T>>>>>;

#[derive(Debug)]
pub struct MessageCache<T>
where
    T: AvailabilityListener,
{
    pool: deadpool_redis::Pool,
    listeners: ListenerMap<T>,
    #[cfg(test)]
    pub test_key: String,
}

impl<T> Manager for MessageCache<T>
where
    T: AvailabilityListener + 'static,
{
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl<T> Clone for MessageCache<T>
where
    T: AvailabilityListener,
{
    fn clone(&self) -> Self {
        #[cfg(not(test))]
        return Self {
            pool: self.pool.clone(),
            listeners: self.listeners.clone(),
        };
        #[cfg(test)]
        Self {
            pool: self.pool.clone(),
            listeners: self.listeners.clone(),
            test_key: self.test_key.clone(),
        }
    }
}

impl<T> MessageCache<T>
where
    T: AvailabilityListener,
{
    pub fn connect() -> Self {
        let _ = dotenv::dotenv();
        let redis_url = std::env::var("REDIS_URL").expect("Unable to read REDIS_URL .env var");
        let redis_config = Config::from_url(redis_url);
        let redis_pool: deadpool_redis::Pool = redis_config
            .create_pool(Some(Runtime::Tokio1))
            .expect("Failed to create connection pool");
        #[cfg(not(test))]
        return Self {
            pool: redis_pool,
            listeners: Arc::new(Mutex::new(HashMap::new())),
        };
        #[cfg(test)]
        Self {
            pool: redis_pool,
            listeners: Arc::new(Mutex::new(HashMap::new())),
            test_key: random_string(8),
        }
    }

    pub async fn get_connection(&self) -> Result<Connection> {
        Ok(self.pool.get().await?)
    }

    pub async fn insert(
        &self,
        address: &ProtocolAddress,
        envelope: &mut Envelope,
        message_guid: &str,
    ) -> Result<u64> {
        let mut connection = self.pool.get().await?;

        let queue_key: String = self.get_message_queue_key(address);
        let queue_metadata_key: String = self.get_message_queue_metadata_key(address);
        let queue_total_index_key: String = self.get_queue_index_key();

        envelope.server_guid = Some(message_guid.to_string());
        let data = bincode::serialize(&envelope)?;

        let message_guid_exists = cmd("HEXISTS")
            .arg(&queue_metadata_key)
            .arg(message_guid)
            .query_async::<u8>(&mut connection)
            .await?;

        if message_guid_exists == 1 {
            let num = cmd("HGET")
                .arg(&queue_metadata_key)
                .arg(message_guid)
                .query_async::<String>(&mut connection)
                .await?;

            return Ok(num.parse().expect("Could not parse redis id"));
        }

        let message_id = cmd("HINCRBY")
            .arg(&queue_metadata_key)
            .arg("counter")
            .arg(1)
            .query_async::<u64>(&mut connection)
            .await?;

        cmd("ZADD")
            .arg(&queue_key)
            .arg("NX")
            .arg(message_id)
            .arg(&data)
            .query_async::<()>(&mut connection)
            .await?;

        cmd("HSET")
            .arg(&queue_metadata_key)
            .arg(message_guid)
            .arg(message_id)
            .query_async::<()>(&mut connection)
            .await?;

        cmd("EXPIRE")
            .arg(&queue_key)
            .arg(2678400)
            .query_async::<()>(&mut connection)
            .await?;

        cmd("EXPIRE")
            .arg(&queue_metadata_key)
            .arg(2678400)
            .query_async::<()>(&mut connection)
            .await?;

        let time = SystemTime::now();
        let time_in_millis: u64 = time.duration_since(UNIX_EPOCH)?.as_secs();

        cmd("ZADD")
            .arg(&queue_total_index_key)
            .arg("NX")
            .arg(time_in_millis)
            .arg(&queue_key)
            .query_async::<()>(&mut connection)
            .await?;

        // notifies the message availability manager
        let queue_name = format!("{}::{}", address.name(), address.device_id());
        if let Some(listener) = self.listeners.lock().await.get(&queue_name) {
            listener.lock().await.send_cached().await;
        }

        Ok(message_id)
    }

    pub async fn remove(
        &self,
        address: &ProtocolAddress,
        message_guids: Vec<String>,
    ) -> Result<Vec<Envelope>> {
        let mut connection = self.pool.get().await?;

        let queue_key: String = self.get_message_queue_key(address);
        let queue_metadata_key: String = self.get_message_queue_metadata_key(address);
        let queue_total_index_key: String = self.get_queue_index_key();

        let mut removed_messages: Vec<Envelope> = Vec::new();

        for guid in message_guids {
            let message_id: Option<String> = cmd("HGET")
                .arg(&queue_metadata_key)
                .arg(&guid)
                .query_async(&mut connection)
                .await?;

            if let Some(msg_id) = message_id.clone() {
                // retrieving the message
                let envelope = cmd("ZRANGE")
                    .arg(&queue_key)
                    .arg(&msg_id)
                    .arg(&msg_id)
                    .arg("BYSCORE")
                    .arg("LIMIT")
                    .arg(0)
                    .arg(1)
                    .query_async::<Option<Vec<Vec<u8>>>>(&mut connection)
                    .await?;

                // delete the message
                cmd("ZREMRANGEBYSCORE")
                    .arg(&queue_key)
                    .arg(&msg_id)
                    .arg(&msg_id)
                    .query_async::<()>(&mut connection)
                    .await?;

                // delete the guid from the cache
                cmd("HDEL")
                    .arg(&queue_metadata_key)
                    .arg(&guid)
                    .query_async::<()>(&mut connection)
                    .await?;

                if let Some(envel) = envelope {
                    removed_messages.push(bincode::deserialize(&envel[0])?);
                }
            }
        }

        if cmd("ZCARD")
            .arg(&queue_key)
            .query_async::<u64>(&mut connection)
            .await?
            == 0
        {
            cmd("DEL")
                .arg(&queue_key)
                .query_async::<()>(&mut connection)
                .await?;

            cmd("DEL")
                .arg(&queue_metadata_key)
                .query_async::<()>(&mut connection)
                .await?;

            cmd("ZREM")
                .arg(&queue_total_index_key)
                .arg(&queue_key)
                .query_async::<()>(&mut connection)
                .await?;
        }

        Ok(removed_messages)
    }

    pub async fn has_messages(&self, address: &ProtocolAddress) -> Result<bool> {
        let mut connection = self.pool.get().await?;

        let msg_count = cmd("ZCARD")
            .arg(self.get_message_queue_key(address))
            .query_async::<u32>(&mut connection)
            .await?;
        let queue_name = format!("{}::{}", address.name(), address.device_id());
        if let Some(listener) = self.listeners.lock().await.get(&queue_name) {
            listener.lock().await.send_cached().await;
        }

        Ok(msg_count > 0)
    }

    pub async fn get_all_messages(&self, address: &ProtocolAddress) -> Result<Vec<Envelope>> {
        let messages = self.get_items(address, -1).await?;

        if messages.is_empty() {
            return Ok(Vec::new());
        }

        let mut envelopes = Vec::new();
        // messages is a [envelope1, msg_id1, envelope2, msg_id2, ...]
        for i in (0..messages.len()).step_by(2) {
            envelopes.push(bincode::deserialize(&messages[i])?);
        }
        Ok(envelopes)
    }

    async fn get_items(
        &self,
        address: &ProtocolAddress,
        after_message_id: i32,
    ) -> Result<Vec<Vec<u8>>> {
        let mut connection = self.pool.get().await?;
        let queue_key = self.get_message_queue_key(address);
        let queue_lock_key = self.get_persist_in_progress_key(address);
        let message_sort = format!("({}", after_message_id);

        let locked = cmd("GET")
            .arg(&queue_lock_key)
            .query_async::<Option<String>>(&mut connection)
            .await?;

        // if there is a queue lock key on, due to persist of message.
        if locked.is_some() {
            return Ok(Vec::new());
        }

        let messages = cmd("ZRANGE")
            .arg(queue_key.clone())
            .arg(message_sort.clone())
            .arg("+inf")
            .arg("BYSCORE")
            .arg("LIMIT")
            .arg(0)
            .arg(PAGE_SIZE)
            .arg("WITHSCORES")
            .query_async::<Vec<Vec<u8>>>(&mut connection)
            .await?;

        Ok(messages.clone())
    }

    pub async fn get_messages_to_persist(
        &self,
        address: &ProtocolAddress,
        limit: i32,
    ) -> Result<Vec<Envelope>> {
        let mut connection = self.pool.get().await?;

        let messages = cmd("ZRANGE")
            .arg(self.get_message_queue_key(address))
            .arg(0)
            .arg(limit)
            .query_async::<Vec<Vec<u8>>>(&mut connection)
            .await?;

        let valid_envelopes: Vec<Envelope> = messages
            .into_iter()
            .filter_map(|m| bincode::deserialize(&m).ok())
            .collect();

        Ok(valid_envelopes)
    }

    pub async fn get_message_queues_to_persist(
        &self,
        max_time: u64,
        limit: u8,
    ) -> Result<Vec<String>> {
        let mut connection = self.pool.get().await?;
        let queue_index_key = self.get_queue_index_key();

        let results = cmd("ZRANGE")
            .arg(&queue_index_key)
            .arg(0)
            .arg(max_time)
            .arg("BYSCORE")
            .arg("LIMIT")
            .arg(0)
            .arg(limit)
            .query_async::<Vec<String>>(&mut connection)
            .await?;

        if !results.is_empty() {
            cmd("ZREM")
                .arg(&queue_index_key)
                .arg(&results)
                .query_async::<()>(&mut connection)
                .await?;
        }
        Ok(results)
    }

    pub async fn lock_queue_for_persistence(&self, address: &ProtocolAddress) -> Result<()> {
        let mut connection = self.pool.get().await?;

        cmd("SETEX")
            .arg(self.get_persist_in_progress_key(address))
            .arg(30)
            .arg("1")
            .query_async::<()>(&mut connection)
            .await?;

        Ok(())
    }

    pub async fn unlock_queue_for_persistence(&self, address: &ProtocolAddress) -> Result<()> {
        let mut connection = self.pool.get().await?;

        cmd("DEL")
            .arg(self.get_persist_in_progress_key(address))
            .query_async::<()>(&mut connection)
            .await?;

        let queue_name = format!("{}::{}", address.name(), address.device_id());
        if let Some(listener) = self.listeners.lock().await.get(&queue_name) {
            listener.lock().await.send_persisted().await;
        }

        Ok(())
    }

    pub fn get_message_queue_key(&self, address: &ProtocolAddress) -> String {
        #[cfg(not(test))]
        return format!(
            "user_queue::{{{}::{}}}",
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}user_queue::{{{}::{}}}",
            self.test_key,
            address.name(),
            address.device_id()
        )
    }

    pub fn get_persist_in_progress_key(&self, address: &ProtocolAddress) -> String {
        #[cfg(not(test))]
        return format!(
            "user_queue_persisting::{{{}::{}}}",
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}user_queue_persisting::{{{}::{}}}",
            self.test_key,
            address.name(),
            address.device_id()
        )
    }

    fn get_message_queue_metadata_key(&self, address: &ProtocolAddress) -> String {
        #[cfg(not(test))]
        return format!(
            "user_queue_metadata::{{{}::{}}}",
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}user_queue_metadata::{{{}::{}}}",
            self.test_key,
            address.name(),
            address.device_id()
        )
    }

    pub fn get_queue_index_key(&self) -> String {
        #[cfg(not(test))]
        return "user_queue_index_key".to_string(); // Should be changed if we use Redis Cluster
        #[cfg(test)]
        format!("{}user_queue_index_key", self.test_key) // Should be changed if we use Redis Cluster
    }

    pub fn get_account_and_device_id_from_queue_key(&self, queue_key: &str) -> (String, String) {
        let parts = queue_key.split("::").collect::<Vec<&str>>();
        let account_id = parts[1].trim_matches('{').to_string();
        let device_id = parts[2].trim_end_matches('}').to_string();
        (account_id, device_id)
    }

    pub async fn add_message_availability_listener(
        &mut self,
        address: &ProtocolAddress,
        listener: Arc<Mutex<T>>,
    ) {
        let queue_name = format!("{}::{}", address.name(), address.device_id());
        self.listeners.lock().await.insert(queue_name, listener);
    }

    pub async fn remove_message_availability_listener(&mut self, address: &ProtocolAddress) {
        let queue_name: String = format!("{}::{}", address.name(), address.device_id());
        self.listeners.lock().await.remove(&queue_name);
    }
}

#[cfg(test)]
pub mod message_cache_tests {
    use super::*;
    use crate::test_utils::{
        message_cache::{generate_envelope, generate_uuid, teardown, MockWebSocketConnection},
        user::new_protocol_address,
    };

    #[tokio::test]
    async fn test_message_availability_listener_new_messages() {
        let mut message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();
        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));
        let uuid = generate_uuid();
        let address = new_protocol_address();

        let mut envelope = generate_envelope(&uuid);

        message_cache
            .add_message_availability_listener(&address, websocket.clone())
            .await;

        message_cache
            .insert(&address, &mut envelope, &uuid)
            .await
            .unwrap();

        assert!(websocket.lock().await.evoked_handle_new_messages);
    }

    #[tokio::test]

    async fn test_insert() {
        let message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();
        let mut connection = message_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let message_guid = generate_uuid();

        let mut envelope = generate_envelope(&message_guid);

        let message_id = message_cache
            .insert(&address, &mut envelope, &message_guid)
            .await
            .unwrap();

        let result = cmd("ZRANGEBYSCORE")
            .arg(message_cache.get_message_queue_key(&address))
            .arg(message_id)
            .arg(message_id)
            .query_async::<Vec<Vec<u8>>>(&mut connection)
            .await
            .unwrap();

        teardown(&message_cache.test_key, connection).await;

        assert_eq!(
            envelope,
            bincode::deserialize::<Envelope>(&result[0]).unwrap()
        );
    }

    #[tokio::test]

    async fn test_insert_same_id() {
        let message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();

        let mut connection = message_cache.pool.get().await.unwrap();

        let address = new_protocol_address();

        let message_guid = generate_uuid();
        let mut envelope1 = generate_envelope(&message_guid);
        let mut envelope2 = generate_envelope(&message_guid);

        let message_id = message_cache
            .insert(&address, &mut envelope1, &message_guid)
            .await
            .unwrap();

        // should return the same message id
        let message_id_2 = message_cache
            .insert(&address, &mut envelope2, &message_guid)
            .await
            .unwrap();

        let result = cmd("ZRANGEBYSCORE")
            .arg(message_cache.get_message_queue_key(&address))
            .arg(message_id_2)
            .arg(message_id_2)
            .query_async::<Vec<Vec<u8>>>(&mut connection)
            .await
            .unwrap();

        teardown(&message_cache.test_key, connection).await;

        assert_eq!(
            envelope1,
            bincode::deserialize::<Envelope>(&result[0]).unwrap()
        );

        assert_eq!(message_id, message_id_2);
    }

    #[tokio::test]

    async fn test_insert_different_ids() {
        let message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();

        let mut connection = message_cache.pool.get().await.unwrap();

        let address = new_protocol_address();

        let message_guid1 = generate_uuid();
        let message_guid2 = generate_uuid();
        let mut envelope1 = generate_envelope(&message_guid1);
        let mut envelope2 = generate_envelope(&message_guid2);

        // inserting messages
        let message_id = message_cache
            .insert(&address, &mut envelope1, &message_guid1)
            .await
            .unwrap();

        let message_id_2 = message_cache
            .insert(&address, &mut envelope2, &message_guid2)
            .await
            .unwrap();

        // querying the envelopes
        let result_1 = cmd("ZRANGEBYSCORE")
            .arg(message_cache.get_message_queue_key(&address))
            .arg(message_id)
            .arg(message_id)
            .query_async::<Vec<Vec<u8>>>(&mut connection)
            .await
            .unwrap();

        let result_2 = cmd("ZRANGEBYSCORE")
            .arg(message_cache.get_message_queue_key(&address))
            .arg(message_id_2)
            .arg(message_id_2)
            .query_async::<Vec<Vec<u8>>>(&mut connection)
            .await
            .unwrap();

        teardown(&message_cache.test_key, connection).await;

        // they are inserted as two different messages
        assert_ne!(message_id, message_id_2);

        assert_ne!(
            bincode::deserialize::<Envelope>(&result_1[0]).unwrap(),
            bincode::deserialize::<Envelope>(&result_2[0]).unwrap()
        );
    }

    #[tokio::test]

    async fn test_remove() {
        let message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();
        let connection = message_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let message_guid = generate_uuid();
        let mut envelope = generate_envelope(&message_guid);

        message_cache
            .insert(&address, &mut envelope, &message_guid)
            .await
            .unwrap();

        let removed_messages = message_cache
            .remove(&address, vec![message_guid])
            .await
            .unwrap();

        teardown(&message_cache.test_key, connection).await;

        assert_eq!(removed_messages.len(), 1);
        assert_eq!(removed_messages[0], envelope);
    }

    #[tokio::test]

    async fn test_get_all_messages() {
        let message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();
        let connection = message_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let mut envelopes = Vec::new();

        for _ in 0..10 {
            let message_guid = generate_uuid();
            let mut envelope = generate_envelope(&message_guid);

            message_cache
                .insert(&address, &mut envelope, &message_guid)
                .await
                .unwrap();

            envelopes.push(envelope);
        }

        //getting those messages
        let messages = message_cache.get_all_messages(&address).await.unwrap();

        teardown(&message_cache.test_key, connection).await;

        assert_eq!(messages.len(), 10);

        for (message, envelope) in messages.into_iter().zip(envelopes.into_iter()) {
            assert_eq!(message, envelope);
        }
    }

    #[tokio::test]

    async fn test_has_messages() {
        let message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();
        let connection = message_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let message_guid = generate_uuid();

        let mut envelope = generate_envelope(&message_guid);

        let does_not_has_messages = message_cache.has_messages(&address).await.unwrap();

        message_cache
            .insert(&address, &mut envelope, &message_guid)
            .await
            .unwrap();

        let has_messages = message_cache.has_messages(&address).await.unwrap();

        teardown(&message_cache.test_key, connection).await;

        assert!(!does_not_has_messages);
        assert!(has_messages);
    }

    #[tokio::test]

    async fn test_get_messages_to_persist() {
        let message_cache: MessageCache<MockWebSocketConnection> = MessageCache::connect();
        let connection = message_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let message_guid = generate_uuid();

        let mut envelope = generate_envelope(&message_guid);

        message_cache
            .insert(&address, &mut envelope, &message_guid)
            .await
            .unwrap();

        let envelopes = message_cache
            .get_messages_to_persist(&address, -1)
            .await
            .unwrap();

        teardown(&message_cache.test_key, connection).await;

        assert_eq!(envelopes.len(), 1);
    }
}
