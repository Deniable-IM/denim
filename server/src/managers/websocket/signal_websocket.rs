use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::Error;
use common::websocket::wsstream::WSStream;
use futures_util::{Sink, SinkExt, Stream};
use std::{
    fmt::Debug,
    pin::Pin,
    task::{Context, Poll},
};

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
