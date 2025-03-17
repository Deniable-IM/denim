use super::connection::{ClientConnection, ConnectionMap, WebSocketConnection};
use crate::database::SignalDatabase;
use axum::extract::ws::Message;
use common::signalservice::WebSocketMessage;
use common::websocket::wsstream::WSStream;
use futures_util::stream::{SplitStream, StreamExt};
use libsignal_core::ProtocolAddress;
use prost::{bytes::Bytes, Message as PMessage};
use std::{collections::HashMap, fmt::Debug, sync::Arc};
use tokio::sync::Mutex;

/*const WS_ENDPOINTS: [&str; 24] = [
    "v4/attachments/form/upload",
    "v1/config",
    "v1/certificate/delivery",
    "v1/devices",
    "v2/directory/auth",
    "v1/certificate/auth/group",
    "v1/archives/auth",
    "v1/archives/auth/read",
    "v1/archives/upload/form",
    "v1/archives/media/upload/form",
    "v1/devices/link",
    "v1/messages",
    "v1/messages/multi_recipient",
    "v2/accounts/phone_number_discoverability",
    "v1/profile",
    "v1/archives",
    "v1/archives/media",
    "v1/archives/media/batch",
    "v1/archives/media/delete",
    "v1/devices/capabilities",
    "v1/messages/report",
    "v1/archives/backupid",
    "v1/archives/keys",
    "v1/storage/auth",
];*/

#[derive(Default, Debug)]
pub struct WebSocketManager<T, U>
where
    T: WSStream<Message, axum::Error> + Debug,
    U: SignalDatabase,
{
    sockets: ConnectionMap<T, U>,
}

impl<T, U> Clone for WebSocketManager<T, U>
where
    T: WSStream<Message, axum::Error> + Debug,
    U: SignalDatabase,
{
    fn clone(&self) -> Self {
        Self {
            sockets: Arc::clone(&self.sockets),
        }
    }
}

impl<T, U> WebSocketManager<T, U>
where
    T: WSStream<Message, axum::Error> + Debug,
    U: SignalDatabase,
{
    pub fn new() -> Self {
        Self {
            sockets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn listen(
        &mut self,
        connection: WebSocketConnection<T, U>,
        mut receiver: SplitStream<T>,
    ) {
        let address = connection.protocol_address();
        let connection = Arc::new(Mutex::new(connection));
        self.register_new_connection(address.clone(), connection.clone())
            .await;

        tokio::spawn({
            let mut self_clone = self.clone();

            async move {
                while let Some(res) = receiver.next().await {
                    let Ok(msg) = res else {
                        println!("WebSocketManager recv ERROR: {}", res.unwrap_err());
                        self_clone
                            .close_connection(&address, connection.clone())
                            .await;
                        break;
                    };

                    match msg {
                        Message::Binary(b) => {
                            let msg = match WebSocketMessage::decode(Bytes::from(b)) {
                                Ok(msg) => msg,
                                Err(err) => {
                                    println!("WebSocketManager ERROR - Message::Binary: {}", err);
                                    let _ = connection
                                        .lock()
                                        .await
                                        .close_reason(1007, "Badly formatted")
                                        .await;
                                    break;
                                }
                            };
                            connection.lock().await.on_receive(msg).await.unwrap();
                        }
                        Message::Text(t) => {
                            println!("Message '{}' from '{}'", t, address);
                            println!("replying...");
                            let _ = connection.lock().await.send(Message::Text(t)).await;
                            println!("sent!");
                        }
                        Message::Close(_) => {
                            break;
                        }
                        _ => {}
                    }
                }

                if self_clone
                    .close_connection(&address, connection)
                    .await
                    .is_none()
                {
                    println!("WebSocketManager: Client was already removed from Manager!")
                };
            }
        });
    }

    /// Keep new connection open
    pub async fn register_new_connection(
        &mut self,
        address: ProtocolAddress,
        connection: ClientConnection<T, U>,
    ) {
        self.sockets.lock().await.insert(address, connection);
    }

    pub async fn is_connected(&mut self, address: &ProtocolAddress) -> bool {
        self.sockets.lock().await.contains_key(address)
    }

    pub async fn get(&self, address: &ProtocolAddress) -> Option<ClientConnection<T, U>> {
        self.sockets.lock().await.get(address).cloned()
    }

    pub async fn get_mut(&mut self, address: &ProtocolAddress) -> Option<ClientConnection<T, U>> {
        self.sockets.lock().await.get_mut(address).cloned()
    }

    async fn close_connection(
        &mut self,
        address: &ProtocolAddress,
        connection: ClientConnection<T, U>,
    ) -> Option<ClientConnection<T, U>> {
        connection.lock().await.close().await;
        print!("Connection closed!");
        self.sockets.lock().await.remove(address)
    }
}

#[cfg(test)]
mod test {
    use crate::{
        managers::{
            state::SignalServerState,
            websocket::connection::{test::create_connection, ClientConnection},
        },
        test_utils::websocket::{MockDB, MockSocket},
    };
    use axum::extract::ws::Message;
    use common::websocket::net_helper;
    use prost::Message as PMessage;

    #[tokio::test]
    async fn test_insert() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, _sender, _receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.listen(ws, mreceiver).await;

        assert!(mgr.is_connected(&address).await)
    }

    #[tokio::test]
    async fn test_none_msg() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, sender, _receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.listen(ws, mreceiver).await;

        let ws: ClientConnection<MockSocket, MockDB> = mgr.get(&address).await.unwrap();

        assert!(ws.lock().await.is_active());

        drop(sender);

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert!(!mgr.is_connected(&address).await);
        assert!(!ws.lock().await.is_active());
    }

    #[tokio::test]
    async fn test_error_msg() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, sender, _receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.listen(ws, mreceiver).await;

        let ws: ClientConnection<MockSocket, MockDB> = mgr.get(&address).await.unwrap();

        assert!(ws.lock().await.is_active());

        sender
            .send(Err(axum::Error::new("Error message")))
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert!(!mgr.is_connected(&address).await);
        assert!(!ws.lock().await.is_active());
    }

    #[tokio::test]
    async fn test_close_msg() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, sender, _receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.listen(ws, mreceiver).await;

        let ws: ClientConnection<MockSocket, MockDB> = mgr.get(&address).await.unwrap();

        assert!(ws.lock().await.is_active());

        sender.send(Ok(Message::Close(None))).await.unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        assert!(!mgr.is_connected(&address).await);
        assert!(!ws.lock().await.is_active());
    }

    #[tokio::test]
    async fn test_text_msg() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, sender, mut receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.listen(ws, mreceiver).await;

        let ws: ClientConnection<MockSocket, MockDB> = mgr.get(&address).await.unwrap();

        assert!(ws.lock().await.is_active());

        sender
            .send(Ok(Message::Text("hello".to_string())))
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        assert!(!receiver.is_empty());
        assert!(mgr.is_connected(&address).await);
        assert!(ws.lock().await.is_active());

        match receiver.recv().await.unwrap() {
            Message::Text(x) => assert!(x == "hello", "Expected 'hello'"),
            _ => panic!("Did not receive text message"),
        }
    }

    #[tokio::test]
    async fn test_binary_decode_error() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, sender, mut receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.listen(ws, mreceiver).await;

        let ws: ClientConnection<MockSocket, MockDB> = mgr.get(&address).await.unwrap();

        assert!(ws.lock().await.is_active());

        sender
            .send(Ok(Message::Binary("hello".as_bytes().to_vec())))
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        assert!(!receiver.is_empty());
        assert!(!mgr.is_connected(&address).await);
        assert!(!ws.lock().await.is_active());

        match receiver.recv().await.unwrap() {
            Message::Close(Some(x)) => {
                assert!(x.code == 1007);
                assert!(x.reason == "Badly formatted");
            }
            _ => panic!("Did not receive close message"),
        }
    }

    #[ignore = "not implemented"]
    #[tokio::test]
    async fn test_binary_decode_ok() {
        let msg = r#"
        {
            "messages":[
                {
                    "regularPayload": {
                        "signalMessage": {
                            "type": 1,
                            "destinationDeviceId": 3,
                            "destinationRegistrationId": 22,
                            "content": "aGVsbG8="
                        }
                    },
                    "chunks": [],
                    "ballast": 0
                }
            ],
            "online": false,
            "urgent": true,
            "timestamp": 1730217386
        }
        "#;

        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, sender, receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.listen(ws, mreceiver).await;

        let ws: ClientConnection<MockSocket, MockDB> = mgr.get(&address).await.unwrap();

        assert!(ws.lock().await.is_active());

        let id = net_helper::generate_req_id();
        let req = net_helper::create_request(
            id,
            "PUT",
            "v1/messages/aaa?story=false",
            vec![],
            Some(msg.as_bytes().to_vec()),
        );

        sender
            .send(Ok(Message::Binary(req.encode_to_vec())))
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

        assert!(!receiver.is_empty());
        assert!(mgr.is_connected(&address).await);
        assert!(ws.lock().await.is_active());

        // TODO: check that response is ok
        todo!()
    }
}
