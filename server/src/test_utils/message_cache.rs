use crate::availability_listener::AvailabilityListener;
use common::signalservice::Envelope;
use redis::cmd;
use uuid::Uuid;

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

pub struct MockWebSocketConnection {
    pub evoked_handle_new_messages: bool,
    pub evoked_handle_messages_persisted: bool,
}

impl MockWebSocketConnection {
    pub(crate) fn new() -> Self {
        MockWebSocketConnection {
            evoked_handle_new_messages: false,
            evoked_handle_messages_persisted: false,
        }
    }
}

#[async_trait::async_trait]
impl AvailabilityListener for MockWebSocketConnection {
    async fn send_cached(&mut self) -> bool {
        self.evoked_handle_new_messages = true;
        true
    }

    async fn send_persisted(&mut self) -> bool {
        self.evoked_handle_messages_persisted = true;
        true
    }
}
