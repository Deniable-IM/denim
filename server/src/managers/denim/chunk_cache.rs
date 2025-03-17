#[cfg(test)]
use crate::test_utils::random_string;
use crate::{availability_listener::AvailabilityListener, managers::manager::Manager};
use anyhow::Result;
use common::web_api::DenimChunk;
use deadpool_redis::{Config, Connection, Runtime};
use libsignal_core::ProtocolAddress;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

type ListenerMap<T> = Arc<Mutex<HashMap<String, Arc<Mutex<T>>>>>;

#[derive(Debug)]
pub struct ChunkCache<T>
where
    T: AvailabilityListener,
{
    pool: deadpool_redis::Pool,
    listeners: ListenerMap<T>,
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

impl<T> ChunkCache<T>
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

    pub async fn insert(&self, address: &ProtocolAddress, chunk: &DenimChunk) -> Result<u64> {
        Ok(1337)
        // let mut connection = self.pool.get().await?;
        //
        // let queue_key: String = self.get_message_queue_key(address);
        // let queue_metadata_key: String = self.get_message_queue_metadata_key(address);
        // let queue_total_index_key: String = self.get_queue_index_key();
        //
        // envelope.server_guid = Some(message_guid.to_string());
        // let data = bincode::serialize(&envelope)?;
        //
        // let message_guid_exists = cmd("HEXISTS")
        //     .arg(&queue_metadata_key)
        //     .arg(message_guid)
        //     .query_async::<u8>(&mut connection)
        //     .await?;
        //
        // if message_guid_exists == 1 {
        //     let num = cmd("HGET")
        //         .arg(&queue_metadata_key)
        //         .arg(message_guid)
        //         .query_async::<String>(&mut connection)
        //         .await?;
        //
        //     return Ok(num.parse().expect("Could not parse redis id"));
        // }
        //
        // let message_id = cmd("HINCRBY")
        //     .arg(&queue_metadata_key)
        //     .arg("counter")
        //     .arg(1)
        //     .query_async::<u64>(&mut connection)
        //     .await?;
        //
        // cmd("ZADD")
        //     .arg(&queue_key)
        //     .arg("NX")
        //     .arg(message_id)
        //     .arg(&data)
        //     .query_async::<()>(&mut connection)
        //     .await?;
        //
        // cmd("HSET")
        //     .arg(&queue_metadata_key)
        //     .arg(message_guid)
        //     .arg(message_id)
        //     .query_async::<()>(&mut connection)
        //     .await?;
        //
        // cmd("EXPIRE")
        //     .arg(&queue_key)
        //     .arg(2678400)
        //     .query_async::<()>(&mut connection)
        //     .await?;
        //
        // cmd("EXPIRE")
        //     .arg(&queue_metadata_key)
        //     .arg(2678400)
        //     .query_async::<()>(&mut connection)
        //     .await?;
        //
        // let time = SystemTime::now();
        // let time_in_millis: u64 = time.duration_since(UNIX_EPOCH)?.as_secs();
        //
        // cmd("ZADD")
        //     .arg(&queue_total_index_key)
        //     .arg("NX")
        //     .arg(time_in_millis)
        //     .arg(&queue_key)
        //     .query_async::<()>(&mut connection)
        //     .await?;
        //
        // // notifies the message availability manager
        // let queue_name = format!("{}::{}", address.name(), address.device_id());
        // if let Some(listener) = self.listeners.lock().await.get(&queue_name) {
        //     listener.lock().await.handle_new_messages_available().await;
        // }
        //
        // Ok(message_id)
    }
}
