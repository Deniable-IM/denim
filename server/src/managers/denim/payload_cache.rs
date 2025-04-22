use super::buffer::Buffer;
use crate::{
    availability_listener::{add, notify_cached, remove, AvailabilityListener},
    managers::{
        manager::Manager,
        message::message_cache::{self, MessageCache},
    },
    storage::redis::{self, Decoder},
};
use anyhow::{Ok, Result};
use common::web_api::DeniablePayload;
use deadpool_redis::Connection;
use libsignal_core::ProtocolAddress;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

/// Use default decoder implementation
impl Decoder<DeniablePayload> for DeniablePayload {}

type ListenerMap<T> = Arc<Mutex<HashMap<String, Arc<Mutex<T>>>>>;

#[derive(Debug)]
pub struct PayloadCache<T> {
    pub(crate) pool: deadpool_redis::Pool,
    pub(crate) listeners: ListenerMap<T>,
    #[cfg(test)]
    pub test_key: String,
}

impl<T> Manager for PayloadCache<T>
where
    T: AvailabilityListener,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T> From<MessageCache<T>> for PayloadCache<T>
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

impl<T> PayloadCache<T>
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
        payload: &DeniablePayload,
        payload_guid: &str,
    ) -> Result<u64> {
        let connection = self.pool.get().await?;
        let queue_key: String = self.get_queue_key(address, buffer);
        let queue_metadata_key: String = self.get_queue_metadata_key(address, buffer);
        let queue_total_index_key: String = self.get_queue_index_key(buffer);
        let value = bincode::serialize(payload)?;

        let payload_id = redis::insert(
            connection,
            queue_key,
            queue_metadata_key,
            queue_total_index_key,
            payload_guid,
            value,
        )
        .await;

        notify_cached(self.listeners.clone(), address).await;
        payload_id
    }

    pub async fn remove(
        &self,
        address: &ProtocolAddress,
        buffer: Buffer,
        payload_guid: Vec<String>,
    ) -> Result<Vec<DeniablePayload>> {
        let connection = self.pool.get().await?;
        let queue_key: String = self.get_queue_key(address, buffer);
        let queue_metadata_key: String = self.get_queue_metadata_key(address, buffer);
        let queue_total_index_key: String = self.get_queue_index_key(buffer);

        redis::remove(
            connection,
            queue_key,
            queue_metadata_key,
            queue_total_index_key,
            payload_guid,
        )
        .await
    }

    pub async fn get_all_payloads(
        &self,
        address: &ProtocolAddress,
        buffer: Buffer,
    ) -> Result<Vec<DeniablePayload>> {
        let connection = self.pool.get().await?;
        let queue_key = self.get_queue_key(address, buffer);
        let queue_lock_key = self.get_persist_in_progress_key(address, buffer);

        let values = redis::get_values(connection, queue_key, queue_lock_key, -1).await?;
        if values.is_empty() {
            return Ok(Vec::new());
        }

        Ok(DeniablePayload::decode(values)?)
    }

    pub async fn get_all_payloads_raw(
        &self,
        address: &ProtocolAddress,
        buffer: Buffer,
    ) -> Result<Vec<Vec<u8>>> {
        let connection = self.pool.get().await?;
        let queue_key = self.get_queue_key(address, buffer);
        let queue_lock_key = self.get_persist_in_progress_key(address, buffer);

        let values = redis::get_values(connection, queue_key, queue_lock_key, -1).await?;
        if values.is_empty() {
            return Ok(Vec::new());
        }

        Ok(redis::Bytes::decode(values)?)
    }

    pub async fn take_values(
        &self,
        address: &ProtocolAddress,
        buffer: Buffer,
        data_size: usize,
    ) -> Result<Vec<(Vec<u8>, i32)>> {
        let queue_key = self.get_queue_key(address, buffer);
        let queue_lock_key = self.get_persist_in_progress_key(address, buffer);
        let queue_metadata_key: String = self.get_queue_metadata_key(address, buffer);
        let queue_total_index_key: String = self.get_queue_index_key(buffer);

        let mut result = Vec::new();
        let mut take = data_size;
        while take > 0 {
            let connection = self.pool.get().await?;
            let (data, taken, order) = redis::take_value(
                connection,
                queue_key.clone(),
                queue_metadata_key.clone(),
                queue_total_index_key.clone(),
                queue_lock_key.clone(),
                take,
            )
            .await?;
            if taken == 0 {
                break;
            } else {
                take -= taken;
                result.push((data, order));
            }
        }

        Ok(result)
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
            "payload_{}_queue::{{{}::{}}}",
            buffer,
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}payload_{}_queue::{{{}::{}}}",
            self.test_key,
            buffer,
            address.name(),
            address.device_id()
        )
    }

    fn get_persist_in_progress_key(&self, address: &ProtocolAddress, buffer: Buffer) -> String {
        #[cfg(not(test))]
        return format!(
            "payload_{}_queue_persisting::{{{}::{}}}",
            buffer,
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}payload_{}_queue_persisting::{{{}::{}}}",
            self.test_key,
            buffer,
            address.name(),
            address.device_id()
        )
    }

    fn get_queue_metadata_key(&self, address: &ProtocolAddress, buffer: Buffer) -> String {
        #[cfg(not(test))]
        return format!(
            "payload_{}_queue_metadata::{{{}::{}}}",
            buffer,
            address.name(),
            address.device_id()
        );
        #[cfg(test)]
        format!(
            "{}payload_{}_queue_metadata::{{{}::{}}}",
            self.test_key,
            buffer,
            address.name(),
            address.device_id()
        )
    }

    fn get_queue_index_key(&self, buffer: Buffer) -> String {
        #[cfg(not(test))]
        return format!("payload_{}_queue_index_key", buffer);
        #[cfg(test)]
        format!("{}payload_{}_queue_index_key", self.test_key, buffer)
    }
}

#[cfg(test)]
pub mod payload_cache_tests {
    use super::*;
    use crate::test_utils::{
        message_cache::{
            generate_payload, generate_uuid, teardown, DeniablePayloadType, MockWebSocketConnection,
        },
        user::new_protocol_address,
    };
    use ::redis::{cmd, Value};
    use common::web_api::SignalMessage;

    #[tokio::test]
    async fn test_availability_listener_new_messages() {
        let mut payload_cache: PayloadCache<MockWebSocketConnection> = PayloadCache::connect();
        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));
        let uuid = generate_uuid();
        let address = new_protocol_address();

        let mut payload = DeniablePayload::SignalMessage(SignalMessage::default());
        let buffer = Buffer::Receiver;

        payload_cache
            .add_availability_listener(&address, websocket.clone())
            .await;

        payload_cache
            .insert(&address, buffer, &mut payload, &uuid)
            .await
            .unwrap();

        assert!(websocket.lock().await.evoked_handle_new_messages);
    }

    #[tokio::test]
    async fn test_insert() {
        let payload_cache: PayloadCache<MockWebSocketConnection> = PayloadCache::connect();
        let mut connection = payload_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let payload_guid = generate_uuid();

        let mut payload = DeniablePayload::SignalMessage(SignalMessage::default());
        let reciver = Buffer::Receiver;

        let payload_id = payload_cache
            .insert(&address, reciver, &mut payload, &payload_guid)
            .await
            .unwrap();

        let values = cmd("ZRANGEBYSCORE")
            .arg(payload_cache.get_queue_key(&address, reciver))
            .arg(payload_id)
            .arg(payload_id)
            .query_async::<Vec<Value>>(&mut connection)
            .await
            .unwrap();

        teardown(&payload_cache.test_key, connection).await;

        let result = DeniablePayload::decode(values).unwrap();
        assert_eq!(payload, result[0]);
    }

    #[tokio::test]
    async fn test_insert_multiple_payloads() {
        let payload_cache: PayloadCache<MockWebSocketConnection> = PayloadCache::connect();
        let mut connection = payload_cache.pool.get().await.unwrap();
        let address = new_protocol_address();

        let mut payload1 = generate_payload(DeniablePayloadType::SignalMessage);
        let mut payload2 = generate_payload(DeniablePayloadType::Envelope);
        let mut payload3 = generate_payload(DeniablePayloadType::KeyRequest);
        let mut payload4 = generate_payload(DeniablePayloadType::KeyResponse);

        let reciver = Buffer::Receiver;

        let payload_id1 = payload_cache
            .insert(&address, reciver, &mut payload1, &generate_uuid())
            .await
            .unwrap();

        let _payload_id2 = payload_cache
            .insert(&address, reciver, &mut payload2, &generate_uuid())
            .await
            .unwrap();

        let _payload_id3 = payload_cache
            .insert(&address, reciver, &mut payload3, &generate_uuid())
            .await
            .unwrap();

        let payload_id4 = payload_cache
            .insert(&address, reciver, &mut payload4, &generate_uuid())
            .await
            .unwrap();

        let values = cmd("ZRANGEBYSCORE")
            .arg(payload_cache.get_queue_key(&address, reciver))
            .arg(payload_id1)
            .arg(payload_id4)
            .query_async::<Vec<Value>>(&mut connection)
            .await
            .unwrap();

        teardown(&payload_cache.test_key, connection).await;

        let result = DeniablePayload::decode(values).unwrap();
        assert_eq!(4, result.len());
        assert_eq!(payload1, result[0]);
        assert_eq!(payload2, result[1]);
        assert_eq!(payload3, result[2]);
        assert_eq!(payload4, result[3]);
    }

    #[tokio::test]
    async fn test_remove() {
        let payload_cache: PayloadCache<MockWebSocketConnection> = PayloadCache::connect();
        let connection = payload_cache.pool.get().await.unwrap();
        let address = new_protocol_address();
        let payload_guid = generate_uuid();

        let mut payload = DeniablePayload::SignalMessage(SignalMessage::default());
        let reciver = Buffer::Receiver;

        payload_cache
            .insert(&address, reciver, &mut payload, &payload_guid)
            .await
            .unwrap();

        let removed_payloads = payload_cache
            .remove(&address, reciver, vec![payload_guid])
            .await
            .unwrap();

        teardown(&payload_cache.test_key, connection).await;

        assert_eq!(removed_payloads.len(), 1);
        assert_eq!(removed_payloads[0], payload);
    }
}
