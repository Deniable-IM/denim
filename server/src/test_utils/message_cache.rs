use std::sync::Arc;

use crate::availability_listener::AvailabilityListener;
use common::{
    signalservice::Envelope,
    web_api::{DeniablePayload, DenimChunk, PreKeyRequest, PreKeyResponse, SignalMessage},
};
use redis::cmd;
use tokio::sync::{Mutex, Notify};
use uuid::Uuid;

use super::key::{new_identity_key, new_pre_key_response_itmes};

pub fn generate_uuid() -> String {
    Uuid::new_v4().to_string()
}

pub async fn teardown(key: &str, mut con: deadpool_redis::Connection) {
    let pattern = format!("{}*", key);

    let mut cursor = 0;
    loop {
        let (new_cursor, keys): (u64, Vec<String>) = cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern.clone())
            .query_async(&mut con)
            .await
            .expect("Teardown scan failed");

        if !keys.is_empty() {
            cmd("DEL")
                .arg(&keys)
                .query_async::<u8>(&mut con)
                .await
                .expect("Teardown delete failed");
        }

        cursor = new_cursor;
        if cursor == 0 {
            break;
        }
    }
}

pub fn generate_envelope(uuid: &str) -> Envelope {
    Envelope {
        server_guid: Some(uuid.to_string()),
        ..Default::default()
    }
}

pub fn generate_chunk() -> DenimChunk {
    DenimChunk {
        ..Default::default()
    }
}

pub enum DeniablePayloadType {
    SignalMessage,
    Envelope,
    KeyRequest,
    KeyResponse,
}

pub fn generate_payload(payload_type: DeniablePayloadType) -> DeniablePayload {
    match payload_type {
        DeniablePayloadType::SignalMessage => {
            DeniablePayload::SignalMessage(SignalMessage::default())
        }
        DeniablePayloadType::Envelope => {
            DeniablePayload::Envelope(generate_envelope(&generate_uuid()))
        }
        DeniablePayloadType::KeyRequest => DeniablePayload::KeyRequest(PreKeyRequest {
            service_id: "1".to_string(),
        }),
        DeniablePayloadType::KeyResponse => DeniablePayload::KeyResponse(PreKeyResponse::new(
            new_identity_key(),
            new_pre_key_response_itmes(),
        )),
    }
}

pub struct MockWebSocketConnection {
    pub evoked_notify: Arc<Notify>,
    pub evoked_handle_new_messages: Arc<Mutex<bool>>,
    pub evoked_handle_messages_persisted: Arc<Mutex<bool>>,
}

impl MockWebSocketConnection {
    pub(crate) fn new() -> Self {
        MockWebSocketConnection {
            evoked_notify: Arc::new(Notify::new()),
            evoked_handle_new_messages: Arc::new(Mutex::new(false)),
            evoked_handle_messages_persisted: Arc::new(Mutex::new(false)),
        }
    }
}

#[async_trait::async_trait]
impl AvailabilityListener for MockWebSocketConnection {
    async fn send_cached(&mut self) -> bool {
        let mut evoked = self.evoked_handle_new_messages.lock().await;
        *evoked = true;
        self.evoked_notify.notify_waiters();
        *evoked
    }

    async fn send_persisted(&mut self) -> bool {
        let mut evoked = self.evoked_handle_messages_persisted.lock().await;
        *evoked = true;
        self.evoked_notify.notify_waiters();
        *evoked
    }
}
