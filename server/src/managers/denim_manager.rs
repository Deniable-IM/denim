use crate::{
    database::SignalDatabase,
    message_cache::{MessageAvailabilityListener, MessageCache},
};

pub struct DenIMManager<T, U>
where
    T: SignalDatabase + Send,
    U: MessageAvailabilityListener + Send,
{
    db: T,
    message_cache: MessageCache<U>,
}

impl<T, U> DenIMManager<T, U>
where
    T: SignalDatabase + Send,
    U: MessageAvailabilityListener + Send,
{
    pub fn new(db: T, message_cache: MessageCache<U>) -> Self {
        Self { db, message_cache }
    }
}

#[cfg(test)]
pub mod denim_manager_tests {
    use common::signalservice::Envelope;

    use super::*;
    use crate::{
        message_cache::MessageCache,
        postgres::PostgresDatabase,
        test_utils::{
            database::database_connect,
            message_cache::{generate_uuid, teardown, MockWebSocketConnection},
            user::new_account_and_address,
        },
    };

    async fn init_manager() -> DenIMManager<PostgresDatabase, MockWebSocketConnection> {
        DenIMManager::<PostgresDatabase, MockWebSocketConnection> {
            db: database_connect().await,
            message_cache: MessageCache::connect(),
        }
    }

    #[tokio::test]
    async fn test_have_chunk() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.message_cache.get_connection().await.unwrap();
        let (_, address) = new_account_and_address();
        let message_guid = generate_uuid();
        let mut envelope = Envelope::default();

        // Cache
        denim_manager
            .message_cache
            .insert(&address.clone(), &mut envelope, &message_guid)
            .await
            .unwrap();

        // Act
        let has_messages = denim_manager
            .message_cache
            .has_messages(&address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.message_cache.test_key, connection).await;

        assert_eq!(has_messages, true);
    }
}
