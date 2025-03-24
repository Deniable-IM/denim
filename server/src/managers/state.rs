use super::{
    account_manager::AccountManager,
    client_presence_manager::ClientPresenceManager,
    key_manager::KeyManager,
    manager::Manager,
    message::{message_cache::MessageCache, messages_manager::MessagesManager},
    websocket::{connection::WebSocketConnection, websocket_manager::WebSocketManager},
};
#[cfg(test)]
use crate::test_utils::websocket::{MockDB, MockSocket};
use crate::{storage::database::SignalDatabase, storage::postgres::PostgresDatabase};
use axum::extract::ws::Message;
use common::websocket::wsstream::WSStream;
use std::fmt::Debug;

#[derive(Debug)]
pub struct SignalServerState<T, U>
where
    T: SignalDatabase,
    U: WSStream<Message, axum::Error> + Debug,
{
    pub db: T,
    pub websocket_manager: WebSocketManager<U, T>,
    pub account_manager: AccountManager<T>,
    pub key_manager: KeyManager<T>,
    pub message_manager: MessagesManager<T, WebSocketConnection<U, T>>,
    pub client_presence_manager: ClientPresenceManager<WebSocketConnection<U, T>>,
    pub message_cache: MessageCache<WebSocketConnection<U, T>>,
}

impl<T, U> Manager for SignalServerState<T, U>
where
    T: SignalDatabase,
    U: WSStream<Message, axum::Error> + Debug,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T, U> Clone for SignalServerState<T, U>
where
    T: SignalDatabase,
    U: WSStream<Message, axum::Error> + Debug,
{
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            websocket_manager: self.websocket_manager.clone(),
            account_manager: self.account_manager.clone(),
            key_manager: self.key_manager.clone(),
            message_manager: self.message_manager.clone(),
            client_presence_manager: self.client_presence_manager.clone(),
            message_cache: self.message_cache.clone(),
        }
    }
}

impl<U> SignalServerState<PostgresDatabase, U>
where
    U: WSStream<Message, axum::Error> + Debug,
{
    pub async fn new() -> Self {
        SignalServerState::connect("DATABASE_URL").await
    }

    pub async fn connect(connection_str: &str) -> Self {
        let db = PostgresDatabase::connect(connection_str.to_string()).await;
        let cache = MessageCache::connect();
        Self {
            db: db.clone(),
            websocket_manager: WebSocketManager::new(),
            account_manager: AccountManager::new(db.clone()),
            key_manager: KeyManager::new(db.clone()),
            message_manager: MessagesManager::new(db, cache.clone()),
            client_presence_manager: ClientPresenceManager::connect(),
            message_cache: cache,
        }
    }
}

#[cfg(test)]
impl Default for SignalServerState<MockDB, MockSocket> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl SignalServerState<MockDB, MockSocket> {
    pub fn new() -> Self {
        let db = MockDB {};
        let cache = MessageCache::connect();

        Self {
            db: db.clone(),
            websocket_manager: WebSocketManager::new(),
            account_manager: AccountManager::new(db.clone()),
            key_manager: KeyManager::new(db.clone()),
            message_manager: MessagesManager::new(db, cache.clone()),
            client_presence_manager: ClientPresenceManager::connect(),
            message_cache: cache,
        }
    }
}
