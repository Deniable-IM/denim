use super::{buffer::Buffer, chunk_cache::ChunkCache};
use crate::{
    availability_listener::AvailabilityListener, managers::manager::Manager,
    storage::database::SignalDatabase,
};
use anyhow::Result;
use common::web_api::{DeniablePayload, DenimChunk};
use libsignal_core::ProtocolAddress;
use uuid::Uuid;

pub struct DenIMManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    db: T,
    chunk_cache: ChunkCache<U>,
}

impl<T, U> Manager for DenIMManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T, U> DenIMManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    pub fn new(db: T, chunk_cache: ChunkCache<U>) -> Self {
        Self { db, chunk_cache }
    }

    pub async fn get_incoming_chunks(&self, sender: &ProtocolAddress) -> Result<Vec<DenimChunk>> {
        self.chunk_cache
            .get_all_chunks(sender, Buffer::Sender)
            .await
    }

    pub async fn set_incoming_chunks(
        &self,
        sender: &ProtocolAddress,
        chunks: Vec<DenimChunk>,
    ) -> Result<u64> {
        let mut count = 0;
        for chunk in chunks {
            count += self
                .chunk_cache
                .insert(sender, Buffer::Sender, &chunk, &Uuid::new_v4().to_string())
                .await?
        }
        Ok(count)
    }

    pub async fn get_outgoing_chunks(&self, recever: &ProtocolAddress) -> Result<Vec<DenimChunk>> {
        self.chunk_cache
            .get_all_chunks(recever, Buffer::Receiver)
            .await
    }

    pub async fn set_outgoing_chunks(
        &self,
        recever: &ProtocolAddress,
        chunks: Vec<DenimChunk>,
    ) -> Result<u64> {
        let mut count = 0;
        for chunk in chunks {
            count += self
                .chunk_cache
                .insert(
                    recever,
                    Buffer::Receiver,
                    &chunk,
                    &Uuid::new_v4().to_string(),
                )
                .await?
        }
        Ok(count)
    }

    pub fn get_deniable_payload(&self, _recever: &ProtocolAddress) -> Result<DeniablePayload> {
        todo!()
    }

    pub fn queue_deniable_payload(
        &self,
        _recever: &ProtocolAddress,
        _payload: DeniablePayload,
    ) -> Result<()> {
        todo!()
    }

    pub fn create_deniable_payload(
        &self,
        _sender: &ProtocolAddress,
        _chunks: Vec<DenimChunk>,
    ) -> Result<DeniablePayload> {
        todo!()
    }
}

#[cfg(test)]
pub mod denim_manager_tests {
    use super::*;
    use crate::{
        storage::postgres::PostgresDatabase,
        test_utils::{
            database::database_connect,
            message_cache::{teardown, MockWebSocketConnection},
            user::new_account_and_address,
        },
    };

    async fn init_manager() -> DenIMManager<PostgresDatabase, MockWebSocketConnection> {
        DenIMManager::<PostgresDatabase, MockWebSocketConnection> {
            db: database_connect().await,
            chunk_cache: ChunkCache::connect(),
        }
    }

    fn create_chunks(size: usize) -> Vec<DenimChunk> {
        let mut chunks = Vec::new();
        for i in 0..size {
            chunks.push(DenimChunk {
                flags: i as i32,
                ..Default::default()
            });
        }
        chunks
    }

    #[tokio::test]
    async fn test_incoming_and_outgoing_buffer() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, sender_address) = new_account_and_address();
        let incoming_chunks = create_chunks(1);

        let (_, reciver_address) = new_account_and_address();
        let outgoing_chunks = create_chunks(2);

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks)
            .await;

        let _ = denim_manager
            .set_outgoing_chunks(&reciver_address, outgoing_chunks)
            .await;

        let result_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let result_outgoing_chunks = denim_manager
            .get_outgoing_chunks(&reciver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(result_incoming_chunks.len(), 1);
        assert_eq!(result_outgoing_chunks.len(), 2);
    }

    #[tokio::test]
    async fn test_multiple_incoming_buffers() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, sender_address1) = new_account_and_address();
        let incoming_chunks1 = create_chunks(1);

        let (_, sender_address2) = new_account_and_address();
        let incoming_chunks2 = create_chunks(2);

        let _ = denim_manager
            .set_incoming_chunks(&sender_address1, incoming_chunks1)
            .await;

        let _ = denim_manager
            .set_incoming_chunks(&sender_address2, incoming_chunks2)
            .await;

        let result_incoming_chunks1 = denim_manager
            .get_incoming_chunks(&sender_address1)
            .await
            .unwrap();

        let result_incoming_chunks2 = denim_manager
            .get_incoming_chunks(&sender_address2)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(result_incoming_chunks1.len(), 1);
        assert_eq!(result_incoming_chunks2.len(), 2);
    }

    #[tokio::test]
    async fn test_multiple_outgoing_buffers() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, reciver_address1) = new_account_and_address();
        let outgoing_chunks1 = create_chunks(1);

        let (_, reciver_address2) = new_account_and_address();
        let outgoing_chunks2 = create_chunks(2);

        let _ = denim_manager
            .set_outgoing_chunks(&reciver_address1, outgoing_chunks1)
            .await;

        let _ = denim_manager
            .set_outgoing_chunks(&reciver_address2, outgoing_chunks2)
            .await;

        let result_outgoing_chunks1 = denim_manager
            .get_outgoing_chunks(&reciver_address1)
            .await
            .unwrap();

        let result_outgoing_chunks2 = denim_manager
            .get_outgoing_chunks(&reciver_address2)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(result_outgoing_chunks1.len(), 1);
        assert_eq!(result_outgoing_chunks2.len(), 2);
    }
}
