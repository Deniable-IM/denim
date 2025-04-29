use axum::async_trait;
use libsignal_protocol::SignalProtocolError;

pub mod chunk;
pub mod constants;

#[async_trait(?Send)]
pub trait DeniableSendingBuffer {
    async fn get_outgoing_message(&mut self) -> Result<(u32, Vec<u8>, i32), SignalProtocolError>;
    async fn set_outgoing_message(
        &mut self,
        message_id: Option<u32>,
        chunk_count: i32,
        outgoing_message: Vec<u8>,
    ) -> Result<(), SignalProtocolError>;
    async fn remove_outgoing_message(&mut self, message_id: u32)
        -> Result<(), SignalProtocolError>;
}
