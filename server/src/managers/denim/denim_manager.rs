use super::chunk_cache::ChunkCache;
use crate::{
    availability_listener::AvailabilityListener, database::SignalDatabase,
    managers::manager::Manager,
};
use anyhow::Result;
use common::web_api::DenimChunk;
use libsignal_core::ProtocolAddress;

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

    pub async fn insert_chunk(&self, address: &ProtocolAddress, chunk: &DenimChunk) -> Result<u64> {
        self.chunk_cache.insert(address, chunk).await
    }
}

// #[cfg(test)]
// pub mod denim_manager_tests {
//     use common::signalservice::Envelope;
//
//     use super::*;
//     use crate::{
//         postgres::PostgresDatabase,
//         test_utils::{
//             database::database_connect,
//             message_cache::{generate_uuid, teardown, MockWebSocketConnection},
//             user::new_account_and_address,
//         },
//     };
//
//     async fn init_manager() -> DenIMManager<PostgresDatabase, MockWebSocketConnection> {
//         DenIMManager::<PostgresDatabase, MockWebSocketConnection> {
//             db: database_connect().await,
//             chunk_cache: ChunkCache::connect(),
//         }
//     }
//
//     #[tokio::test]
//     async fn test_have_chunk() {
//         let denim_manager = init_manager().await;
//         let connection = denim_manager.chunk_cache.get_connection().await.unwrap();
//         let (_, address) = new_account_and_address();
//         let message_guid = generate_uuid();
//         let mut envelope = Envelope::default();
//
//         // Cache
//         denim_manager
//             .chunk_cache
//             .insert(&address.clone(), &mut envelope, &message_guid)
//             .await
//             .unwrap();
//
//         // Act
//         let has_messages = denim_manager
//             .chunk_cache
//             .has_messages(&address)
//             .await
//             .unwrap();
//
//         // Teardown cache
//         teardown(&denim_manager.message_cache.test_key, connection).await;
//
//         assert_eq!(has_messages, true);
//     }
// }
