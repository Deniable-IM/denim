use crate::{
    account::AuthenticatedDevice,
    database::SignalDatabase,
    managers::{client_presence_manager::DisplacedPresenceListener, state::SignalServerState},
    message_cache::MessageAvailabilityListener,
    server::{handle_keepalive, handle_put_messages},
};
use axum::extract::ws::WebSocket;
use axum::Error;
use axum::{
    extract::ws::{CloseFrame, Message},
    http::{StatusCode, Uri},
};
use common::signalservice::{
    web_socket_message, Envelope, WebSocketMessage, WebSocketRequestMessage,
    WebSocketResponseMessage,
};
use common::websocket::connection_state::ConnectionState;
use common::websocket::net_helper::{
    create_request, create_response, current_millis, generate_req_id, unpack_messages,
    PathExtractor,
};
use common::websocket::wsstream::WSStream;
use futures_util::{stream::SplitSink, Sink, SinkExt, Stream};
use libsignal_core::{ProtocolAddress, ServiceId, ServiceIdKind};
use prost::Message as PMessage;
use std::{
    collections::HashMap,
    fmt::Debug,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::SystemTimeError,
};
use tokio::sync::Mutex;

#[derive(Debug)]
pub enum UserIdentity {
    ProtocolAddress(ProtocolAddress),
    AuthenticatedDevice(Box<AuthenticatedDevice>),
}

#[derive(Debug)]
pub struct WebSocketConnection<W: WSStream<Message, Error> + Debug, DB: SignalDatabase> {
    identity: UserIdentity,
    socket_address: SocketAddr,
    ws: ConnectionState<W, Message>,
    pending_requests: HashMap<u64, String>,
    state: SignalServerState<DB, W>,
}

impl<W: WSStream<Message, Error> + Debug + Send + 'static, DB: SignalDatabase + Send + 'static>
    WebSocketConnection<W, DB>
{
    pub fn new(
        identity: UserIdentity,
        socket_addr: SocketAddr,
        ws: SplitSink<W, Message>,
        state: SignalServerState<DB, W>,
    ) -> Self {
        Self {
            identity,
            socket_address: socket_addr,
            ws: ConnectionState::Active(ws),
            pending_requests: HashMap::new(),
            state,
        }
    }

    pub fn socket_address(&self) -> SocketAddr {
        self.socket_address
    }

    pub fn protocol_address(&self) -> ProtocolAddress {
        match &self.identity {
            UserIdentity::AuthenticatedDevice(x) => x.get_protocol_address(ServiceIdKind::Aci),
            UserIdentity::ProtocolAddress(y) => y.clone(),
        }
    }

    pub async fn send_message(&mut self, message: Envelope) -> Result<(), String> {
        let msg = self
            .create_message(message.clone())
            .map_err(|_| "Time went backwards".to_string())?;
        match self.send(Message::Binary(msg.encode_to_vec())).await {
            Ok(_) => {
                self.state
                    .message_manager
                    .delete(
                        &self.protocol_address(),
                        vec![message.server_guid().to_owned()],
                    )
                    .await
                    .map_err(|err| format!("{}", err))?;
                Ok(())
            }
            Err(err) => Err(format!("{}", err)),
        }
    }

    pub async fn send_messages(&mut self, cached_only: bool) -> bool {
        let Ok(envelopes) = self
            .state
            .message_manager
            .get_messages_for_device(&self.protocol_address(), cached_only)
            .await
        else {
            println!("Failed to fetch messages");
            return false;
        };

        println!("Sending {} messages", envelopes.len());

        for envelope in envelopes {
            let _ = self
                .send_message(envelope)
                .await
                .map_err(|e| println!("{}", e));
        }
        true
    }

    fn create_message(
        &mut self,
        mut message: Envelope,
    ) -> Result<WebSocketMessage, SystemTimeError> {
        let id = generate_req_id();
        message.ephemeral = None; // was false
        message.story = Some(false); // TODO: needs to handled in handle_request instead
        let msg = create_request(
            id,
            "PUT",
            "/api/v1/message",
            vec![
                "X-Signal-Key: false".to_string(),
                format!("X-Signal-Timestamp: {}", current_millis()?),
            ],
            Some(message.encode_to_vec()),
        );
        self.pending_requests
            .insert(id, message.server_guid.expect("This is always some"));
        Ok(msg)
    }

    pub async fn close(&mut self) {
        if let ConnectionState::Active(ref mut socket) = self.ws {
            if let Err(e) = socket.close().await {
                println!("WebSocketConnection ERROR: {e}");
            }
        }
        self.ws = ConnectionState::Closed;
    }

    pub async fn close_reason(&mut self, code: u16, reason: &str) -> Result<(), String> {
        let fut = self.send(Message::Close(Some(CloseFrame {
            code,
            reason: reason.to_string().into(),
        })));

        if let Err(x) = fut.await {
            return Err(format!("{}", x));
        }
        Ok(())
    }

    pub fn is_active(&self) -> bool {
        self.ws.is_active()
    }

    pub async fn send(&mut self, msg: Message) -> Result<(), axum::Error> {
        match self.ws {
            ConnectionState::Active(ref mut socket) => socket.send(msg).await,
            ConnectionState::Closed => Err(axum::Error::new("Connection is closed")),
        }
    }

    async fn send_queue_empty(&mut self) -> bool {
        let id = generate_req_id();
        let Ok(time) = current_millis() else {
            return false;
        };
        let msg = create_request(
            id,
            "PUT",
            "/api/v1/queue/empty",
            vec![format!("X-Signal-Timestamp: {}", time)],
            None,
        );
        self.send(Message::Binary(msg.encode_to_vec()))
            .await
            .is_ok()
    }

    pub async fn on_receive(&mut self, proto_message: WebSocketMessage) -> Result<(), String> {
        match proto_message.r#type() {
            web_socket_message::Type::Unknown => self.close_reason(1007, "Badly formatted").await,
            web_socket_message::Type::Request => match proto_message.request {
                Some(req) => self.handle_request(req).await,
                None => self.close_reason(1007, "Badly formatted").await,
            },
            web_socket_message::Type::Response => match proto_message.response {
                Some(res) => self.handle_response(res).await,
                None => self.close_reason(1007, "Badly formatted").await,
            },
        }
    }

    async fn handle_request(&mut self, request_msq: WebSocketRequestMessage) -> Result<(), String> {
        let msq_id = request_msq.id.ok_or("Request id was not present")?;
        let path = request_msq
            .path
            .clone()
            .ok_or("Request path was not present")?;
        if !path.starts_with("/v1/messages") && !path.starts_with("/v1/keepalive") {
            self.send(Message::Binary(
                create_response(msq_id, StatusCode::INTERNAL_SERVER_ERROR, vec![], None)?
                    .encode_to_vec(),
            ))
            .await
            .map_err(|err| format!("{}", err))?;

            return Err(format!("Incorret path: {}", request_msq.path()));
        }

        if request_msq.path().starts_with("/v1/keepalive") {
            let user = match &self.identity {
                UserIdentity::ProtocolAddress(pa) => {
                    todo!()
                }
                UserIdentity::AuthenticatedDevice(auth_device) => auth_device,
            };

            return match handle_keepalive(&self.state, user).await {
                Ok(()) => self
                    .send(Message::Binary(
                        create_response(msq_id, StatusCode::OK, vec![], None)?.encode_to_vec(),
                    ))
                    .await
                    .map_err(|err| err.to_string()),
                _ => self.close_reason(1000, "OK").await, //This is extremely wrong and stupid, but it is what happens on line 54 in KeepAliveController.java
            };
        }

        let res = match &self.identity {
            UserIdentity::ProtocolAddress(_) => {
                todo!("We do not support protocal addresses yet!")
            }
            UserIdentity::AuthenticatedDevice(authenticated_device) => {
                handle_put_messages(
                    &self.state,
                    authenticated_device,
                    &ServiceId::parse_from_service_id_string(
                        PathExtractor::new(
                            &request_msq
                                .path()
                                .parse::<Uri>()
                                .map_err(|err| err.to_string())?,
                        )?
                        .extract::<String>(2)?
                        .as_str(),
                    )
                    .ok_or("Could not parse uri to service id")?,
                    unpack_messages(request_msq.body.clone())?,
                )
                .await
            }
        };
        match res {
            Ok(res) => self
                .send(Message::Binary(
                    create_response(
                        msq_id,
                        StatusCode::OK,
                        vec![],
                        Some(serde_json::to_string(&res).unwrap().as_bytes().to_vec()),
                    )?
                    .encode_to_vec(),
                ))
                .await
                .map_err(|err| err.to_string()),
            Err(_) => self
                .send(Message::Binary(
                    create_response(msq_id, StatusCode::INTERNAL_SERVER_ERROR, vec![], None)?
                        .encode_to_vec(),
                ))
                .await
                .map_err(|err| err.to_string()),
        }
    }

    async fn handle_response(
        &mut self,
        response_msq: WebSocketResponseMessage,
    ) -> Result<(), String> {
        // TODO: Figure out how to get the ID of the message so the message manager can delete it.
        // The ID is on the envelope and is called server_guid
        //
        // TODO This should be fixed, since the current implementation is wrong

        if !self
            .pending_requests
            .contains_key(&response_msq.id.ok_or("Response message was not present")?)
        {
            return Err("pending_requests did not have the expected request".to_string());
        }

        self.state
            .message_manager
            .delete(
                &self.protocol_address(),
                vec![self.pending_requests
                    [&response_msq.id.ok_or("Response message was not present")?]
                    .clone()],
            )
            .await
            .map(|_| ())
            .map_err(|err| err.to_string())?;

        self.pending_requests
            .remove(&response_msq.id.ok_or("Request id was not present")?)
            .ok_or("Could not remove pending requests".to_string())
            .map(|_| ())
    }
}

#[async_trait::async_trait]
impl<T, U> MessageAvailabilityListener for WebSocketConnection<T, U>
where
    T: WSStream<Message, Error> + Debug + 'static,
    U: SignalDatabase,
{
    async fn handle_new_messages_available(&mut self) -> bool {
        if !(self.is_active() && self.send_messages(true).await) {
            return false;
        }
        self.send_queue_empty().await
    }

    async fn handle_messages_persisted(&mut self) -> bool {
        if !(self.is_active() && self.send_messages(false).await) {
            return false;
        }
        self.send_queue_empty().await
    }
}

#[async_trait::async_trait]
impl<T, U> DisplacedPresenceListener for WebSocketConnection<T, U>
where
    T: WSStream<Message, axum::Error> + Debug + 'static,
    U: SignalDatabase,
{
    async fn handle_displacement(&mut self, connected_elsewhere: bool) {
        let fut = if connected_elsewhere {
            self.close_reason(4409, "Connected elsewhere")
        } else {
            self.close_reason(1000, "OK")
        };
        if let Err(x) = fut.await {
            println!("Displacement Close Error: {}", x);
        };
    }
}

#[derive(Debug)]
pub struct SignalWebSocket(WebSocket);
impl SignalWebSocket {
    pub fn new(w: WebSocket) -> Self {
        Self(w)
    }
}

impl Stream for SignalWebSocket {
    type Item = Result<Message, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Stream::poll_next(Pin::new(&mut self.0), cx)
    }
}

impl Sink<Message> for SignalWebSocket {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Sink::poll_ready(Pin::new(&mut self.0), cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Sink::start_send(Pin::new(&mut self.0), item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Sink::poll_flush(Pin::new(&mut self.0), cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Sink::poll_close(Pin::new(&mut self.0), cx)
    }
}

#[async_trait::async_trait]
impl WSStream<Message, Error> for SignalWebSocket {
    async fn recv(&mut self) -> Option<Result<Message, Error>> {
        self.recv().await
    }
    async fn send(&mut self, msg: Message) -> Result<(), Error> {
        SinkExt::send(self, msg).await
    }
    async fn close(self) -> Result<(), Error> {
        self.close().await
    }
}

pub type ClientConnection<T, U> = Arc<Mutex<WebSocketConnection<T, U>>>;
pub type ConnectionMap<T, U> = Arc<Mutex<HashMap<ProtocolAddress, ClientConnection<T, U>>>>;

#[cfg(test)]
pub(crate) mod test {
    use super::{UserIdentity, WebSocketConnection};
    use crate::{
        account::Device,
        database::SignalDatabase,
        managers::state::SignalServerState,
        postgres::PostgresDatabase,
        test_utils::{
            message_cache::teardown,
            user::new_authenticated_device,
            websocket::{MockDB, MockSocket},
        },
    };
    use axum::{extract::ws::Message, http::StatusCode, Error};
    use base64::prelude::{Engine as _, BASE64_STANDARD};
    use common::signalservice::{Envelope, WebSocketMessage, WebSocketRequestMessage};
    use common::websocket::net_helper::{create_request, create_response};
    use futures_util::{stream::SplitStream, StreamExt};
    use hmac::digest::consts::False;
    use libsignal_core::Aci;
    use prost::{bytes::Bytes, Message as PMessage};

    use std::time::Duration;
    use std::{net::SocketAddr, str::FromStr, sync::Arc};
    use tokio::sync::mpsc::{Receiver, Sender};
    use tokio::time::sleep;

    fn make_envelope() -> Envelope {
        Envelope {
            ephemeral: None,
            story: Some(false),
            content: Some("Hello".as_bytes().to_vec()),
            ..Default::default()
        }
    }

    pub async fn create_connection<DB: SignalDatabase>(
        socket_addr: &str,
        state: SignalServerState<DB, MockSocket>,
    ) -> (
        WebSocketConnection<MockSocket, DB>,
        Sender<Result<Message, Error>>,
        Receiver<Message>,
        SplitStream<MockSocket>,
    ) {
        let (mock, sender, receiver) = MockSocket::new();
        let (msender, mreceiver) = mock.split();
        let who = SocketAddr::from_str(socket_addr).unwrap();
        let auth_device = new_authenticated_device();

        let ws = WebSocketConnection::new(
            UserIdentity::AuthenticatedDevice(Box::new(auth_device)),
            who,
            msender,
            state,
        );

        (ws, sender, receiver, mreceiver)
    }

    #[tokio::test]
    async fn test_send_and_recv() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, sender, mut receiver, mut mreceiver) =
            create_connection("127.0.0.1:4042", state).await;

        sender.send(Ok(Message::Text("hello".to_string()))).await;

        match mreceiver.next().await {
            Some(Ok(Message::Text(x))) => assert!(x == "hello", "message was not hello"),
            _ => panic!("Unexpected error when receiving msg"),
        }

        client.send(Message::Text("hello back".to_string())).await;

        if receiver.is_empty() {
            panic!("receiver was empty when it was expected not to be")
        }

        match receiver.recv().await {
            Some(Message::Text(x)) => assert!(x == "hello back", "message was not 'hello back'"),
            _ => panic!("Unexpected error when receiving msg"),
        }
    }

    #[tokio::test]
    async fn test_close() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, _, _, _) = create_connection("127.0.0.1:4042", state).await;

        assert!(client.is_active());
        client.close().await;
        assert!(!client.is_active());
    }

    #[tokio::test]
    async fn test_close_reason() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, _, mut receiver, _) = create_connection("127.0.0.1:4042", state).await;
        assert!(client.is_active());
        client.close_reason(666, "test").await;
        client.close().await;
        assert!(!client.is_active());

        assert!(!receiver.is_empty());
        match receiver.recv().await {
            Some(Message::Close(Some(x))) => {
                assert!(x.code == 666);
                assert!(x.reason == "test");
            }
            _ => panic!("Did not receive close frame"),
        }
    }

    #[tokio::test]
    async fn test_send_message() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, sender, mut receiver, mreceiver) =
            create_connection("127.0.0.1:4042", state).await;
        let mut env = make_envelope();
        env.server_guid = Some("abs".to_string());
        client.send_message(env.clone()).await;

        assert!(!receiver.is_empty());

        let msg = match receiver.recv().await {
            Some(Message::Binary(x)) => WebSocketMessage::decode(Bytes::from(x))
                .expect("unexpected error in decode websocket message"),
            _ => panic!("Did not receive close frame"),
        };

        assert!(msg.request.is_some());
        assert!(!client.pending_requests.is_empty());
        let req = msg.request.unwrap();

        assert!(req.verb.unwrap() == "PUT");
        assert!(req.path.unwrap() == "/api/v1/message");
        assert!(req.headers.len() == 2);
        assert!(req.headers[0] == "X-Signal-Key: false");
        assert!(req.headers[1].starts_with("X-Signal-Timestamp:"));
        assert!(req.body.unwrap() == env.encode_to_vec());
    }

    #[tokio::test]
    async fn test_on_receive_request() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, sender, receiver, mreceiver) =
            create_connection("127.0.0.1:4042", state).await;
        let msg = r#"
        {
            "messages":[
                {
                    "type": 1,
                    "destinationDeviceId": 3,
                    "destinationRegistrationId": 22,
                    "content": "aGVsbG8="
                }
            ],
            "online": false,
            "urgent": true,
            "timestamp": 1730217386
        }
        "#
        .as_bytes()
        .to_vec();

        client
            .on_receive(create_request(
                1,
                "PUT",
                &format!("/v1/messages/{}", client.protocol_address().name()),
                vec![],
                Some(msg),
            ))
            .await
            .unwrap();
    }

    #[ignore = "This in currently not testable"]
    #[tokio::test]
    async fn test_on_receive_response() {
        let state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, sender, receiver, mreceiver) =
            create_connection("127.0.0.1:4042", state).await;
        client
            .on_receive(create_response(1, StatusCode::OK, vec![], None).unwrap())
            .await
            .unwrap();
    }
    #[tokio::test]
    async fn test_alice_sends_msg_to_bob() {
        let mut state =
            SignalServerState::<PostgresDatabase, MockSocket>::connect("DATABASE_URL_TEST").await;
        let (alice, alice_sender, mut alice_receiver, alice_mreceiver) =
            create_connection("127.0.0.1:4042", state.clone()).await;
        let (bob, bob_sender, mut bob_receiver, bob_mreceiver) =
            create_connection("127.0.0.1:4042", state.clone()).await;

        match &alice.identity {
            UserIdentity::ProtocolAddress(protocol_address) => todo!(),
            UserIdentity::AuthenticatedDevice(authenticated_device) => state
                .db
                .add_account(authenticated_device.account())
                .await
                .unwrap(),
        }
        let UserIdentity::AuthenticatedDevice(auth_device) = &bob.identity else {
            unreachable!("Create connection should make an auth device");
        };
        state.db.add_account(auth_device.account()).await.unwrap();
        let reg_id = auth_device.device().registration_id();

        let alice_address = alice.protocol_address();
        let bob_address = bob.protocol_address();

        state.websocket_manager.insert(alice, alice_mreceiver).await;
        state.websocket_manager.insert(bob, bob_mreceiver).await;
        let ws_alice = state.websocket_manager.get(&alice_address).await.unwrap();
        let ws_bob = state.websocket_manager.get(&bob_address).await.unwrap();
        state
            .message_manager
            .add_message_availability_listener(&alice_address, ws_alice)
            .await;
        state
            .message_manager
            .add_message_availability_listener(&bob_address, ws_bob.clone())
            .await;

        let sending_msg = format!(
            r#"
        {{
            "messages":[
                {{
                    "type": 1,
                    "destinationDeviceId": {},
                    "destinationRegistrationId": {},
                    "content": "aGVsbG8="
                }}
            ],
            "online": false,
            "urgent": true,
            "timestamp": 1730217386
        }}
        "#,
            bob_address.device_id(),
            reg_id,
        )
        .as_bytes()
        .to_vec();

        alice_sender
            .send(Ok(Message::Binary(
                create_request(
                    0,
                    "PUT",
                    &format!("/v1/messages/{}", bob_address.name()),
                    vec![],
                    Some(sending_msg),
                )
                .encode_to_vec(),
            )))
            .await
            .unwrap();

        sleep(Duration::from_millis(100));

        assert!(ws_bob.lock().await.is_active());
        let alice_response =
            tokio::time::timeout(Duration::from_millis(100), alice_receiver.recv())
                .await
                .expect("Alice did not receive in time");
        let Some(Message::Binary(alice_binary)) = alice_response else {
            panic!("Expected Binary");
        };

        let alice_response = WebSocketMessage::decode(Bytes::from(alice_binary))
            .unwrap()
            .response;

        assert_eq!(alice_response.unwrap().status.unwrap(), 200);

        let res = tokio::time::timeout(Duration::from_millis(100), bob_receiver.recv())
            .await
            .expect("Bob did not recieve message");

        state
            .db
            .delete_account(
                &Aci::parse_from_service_id_string(alice_address.name())
                    .unwrap()
                    .into(),
            )
            .await
            .unwrap();
        state
            .db
            .delete_account(
                &Aci::parse_from_service_id_string(bob_address.name())
                    .unwrap()
                    .into(),
            )
            .await
            .unwrap();

        let message = match res {
            Some(Message::Binary(msg)) => WebSocketMessage::decode(Bytes::from(msg)).unwrap(),
            _ => panic!("Expected binary message"),
        };

        assert_eq!(
            BASE64_STANDARD.encode(
                Envelope::decode(Bytes::from(message.request.unwrap().body.unwrap()))
                    .unwrap()
                    .content
                    .unwrap()
            ),
            "aGVsbG8="
        );
    }

    // this is more of a integration test and should maybe be somewhere else
    #[tokio::test]
    async fn test_handle_new_messages_available() {
        let mut state = SignalServerState::<MockDB, MockSocket>::new();
        let (ws, sender, mut receiver, mreceiver) =
            create_connection("127.0.0.1:4043", state.clone()).await;
        let address = ws.protocol_address();
        let mut mgr = state.websocket_manager.clone();
        mgr.insert(ws, mreceiver).await;
        let listener = mgr.get(&address).await.unwrap();
        let mut env = make_envelope();

        state
            .message_manager
            .add_message_availability_listener(&address, listener)
            .await;
        state.message_manager.insert(&address, &mut env).await;

        let msg = match receiver.recv().await {
            Some(Message::Binary(x)) => {
                WebSocketMessage::decode(Bytes::from(x)).expect("Did not unwrap ws message")
            }
            _ => panic!("Did not receive anything"),
        };

        let queue = match receiver.recv().await {
            Some(Message::Binary(x)) => {
                WebSocketMessage::decode(Bytes::from(x)).expect("Did not unwrap ws message (queue)")
            }
            _ => panic!("Did not receive anything"),
        };

        teardown(
            &state.message_cache.test_key,
            state.message_cache.get_connection().await.unwrap(),
        )
        .await;

        assert!(msg.request.is_some());
        assert!(queue.request.is_some());

        let req = msg.request.unwrap();
        let queue_req = queue.request.unwrap();

        assert!(queue_req.path.unwrap() == "/api/v1/queue/empty");
        assert!(req.verb.unwrap() == "PUT");
        assert!(req.path.unwrap() == "/api/v1/message");
        assert!(req.headers.len() == 2);
        assert!(req.headers[0] == "X-Signal-Key: false");
        assert!(req.headers[1].starts_with("X-Signal-Timestamp:"));
        assert!(req.body.unwrap() == env.encode_to_vec());
    }

    #[tokio::test]
    async fn test_keepalive_for_present_device() {
        let mut state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, sender, mut receiver, mut mreceiver) =
            create_connection("127.0.0.1:4042", state.clone()).await;

        let addr = client.protocol_address();
        let websocket = Arc::new(tokio::sync::Mutex::new(client));

        state
            .client_presence_manager
            .set_present(&addr, websocket.clone())
            .await;
        websocket
            .lock()
            .await
            .on_receive(create_request(1, "PUT", "/v1/keepalive", vec![], None))
            .await
            .unwrap();

        let message_response = receiver.recv().await.unwrap();
        let message = WebSocketMessage::decode(message_response.into_data().as_slice()).unwrap();
        let response = message.response.unwrap();

        assert!(state.client_presence_manager.is_locally_present(&addr));
        assert_eq!(message.r#type.unwrap(), 2);
        assert_eq!(response.status.unwrap(), 200);
        assert_eq!(response.message.unwrap(), "OK");
    }

    #[tokio::test]
    async fn test_keepalive_for_unpresent_device() {
        let mut state = SignalServerState::<MockDB, MockSocket>::new();
        let (mut client, sender, mut receiver, mut mreceiver) =
            create_connection("127.0.0.1:4042", state.clone()).await;

        state
            .clone()
            .client_presence_manager
            .disconnect_presence_in_test(&client.protocol_address())
            .await;

        client
            .on_receive(create_request(1, "PUT", "/v1/keepalive", vec![], None))
            .await
            .unwrap();

        let message_response = receiver.recv().await.unwrap();
        let message = WebSocketMessage::decode(message_response.into_data().as_slice()).unwrap();
        let response = message.response.unwrap();

        println!(
            "Is locally present: {}",
            state
                .client_presence_manager
                .is_locally_present(&client.protocol_address())
        );
        println!(
            "Status: {}, Message: {}",
            response.status.unwrap(),
            response.message.unwrap()
        );

        assert!(!state
            .client_presence_manager
            .is_locally_present(&client.protocol_address()));
        assert_eq!(message.r#type.unwrap(), 2);
        assert_eq!(response.status.unwrap(), 200);
    }
}
