#[cfg(test)]
use crate::test_utils::random_string;
use anyhow::Result;
use deadpool_redis::{Config, Runtime};
use libsignal_core::{DeviceId, ProtocolAddress};
use redis::cmd;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;
use uuid::Uuid;

const PRESENCE_EXPIRATION_SECONDS: u16 = 660;

/**
 * A displaced presence listener is notified when a specific client's presence has been displaced because the same
 * client opened a newer connection to the Signal service.
 */
#[async_trait::async_trait]
pub trait DisplacedPresenceListener: Send + 'static {
    async fn handle_displacement(&mut self, connected_elsewhere: bool);
}

#[derive(Debug)]
pub struct ClientPresenceManager<T: DisplacedPresenceListener> {
    displacement_listeners: HashMap<String, Arc<Mutex<T>>>,
    pool: deadpool_redis::Pool,
    manager_id: String,
    #[cfg(test)]
    pub test_key: String,
}

impl<T: DisplacedPresenceListener> Clone for ClientPresenceManager<T> {
    fn clone(&self) -> Self {
        Self {
            displacement_listeners: self.displacement_listeners.clone(),
            pool: self.pool.clone(),
            manager_id: self.manager_id.clone(),
            #[cfg(test)]
            test_key: self.test_key.clone(),
        }
    }
}

impl<T: DisplacedPresenceListener> ClientPresenceManager<T> {
    pub fn connect() -> Self {
        let _ = dotenv::dotenv();
        let redis_url = std::env::var("REDIS_URL").expect("Unable to read REDIS_URL .env var");
        let redis_config = Config::from_url(redis_url);
        let redis_pool: deadpool_redis::Pool = redis_config
            .create_pool(Some(Runtime::Tokio1))
            .expect("Could not create pool");

        #[cfg(not(test))]
        return Self {
            displacement_listeners: HashMap::new(),
            pool: redis_pool,
            manager_id: Uuid::new_v4().to_string(),
        };

        #[cfg(test)]
        Self {
            displacement_listeners: HashMap::new(),
            pool: redis_pool,
            manager_id: Uuid::new_v4().to_string(),
            test_key: random_string(8),
        }
    }

    pub async fn set_present(
        &mut self,
        address: &ProtocolAddress,
        displacement_listener: Arc<Mutex<T>>,
    ) -> Result<()> {
        let presence_key = self.get_presence_key(address);

        if self.displacement_listeners.contains_key(&presence_key) {
            self.displace_presence(&presence_key, true).await?;
        }

        self.displacement_listeners
            .insert(presence_key.clone(), displacement_listener);

        let mut connection = self.pool.get().await?;
        let set_key = self.get_set_key();

        cmd("SADD")
            .arg(&set_key)
            .arg(&presence_key)
            .query_async::<()>(&mut connection)
            .await?;

        cmd("SET")
            .arg(&presence_key)
            .arg(&self.manager_id)
            .arg("EX")
            .arg(PRESENCE_EXPIRATION_SECONDS)
            .query_async::<()>(&mut connection)
            .await?;

        Ok(())
    }

    #[allow(dead_code)]
    async fn renew_presence(&mut self, address: &ProtocolAddress) -> Result<()> {
        let presence_key = self.get_presence_key(address);
        let mut connection = self.pool.get().await?;

        // If there is no presence key connected to the manager, we cannot renew it.
        if cmd("GET")
            .arg(&presence_key)
            .query_async::<String>(&mut connection)
            .await?
            == self.manager_id
        {
            cmd("EXPIRE")
                .arg(&presence_key)
                .arg(PRESENCE_EXPIRATION_SECONDS)
                .query_async::<()>(&mut connection)
                .await?;
        }
        Ok(())
    }

    async fn clear_presence(&mut self, presence_key: &str) -> Result<bool> {
        self.displacement_listeners.remove(presence_key);
        let mut connection = self.pool.get().await?;

        if cmd("GET")
            .arg(presence_key)
            .query_async::<Option<String>>(&mut connection)
            .await?
            .is_some()
        {
            cmd("DEL")
                .arg(presence_key)
                .query_async::<String>(&mut connection)
                .await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn disconnect_all_presence(
        &mut self,
        account_uuid: &str,
        device_ids: Vec<DeviceId>,
    ) -> Result<u8> {
        let mut presence_keys: Vec<String> = Vec::new();
        let account_uuid = account_uuid.to_string();

        for device_id in device_ids {
            let addr = ProtocolAddress::new(account_uuid.clone(), device_id);
            let presence_key = self.get_presence_key(&addr);
            if self.is_locally_present(&addr)
                && self.displace_presence(&presence_key, false).await?
            {
                presence_keys.push(presence_key.to_string());
            }
        }
        Ok(presence_keys.len() as u8)
    }

    pub fn is_locally_present(&self, address: &ProtocolAddress) -> bool {
        self.displacement_listeners
            .contains_key(&self.get_presence_key(address))
    }

    #[allow(dead_code)]
    async fn disconnect_presence(&mut self, address: &ProtocolAddress) -> Result<u8> {
        self.disconnect_all_presence(address.name(), vec![address.device_id()])
            .await
    }

    #[cfg(test)]
    pub async fn disconnect_presence_in_test(&mut self, address: &ProtocolAddress) -> Result<u8> {
        self.disconnect_all_presence(address.name(), vec![address.device_id()])
            .await
    }

    async fn displace_presence(
        &mut self,
        presence_key: &str,
        connected_elsewhere: bool,
    ) -> Result<bool> {
        if let Some(displacement_listener) = self.displacement_listeners.get(presence_key) {
            displacement_listener
                .lock()
                .await
                .handle_displacement(connected_elsewhere)
                .await;
        }

        self.clear_presence(presence_key).await
    }

    pub async fn is_present(&self, address: &ProtocolAddress) -> Result<bool> {
        let mut connection = self.pool.get().await?;
        let is_present = cmd("EXISTS")
            .arg(self.get_presence_key(address))
            .query_async::<bool>(&mut connection)
            .await?;
        Ok(is_present)
    }

    fn get_presence_key(&self, address: &ProtocolAddress) -> String {
        #[cfg(not(test))]
        return format!("presence::{{{}::{}}}", address.name(), address.device_id());
        #[cfg(test)]
        format!(
            "{}presence::{{{}::{}}}",
            self.test_key,
            address.name(),
            address.device_id()
        )
    }

    fn get_set_key(&mut self) -> String {
        #[cfg(not(test))]
        return format!("presence::clients::{}", self.manager_id);
        #[cfg(test)]
        format!("{}presence::clients::{}", self.test_key, self.manager_id)
    }
}

#[cfg(test)]
mod client_presence_manager_test {
    use super::*;
    use crate::test_utils::message_cache::{generate_uuid, teardown};

    pub struct MockWebSocketConnection {
        pub evoke_handle_displacement: bool,
    }

    impl MockWebSocketConnection {
        fn new() -> Self {
            MockWebSocketConnection {
                evoke_handle_displacement: false,
            }
        }
    }

    #[async_trait::async_trait]
    impl DisplacedPresenceListener for MockWebSocketConnection {
        async fn handle_displacement(&mut self, _: bool) {
            self.evoke_handle_displacement = true;
        }
    }

    #[tokio::test]
    async fn test_handle_displacement() {
        let mut manager: ClientPresenceManager<MockWebSocketConnection> =
            ClientPresenceManager::connect();
        let mut connection = manager.pool.get().await.unwrap();
        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));
        let account_id = generate_uuid();
        let device_id = DeviceId::from(2);
        let addr = ProtocolAddress::new(account_id, device_id);

        manager.set_present(&addr, websocket.clone()).await.unwrap();
        manager.set_present(&addr, websocket.clone()).await.unwrap();

        let presence_keys = cmd("SMEMBERS")
            .arg(manager.get_set_key())
            .query_async::<Vec<String>>(&mut connection)
            .await
            .unwrap();

        for presence_key in presence_keys {
            assert_eq!(presence_key, manager.get_presence_key(&addr));
        }
        let presence_key = manager.get_presence_key(&addr);
        let is_handle_displacement_invoked = manager
            .displacement_listeners
            .get(&presence_key)
            .unwrap()
            .lock()
            .await
            .evoke_handle_displacement;

        teardown(&manager.test_key, connection).await;

        assert!(is_handle_displacement_invoked);
    }

    #[tokio::test]
    async fn test_set_present() {
        let mut manager: ClientPresenceManager<MockWebSocketConnection> =
            ClientPresenceManager::connect();
        let mut connection = manager.pool.get().await.unwrap();
        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));
        let account_id = generate_uuid();
        let device_id = DeviceId::from(2);
        let addr = ProtocolAddress::new(account_id, device_id);

        manager.set_present(&addr, websocket).await.unwrap();

        let presence_keys = cmd("SMEMBERS")
            .arg(manager.get_set_key())
            .query_async::<Vec<String>>(&mut connection)
            .await
            .unwrap();

        for presence_key in presence_keys {
            assert_eq!(presence_key, manager.get_presence_key(&addr));
        }

        let presence_key = manager.get_presence_key(&addr);

        let is_handle_displacement_invoked = manager
            .displacement_listeners
            .get(&presence_key)
            .unwrap()
            .lock()
            .await
            .evoke_handle_displacement;

        teardown(&manager.test_key, connection).await;

        assert!(!is_handle_displacement_invoked);
    }

    #[tokio::test]
    async fn test_disconnect_all_presence() {
        let mut manager: ClientPresenceManager<MockWebSocketConnection> =
            ClientPresenceManager::connect();
        let connection = manager.pool.get().await.unwrap();

        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));
        let account_id = Uuid::new_v4().to_string();
        let device_id = DeviceId::from(1);
        let addr = ProtocolAddress::new(account_id, device_id);

        manager.set_present(&addr, websocket).await.unwrap();

        let removed = manager
            .disconnect_all_presence(addr.name(), vec![device_id])
            .await
            .unwrap();

        teardown(&manager.test_key, connection).await;

        assert_eq!(removed, 1);
    }

    #[tokio::test]
    async fn test_is_present() {
        let mut manager: ClientPresenceManager<MockWebSocketConnection> =
            ClientPresenceManager::connect();
        let connection = manager.pool.get().await.unwrap();
        let websocket = Arc::new(Mutex::new(MockWebSocketConnection::new()));
        let account_id = Uuid::new_v4().to_string();
        let device_id = DeviceId::from(1);
        let addr = ProtocolAddress::new(account_id.clone(), device_id);

        manager.set_present(&addr, websocket).await.unwrap();

        let is_present = manager.is_present(&addr).await.unwrap();

        teardown(&manager.test_key, connection).await;

        assert!(is_present);
    }
}
