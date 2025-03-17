use axum::async_trait;

#[async_trait]
pub trait AvailabilityListener: Send {
    async fn send_cached(&mut self) -> bool;
    async fn send_persisted(&mut self) -> bool;
}
