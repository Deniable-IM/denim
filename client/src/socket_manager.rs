use base64::{prelude::BASE64_STANDARD, Engine as _};
use common::signalservice::{web_socket_message, WebSocketMessage};
use common::websocket::{connection_state::ConnectionState, wsstream::WSStream};
use futures_util::lock::Mutex;
use futures_util::{Sink, SinkExt, Stream, StreamExt};
use prost::{bytes::Bytes, Message as PMessage};
use rustls::pki_types::CertificateDer;
use rustls::{ClientConfig, RootCertStore};
use rustls_pemfile::certs;
use socket2::{SockRef, TcpKeepalive};
use std::fs::File;
use std::io::BufReader;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::broadcast::{self, Receiver, Sender};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, WebSocketConfig};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{
    client_async_tls_with_config, tungstenite, Connector, MaybeTlsStream, WebSocketStream,
};
use tungstenite::protocol::frame::coding::CloseCode;

fn rustls_cfg(ca_file_path: &str) -> Result<ClientConfig, String> {
    let ca_file = File::open(ca_file_path).map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(ca_file);

    let certs: Vec<CertificateDer<'static>> = certs(&mut reader)
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let mut root_store = RootCertStore::empty();
    for cert in certs {
        root_store
            .add(cert)
            .map_err(|_| "Invalid Certificate".to_string())?;
    }

    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(config)
}

type TLSWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
#[derive(Debug)]
pub struct SignalStream(TLSWebSocket);
impl SignalStream {
    pub fn new(w: TLSWebSocket) -> Self {
        Self(w)
    }
}

impl Stream for SignalStream {
    type Item = Result<Message, tungstenite::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.0).poll_next(cx)
    }
}

impl Sink<Message> for SignalStream {
    type Error = tungstenite::Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_ready(cx)
    }

    fn start_send(mut self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        Pin::new(&mut self.0).start_send(item)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Pin::new(&mut self.0).poll_close(cx)
    }
}

#[async_trait::async_trait]
impl WSStream<Message, tungstenite::Error> for SignalStream {
    async fn recv(&mut self) -> Option<Result<Message, tungstenite::Error>> {
        self.0.next().await
    }

    async fn send(&mut self, msg: Message) -> Result<(), tungstenite::Error> {
        self.0.send(msg).await
    }

    async fn close(mut self) -> Result<(), tungstenite::Error> {
        self.0.close(None).await
    }
}

pub async fn signal_ws_connect(
    tls_cert: &Option<String>,
    url: &str,
    username: &str,
    password: &str,
) -> Result<TLSWebSocket, String> {
    let url = format!("{}/v1/websocket", url.replace("http", "ws"));
    let mut req = url
        .into_client_request()
        .map_err(|_| "Failed to convert to client request".to_string())?;

    // TODO: do some smarter header handling than this, now both middleware and this defines headers
    // Create Signal auth
    let headers = [
        (
            "Authorization",
            format!(
                "Basic {}",
                BASE64_STANDARD.encode(format!("{}:{}", username, password))
            ),
        ),
        ("Accept-Encoding", "gzip".to_string()),
        ("user-agent", "Signal Clone Client".to_string()),
        ("x-signal-agent", "OWA".to_string()),
        ("sec-websocket-extensions", "permessage-deflate".to_string()), // TODO: does not actually support this: https://github.com/snapview/tungstenite-rs/pull/426
        ("x-signal-receive-stories", "false".to_string()),
    ];

    for (key, value) in headers.iter() {
        req.headers_mut().insert(
            *key,
            value
                .parse()
                .map_err(|_| format!("failed to add {} header", key))?,
        );
    }

    let addr = req.uri().host().ok_or("No Host".to_string())?;
    let port = req.uri().port_u16().ok_or("No Port".to_string())?;
    let stream = TcpStream::connect((addr, port))
        .await
        .map_err(|e| e.to_string())?;

    {
        let keepalive = TcpKeepalive::new()
            .with_time(std::time::Duration::from_secs(10))
            .with_interval(Duration::from_secs(10));
        let socket_ref = SockRef::from(&stream);
        socket_ref
            .set_tcp_keepalive(&keepalive)
            .map_err(|_| "Failed to set keepalive".to_string())?;
    }

    let connector = if let Some(path) = tls_cert {
        let tls_cfg = rustls_cfg(path)?;
        Some(Connector::Rustls(Arc::new(tls_cfg)))
    } else {
        None
    };

    let config = WebSocketConfig {
        max_frame_size: Some(0x210000),
        ..Default::default()
    };

    let res = client_async_tls_with_config(req, stream, Some(config), connector).await;
    let (ws, _) = res.map_err(|_| "Failed to connect to server".to_string())?;
    Ok(ws)
}

type MessageType = WebSocketMessage;
#[derive(Debug)]
pub struct SocketManager<T: WSStream<Message, tungstenite::Error> + std::fmt::Debug> {
    next_id: Arc<AtomicU64>,
    request_delegater: Sender<MessageType>,
    receiver: Receiver<MessageType>,
    connection: Arc<Mutex<ConnectionState<T, Message>>>,
}

impl<T: WSStream<Message, tungstenite::Error> + std::fmt::Debug> Clone for SocketManager<T> {
    fn clone(&self) -> Self {
        Self {
            next_id: self.next_id.clone(),
            request_delegater: self.request_delegater.clone(),
            receiver: self.request_delegater.subscribe(),
            connection: self.connection.clone(),
        }
    }
}

impl<T: WSStream<Message, tungstenite::Error> + std::fmt::Debug> SocketManager<T> {
    pub fn new(broadcast_capacity: usize) -> Self {
        let (tx, rx) = broadcast::channel::<MessageType>(broadcast_capacity);
        Self {
            next_id: Arc::new(AtomicU64::new(0)),
            request_delegater: tx,
            receiver: rx,
            connection: Arc::new(Mutex::new(ConnectionState::Closed)),
        }
    }

    pub async fn is_active(&self) -> bool {
        self.connection.lock().await.is_active()
    }

    pub async fn set_stream(&mut self, stream: T) -> Result<(), String> {
        let mut guard = self.connection.lock().await;
        if guard.is_active() {
            return Err("A Stream is already present!".to_string());
        }
        let (sender, mut receiver) = stream.split();
        *guard = ConnectionState::Active(sender);

        let mut mgr = self.clone();

        // handle incoming messages
        tokio::spawn(async move {
            while let Some(res) = receiver.next().await {
                let Ok(msg) = res else {
                    println!("SocketManager recv ERROR: {}", res.unwrap_err());
                    mgr.close().await;
                    break;
                };
                match msg {
                    Message::Text(_) => {
                        mgr.close().await;
                        break;
                    }
                    Message::Binary(x) => {
                        let msg = match WebSocketMessage::decode(Bytes::from(x)) {
                            Ok(msg) => msg,
                            Err(err) => {
                                println!("WebSocketManager ERROR - Message::Binary: {}", err);
                                mgr.close_reason(1007.into(), "Badly formatted".into())
                                    .await;
                                break;
                            }
                        };
                        if let Err(err) = mgr.request_delegater.send(msg) {
                            println!("Error while notifying subscribers: {}", err);
                            mgr.close().await;
                            break;
                        }
                    }
                    Message::Close(_) => {
                        mgr.close().await;
                        break;
                    }
                    _ => {
                        continue;
                    }
                }
            }
        });
        Ok(())
    }

    pub async fn close(&mut self) {
        let mut guard = self.connection.lock().await;
        if let ConnectionState::Active(mut socket) =
            std::mem::replace(&mut *guard, ConnectionState::Closed)
        {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code: CloseCode::Normal,
                    reason: "Goodbye".into(),
                })))
                .await;
        }
    }

    async fn close_reason(&mut self, code: CloseCode, reason: String) {
        let mut guard = self.connection.lock().await;
        if let ConnectionState::Active(mut socket) =
            std::mem::replace(&mut *guard, ConnectionState::Closed)
        {
            let _ = socket
                .send(Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                })))
                .await;
        }
    }

    pub fn next_id(&mut self) -> u64 {
        self.next_id.fetch_add(1, Ordering::SeqCst)
    }

    pub fn subscribe(&self) -> Receiver<MessageType> {
        self.request_delegater.subscribe()
    }

    async fn wait_for_id(&mut self, id: u64) -> Result<MessageType, String> {
        loop {
            let msg = self.receiver.recv().await.map_err(|e| e.to_string())?;
            match msg.r#type() {
                web_socket_message::Type::Response => {
                    let _id = msg
                        .response
                        .as_ref()
                        .expect("No Response in response")
                        .id
                        .expect("No ID");
                    if _id == id {
                        return Ok(msg);
                    }
                }
                web_socket_message::Type::Request => {
                    let _id = msg
                        .request
                        .as_ref()
                        .expect("No Request in response")
                        .id
                        .expect("No ID");
                    if _id == id {
                        return Ok(msg);
                    }
                }
                _ => continue,
            }
        }
    }

    pub async fn send(&mut self, id: u64, message: MessageType) -> Result<MessageType, String> {
        let mut guard = self.connection.lock().await;
        match *guard {
            ConnectionState::Active(ref mut sender) => {
                sender
                    .send(Message::Binary(message.encode_to_vec()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            ConnectionState::Closed => return Err("Closed".to_string()),
        }
        drop(guard);
        self.wait_for_id(id).await
    }

    pub async fn send_response(&mut self, message: MessageType) -> Result<(), String> {
        let mut guard = self.connection.lock().await;
        match *guard {
            ConnectionState::Active(ref mut sender) => {
                sender
                    .send(Message::Binary(message.encode_to_vec()))
                    .await
                    .map_err(|e| e.to_string())?;
            }
            ConnectionState::Closed => return Err("Closed".to_string()),
        }
        drop(guard);

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::SocketManager;
    use std::{fmt::Debug, time::Duration};

    use axum::http::StatusCode;
    use common::{
        signalservice::WebSocketMessage,
        websocket::{
            net_helper::{create_request, create_response},
            wsstream::WSStream,
        },
    };
    use futures_util::{stream::Stream, Sink};
    use prost::{bytes::Bytes, Message as PMessage};
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::sync::mpsc::{channel, Receiver, Sender};
    use tokio_tungstenite::tungstenite::{Error, Message};

    type TestManagerSetup = (
        SocketManager<MockSocket>,
        MockSocket,
        Sender<Result<Message, Error>>,
        Receiver<Message>,
    );

    #[derive(Debug)]
    pub struct MockSocket {
        client_sender: Receiver<Result<Message, Error>>,
        client_receiver: Sender<Message>,
    }

    impl MockSocket {
        pub fn new() -> (Self, Sender<Result<Message, Error>>, Receiver<Message>) {
            let (send_to_socket, client_sender) = channel(10); // Queue for test -> socket
            let (client_receiver, receive_from_socket) = channel(10); // Queue for socket -> test

            (
                Self {
                    client_sender,
                    client_receiver,
                },
                send_to_socket,
                receive_from_socket,
            )
        }
    }

    impl Stream for MockSocket {
        type Item = Result<Message, Error>;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Pin::new(&mut self.get_mut().client_sender).poll_recv(cx)
        }
    }

    impl Sink<Message> for MockSocket {
        type Error = Error;

        fn poll_ready(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
            self.client_receiver
                .try_send(item)
                .map_err(|_| Error::ConnectionClosed)
        }

        fn poll_flush(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }

        fn poll_close(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Ok(()))
        }
    }

    #[async_trait::async_trait]
    impl WSStream<Message, Error> for MockSocket {
        async fn recv(&mut self) -> Option<Result<Message, Error>> {
            self.client_sender.recv().await
        }

        async fn send(&mut self, msg: Message) -> Result<(), Error> {
            self.client_receiver
                .send(msg)
                .await
                .map_err(|_| Error::ConnectionClosed)
        }

        async fn close(self) -> Result<(), Error> {
            Ok(())
        }
    }

    fn get_socket_mgr() -> TestManagerSetup {
        let mgr: SocketManager<MockSocket> = SocketManager::new(5);

        let (ms, s, r) = MockSocket::new();

        (mgr, ms, s, r)
    }

    fn unwrap_binary(msg: Message) -> WebSocketMessage {
        match msg {
            Message::Binary(x) => {
                WebSocketMessage::decode(Bytes::from(x)).expect("Expected WebSocketMessage")
            }
            _ => panic!("Expected Binary Message"),
        }
    }

    #[tokio::test]
    async fn test_connect() {
        let (mut mgr, ms, s, r) = get_socket_mgr();
        mgr.set_stream(ms).await.unwrap();
        assert!(mgr.is_active().await);
    }

    #[ignore = "Will not be implemented in this project"]
    #[tokio::test]
    async fn test_keepalive_is_sending() {
        let (mut mgr, ms, s, mut r) = get_socket_mgr();
        mgr.set_stream(ms).await;
        let res = tokio::time::timeout(Duration::from_millis(300), r.recv()).await;
        let msg = res.expect("Expected a message").expect("Expected Some");
        let msg = unwrap_binary(msg);
        assert!(msg.request.unwrap().path.unwrap() == "/v1/keepalive")
    }

    #[tokio::test]
    async fn test_send_success() {
        let (mut mgr, ms, s, r) = get_socket_mgr();
        mgr.set_stream(ms).await;
        let id = mgr.next_id();
        let mut mgr_c = mgr.clone();
        let hndl = tokio::spawn(async move {
            let req_msg = create_request(id, "PUT", "/v1/messages/a", vec![], None);
            let res = tokio::time::timeout(Duration::from_millis(300), mgr_c.send(id, req_msg))
                .await
                .expect("Time out")
                .expect("Expected A Message");
            assert!(res.response.unwrap().status.unwrap() == 200);
        });
        let send_msg = Ok(Message::Binary(
            create_response(id, StatusCode::OK, vec![], None)
                .unwrap()
                .encode_to_vec(),
        ));
        tokio::time::timeout(Duration::from_millis(300), s.send(send_msg))
            .await
            .expect("time out")
            .expect("failed to send");
        hndl.await.expect("Thread Panic");
    }
}
