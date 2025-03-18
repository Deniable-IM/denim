use axum::async_trait;
use libsignal_core::ProtocolAddress;
use std::{collections::HashMap, sync::Arc};
use tokio::sync::Mutex;

pub type ListenerMap<T> = Arc<Mutex<HashMap<String, Arc<Mutex<T>>>>>;

#[async_trait]
pub trait AvailabilityListener: Send + 'static {
    async fn send_cached(&mut self) -> bool;
    async fn send_persisted(&mut self) -> bool;
}

pub async fn notify_cached<T>(listeners: ListenerMap<T>, address: &ProtocolAddress)
where
    T: AvailabilityListener,
{
    let queue_name = format!("{}::{}", address.name(), address.device_id());
    if let Some(listener) = listeners.lock().await.get(&queue_name) {
        listener.lock().await.send_cached().await;
    }
}

pub async fn notify_persisted<T>(listeners: ListenerMap<T>, address: &ProtocolAddress)
where
    T: AvailabilityListener,
{
    let queue_name = format!("{}::{}", address.name(), address.device_id());
    if let Some(listener) = listeners.lock().await.get(&queue_name) {
        listener.lock().await.send_persisted().await;
    }
}
