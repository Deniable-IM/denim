use super::buffer::Buffer;
use crate::{
    availability_listener::{add, notify_cached, remove, AvailabilityListener},
    managers::{
        manager::Manager,
        message::message_cache::{self, MessageCache},
    },
    storage::redis::{self, Decoder},
};
use anyhow::Result;
use common::web_api::DenimChunk;
use deadpool_redis::Connection;
use libsignal_core::ProtocolAddress;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

/// Use default decoder implementation
impl Decoder<DenimChunk> for DenimChunk {}

type ListenerMap<T> = Arc<Mutex<HashMap<String, Arc<Mutex<T>>>>>;

#[derive(Debug)]
pub struct ChunkCache<T> {
    pub(crate) pool: deadpool_redis::Pool,
    pub(crate) listeners: ListenerMap<T>,
    #[cfg(test)]
    pub test_key: String,
}

impl<T> Manager for ChunkCache<T>
where
    T: AvailabilityListener,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T> From<MessageCache<T>> for ChunkCache<T>
where
    T: AvailabilityListener,
{
    fn from(cache: MessageCache<T>) -> Self {
        #[cfg(not(test))]
        return Self {
            pool: cache.pool.clone(),
            listeners: cache.listeners.clone(),
        };

        #[cfg(test)]
        Self {
            pool: cache.pool.clone(),
            listeners: cache.listeners.clone(),
            test_key: cache.test_key.clone(),
        }
    }
}

impl<T> ChunkCache<T>
where
    T: AvailabilityListener,
{
    pub fn connect() -> Self {
        message_cache::MessageCache::<T>::connect().into()
    }

    pub async fn get_connection(&self) -> Result<Connection> {
        Ok(self.pool.get().await?)
    }

    pub async fn insert(
        &self,
        address: &ProtocolAddress,
        buffer: Buffer,
        chunk: &DenimChunk,
        chunk_guid: &str,
    ) -> Result<u64> {
        let connection = self.pool.get().await?;
        let queue_key: String = self.get_queue_key(address, buffer);
        let queue_metadata_key: String = self.get_queue_metadata_key(address, buffer);
        let queue_total_index_key: String = self.get_queue_index_key(buffer);
        let value = bincode::serialize(chunk)?;

        let chunk_id = redis::insert(
            connection,
            queue_key,
            queue_metadata_key,
            queue_total_index_key,
            chunk_guid,
            value,
        )
        .await;

        notify_cached(self.listeners.clone(), address).await;
        chunk_id
    }

    pub async fn remove(
        &self,
        address: &ProtocolAddress,
        buffer: Buffer,
        chunk_guid: Vec<String>,
    ) -> Result<Vec<DenimChunk>> {
        let connection = self.pool.get().await?;
        let queue_key: String = self.get_queue_key(address, buffer);
        let queue_metadata_key: String = self.get_queue_metadata_key(address, buffer);
        let queue_total_index_key: String = self.get_queue_index_key(buffer);

        redis::remove(
            connection,
            queue_key,
            queue_metadata_key,
            queue_total_index_key,
            chunk_guid,
        )
        .await
    }

    pub async fn get_all_chunks(
        &self,
        address: &ProtocolAddress,
        buffer: Buffer,
    ) -> Result<Vec<DenimChunk>> {
        let connection = self.pool.get().await?;
        let queue_key = self.get_queue_key(address, buffer);
        let queue_lock_key = self.get_persist_in_progress_key(address, buffer);

        let values = redis::get_values(connection, queue_key, queue_lock_key, -1).await?;
        if values.is_empty() {
            return Ok(Vec::new());
        }

        Ok(DenimChunk::decode(values)?)
    }

    pub async fn add_availability_listener(
        &mut self,
        address: &ProtocolAddress,
        listener: Arc<Mutex<T>>,
    ) {
        add(self.listeners.clone(), address, listener).await;
    }

    pub async fn remove_availability_listener(&mut self, address: &ProtocolAddress) {
        remove(self.listeners.clone(), address).await;
    }

    fn get_queue_key(&self, address: &ProtocolAddress, buffer: Buffer) -> String {
        #[cfg(not(test))]
        return format!(
            "chunk_{}_queue::{{{}::{}}}",
            buffer,
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}chunk_{}_queue::{{{}::{}}}",
            self.test_key,
            buffer,
            address.name(),
            address.device_id()
        )
    }

    fn get_persist_in_progress_key(&self, address: &ProtocolAddress, buffer: Buffer) -> String {
        #[cfg(not(test))]
        return format!(
            "chunk_{}_queue_persisting::{{{}::{}}}",
            buffer,
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}chunk_{}_queue_persisting::{{{}::{}}}",
            self.test_key,
            buffer,
            address.name(),
            address.device_id()
        )
    }

    fn get_queue_metadata_key(&self, address: &ProtocolAddress, buffer: Buffer) -> String {
        #[cfg(not(test))]
        return format!(
            "chunk_{}_queue_metadata::{{{}::{}}}",
            buffer,
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}chunk_{}_queue_metadata::{{{}::{}}}",
            self.test_key,
            buffer,
            address.name(),
            address.device_id()
        )
    }

    fn get_queue_index_key(&self, buffer: Buffer) -> String {
        #[cfg(not(test))]
        return format!("chunk_{}_queue_index_key", buffer);
        #[cfg(test)]
        format!("{}chunk_{}_queue_index_key", self.test_key, buffer)
    }
}

#[cfg(test)]
pub mod chunk_cache_tests {
    use super::*;
    use crate::test_utils::{
        message_cache::{generate_chunk, generate_uuid, teardown, MockWebSocketConnection},
        user::new_protocol_address,
    };
    use ::redis::{cmd, Value};

    #[tokio::test]
    async fn test_availability_listener_new_messages() {
        let mut chunk_cache: ChunkCache<MockWebSocketConnection> = ChunkCache::connect();
        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));
        let uuid = generate_uuid();
        let address = new_protocol_address();

        let mut chunk = generate_chunk();
        let buffer = Buffer::Receiver;

        chunk_cache
            .add_availability_listener(&address, websocket.clone())
            .await;

        chunk_cache
            .insert(&address, buffer, &mut chunk, &uuid)
            .await
            .unwrap();

        assert!(websocket.lock().await.evoked_handle_new_messages);
    }

    #[tokio::test]
    async fn test_insert() {
        let chunk_cache: ChunkCache<MockWebSocketConnection> = ChunkCache::connect();
        let mut connection = chunk_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let chunk_guid = generate_uuid();

        let mut chunk = generate_chunk();
        let reciver = Buffer::Receiver;

        let chunk_id = chunk_cache
            .insert(&address, reciver, &mut chunk, &chunk_guid)
            .await
            .unwrap();

        let values = cmd("ZRANGEBYSCORE")
            .arg(chunk_cache.get_queue_key(&address, reciver))
            .arg(chunk_id)
            .arg(chunk_id)
            .query_async::<Vec<Value>>(&mut connection)
            .await
            .unwrap();

        teardown(&chunk_cache.test_key, connection).await;

        let result = DenimChunk::decode(values).unwrap();
        assert_eq!(chunk, result[0]);
    }

    #[tokio::test]
    async fn test_remove() {
        let chunk_cache: ChunkCache<MockWebSocketConnection> = ChunkCache::connect();
        let connection = chunk_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let chunk_guid = generate_uuid();
        let mut envelope = generate_chunk();
        let reciver = Buffer::Receiver;

        chunk_cache
            .insert(&address, reciver, &mut envelope, &chunk_guid)
            .await
            .unwrap();

        let removed_chunks = chunk_cache
            .remove(&address, reciver, vec![chunk_guid])
            .await
            .unwrap();

        teardown(&chunk_cache.test_key, connection).await;

        assert_eq!(removed_chunks.len(), 1);
        assert_eq!(removed_chunks[0], envelope);
    }
}
