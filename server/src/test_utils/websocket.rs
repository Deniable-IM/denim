use crate::{
    account::{Account, Device},
    storage::database::SignalDatabase,
};
use anyhow::Result;
use axum::{async_trait, extract::ws::Message, Error};
use common::websocket::wsstream::WSStream;
use common::{
    signalservice::Envelope,
    web_api::{DeviceCapabilityType, DevicePreKeyBundle, UploadPreKey, UploadSignedPreKey},
};
use futures_util::{stream::Stream, Sink};
use libsignal_core::{Aci, Pni, ProtocolAddress, ServiceId};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::{channel, Receiver, Sender};

#[derive(Clone)]
pub struct MockDB {}

#[async_trait]
impl SignalDatabase for MockDB {
    async fn store_signed_pre_key(
        &self,
        _: &UploadSignedPreKey,
        _: &ProtocolAddress,
    ) -> Result<()> {
        todo!()
    }

    async fn store_pq_signed_pre_key(
        &self,
        _: &UploadSignedPreKey,
        _: &ProtocolAddress,
    ) -> Result<()> {
        todo!()
    }
    async fn add_device(&self, _: &ServiceId, _: &Device) -> Result<()> {
        todo!()
    }
    async fn get_all_devices(&self, _: &ServiceId) -> Result<Vec<Device>> {
        todo!()
    }
    async fn get_device(&self, _: &ProtocolAddress) -> Result<Device> {
        todo!()
    }
    async fn delete_device(&self, _: &ProtocolAddress) -> Result<()> {
        todo!()
    }

    async fn add_account(&self, _: &Account) -> Result<()> {
        todo!()
    }
    async fn get_account(&self, _: &ServiceId) -> Result<Account> {
        todo!()
    }
    async fn get_account_from_phonenumber_without_devices(&self, _: &str) -> Result<Account> {
        todo!()
    }
    async fn update_account_aci(&self, _: &ServiceId, _: Aci) -> Result<()> {
        todo!()
    }
    async fn update_account_pni(&self, _: &ServiceId, _: Pni) -> Result<()> {
        todo!()
    }

    async fn delete_account(&self, _: &ServiceId) -> Result<()> {
        todo!()
    }

    async fn get_device_capabilities(
        &self,
        _: &ProtocolAddress,
    ) -> Result<Vec<DeviceCapabilityType>> {
        todo!()
    }

    async fn get_all_device_capabilities(
        &self,
        _: &ServiceId,
    ) -> Result<Vec<(i32, DeviceCapabilityType)>> {
        todo!()
    }

    async fn add_used_device_link_token(&self, _: String) -> Result<()> {
        todo!()
    }

    async fn push_message_queue(&self, _: &ProtocolAddress, _: Vec<Envelope>) -> Result<()> {
        todo!()
    }
    async fn pop_msg_queue(&self, _: &ProtocolAddress) -> Result<Vec<Envelope>> {
        todo!()
    }
    async fn store_key_bundle(&self, _: &DevicePreKeyBundle, _: &ProtocolAddress) -> Result<()> {
        todo!()
    }
    async fn get_key_bundle(&self, _: &ProtocolAddress) -> Result<DevicePreKeyBundle> {
        todo!()
    }

    async fn get_one_time_ec_pre_key_count(&self, _: &ServiceId) -> Result<u32> {
        todo!()
    }

    async fn get_one_time_pq_pre_key_count(&self, _: &ServiceId) -> Result<u32> {
        todo!()
    }

    async fn store_one_time_ec_pre_keys(
        &self,
        _: Vec<UploadPreKey>,
        _: &ProtocolAddress,
    ) -> Result<()> {
        todo!()
    }

    async fn store_one_time_pq_pre_keys(
        &self,
        _: Vec<UploadSignedPreKey>,
        _: &ProtocolAddress,
    ) -> Result<()> {
        todo!()
    }

    async fn get_one_time_ec_pre_key(&self, _: &ProtocolAddress) -> Result<Option<UploadPreKey>> {
        todo!()
    }

    async fn get_one_time_pq_pre_key(&self, _: &ProtocolAddress) -> Result<UploadSignedPreKey> {
        todo!()
    }

    async fn count_messages(&self, _: &ProtocolAddress) -> Result<u32> {
        todo!()
    }

    async fn get_messages(&self, _: &ProtocolAddress) -> Result<Vec<Envelope>> {
        todo!()
    }

    async fn delete_messages(&self, _: &ProtocolAddress) -> Result<Vec<Envelope>> {
        Ok(Vec::new())
    }
}

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

    fn poll_ready(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: Message) -> Result<(), Self::Error> {
        self.client_receiver
            .try_send(item)
            .map_err(|_| Error::new("Send failed".to_string()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

#[async_trait::async_trait]
impl WSStream<Message, axum::Error> for MockSocket {
    async fn recv(&mut self) -> Option<Result<Message, Error>> {
        self.client_sender.recv().await
    }

    async fn send(&mut self, msg: Message) -> Result<(), Error> {
        self.client_receiver
            .send(msg)
            .await
            .map_err(|_| Error::new("Send failed".to_string()))
    }

    async fn close(self) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::MockSocket;
    use axum::extract::ws::Message;
    use futures_util::{SinkExt, StreamExt};

    #[tokio::test]
    async fn test_mock_echo() {
        let (mock, sender, mut receiver) = MockSocket::new();
        let (mut ms, mut mr) = mock.split();

        tokio::spawn(async move {
            if let Some(Ok(Message::Text(x))) = mr.next().await {
                ms.send(Message::Text(x)).await
            } else {
                panic!("Expected Text Message");
            }
        });

        sender
            .send(Ok(Message::Text("hello".to_string())))
            .await
            .unwrap();

        match receiver.recv().await.unwrap() {
            Message::Text(x) => assert!(x == "hello", "Expected 'hello' in test_mock"),
            _ => panic!("Did not receive text message"),
        }
    }

    #[tokio::test]
    async fn test_mock_dropped_sender() {
        let (mock, sender, _) = MockSocket::new();

        let (_, mut mr) = mock.split();

        drop(sender);

        if mr.next().await.is_some() {
            panic!("Expected None");
        }
    }
}
