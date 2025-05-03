use super::{buffer::Buffer, chunk_cache::ChunkCache, payload_cache::PayloadCache};
use crate::{availability_listener::AvailabilityListener, managers::manager::Manager};
use anyhow::{Ok, Result};
use common::deniable::chunk::{ChunkType, Chunker};
use common::signalservice::Envelope;
use common::web_api::{DeniablePayload, DenimChunk, DenimMessage};
use libsignal_core::ProtocolAddress;
use prost::Message as PMessage;
use uuid::Uuid;

#[derive(Debug)]
pub struct DenIMManager<T>
where
    T: AvailabilityListener,
{
    chunk_cache: ChunkCache<T>,
    payload_cache: PayloadCache<T>,
    chunker: Chunker,
}

impl<T> Clone for DenIMManager<T>
where
    T: AvailabilityListener,
{
    fn clone(&self) -> Self {
        Self {
            chunk_cache: self.chunk_cache.clone(),
            payload_cache: self.payload_cache.clone(),
            chunker: self.chunker.clone(),
        }
    }
}

impl<T> Manager for DenIMManager<T>
where
    T: AvailabilityListener,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T> DenIMManager<T>
where
    T: AvailabilityListener,
{
    pub fn new(chunk_cache: ChunkCache<T>, payload_cache: PayloadCache<T>) -> Self {
        Self {
            chunk_cache,
            payload_cache,
            chunker: Chunker::default(),
        }
    }

    pub async fn create_denim_message(
        &self,
        receiver: &ProtocolAddress,
        regular_payload: Envelope,
    ) -> Result<DenimMessage> {
        let free_space = self.get_free_space_in_bytes(regular_payload.encoded_len() as f32);
        let chunks = self.dequeue_payload_data(receiver, free_space).await?;
        let q = self.chunker.q;

        let denim_message = DenimMessage {
            regular_payload: common::web_api::RegularPayload::Envelope(regular_payload),
            chunks,
            counter: None, //TODO find out what this is used for, is only used for server -> client
            q: Some(q),
            ballast: Vec::new(),
        };

        Ok(denim_message)
    }

    pub fn get_free_space_in_bytes(&self, regular_payload_size: f32) -> usize {
        self.chunker.get_free_space_in_bytes(regular_payload_size)
    }

    pub async fn get_incoming_chunks(&self, sender: &ProtocolAddress) -> Result<Vec<DenimChunk>> {
        self.chunk_cache
            .get_all_chunks(sender, Buffer::Sender)
            .await
    }

    pub async fn set_incoming_chunks(
        &self,
        sender: &ProtocolAddress,
        chunks: Vec<DenimChunk>,
    ) -> Result<u64> {
        let mut count = 0;
        for chunk in chunks {
            count += self
                .chunk_cache
                .insert(sender, Buffer::Sender, &chunk, &Uuid::new_v4().to_string())
                .await?
        }
        Ok(count)
    }

    pub async fn get_outgoing_chunks(&self, receiver: &ProtocolAddress) -> Result<Vec<DenimChunk>> {
        self.chunk_cache
            .get_all_chunks(receiver, Buffer::Receiver)
            .await
    }

    pub async fn set_outgoing_chunks(
        &self,
        receiver: &ProtocolAddress,
        chunks: Vec<DenimChunk>,
    ) -> Result<u64> {
        let mut count = 0;
        for chunk in chunks {
            count += self
                .chunk_cache
                .insert(
                    receiver,
                    Buffer::Receiver,
                    &chunk,
                    &Uuid::new_v4().to_string(),
                )
                .await?
        }
        Ok(count)
    }

    pub async fn get_deniable_payloads_raw(
        &self,
        receiver: &ProtocolAddress,
    ) -> Result<Vec<Vec<u8>>> {
        let data: Vec<Vec<u8>> = self
            .payload_cache
            .get_all_payloads_raw(receiver, Buffer::Receiver)
            .await?;

        Ok(data)
    }

    pub async fn get_deniable_payloads(
        &self,
        receiver: &ProtocolAddress,
    ) -> Result<Vec<DeniablePayload>> {
        self.payload_cache
            .get_all_payloads(receiver, Buffer::Receiver)
            .await
    }

    pub async fn dequeue_outgoing_chunk_buffer(
        &self,
        receiver: &ProtocolAddress,
    ) -> Result<Vec<DeniablePayload>> {
        let chunks = self.chunk_cache.dequeue_outgoing_chunks(receiver).await?;
        let (payloads, pending_chunks) = self.create_deniable_payloads(chunks)?;

        if !pending_chunks.is_empty() {
            eprintln!("Error: payloads created but there are still chunks left");
            let _ = self.set_outgoing_chunks(receiver, pending_chunks).await?;
        }

        Ok(payloads)
    }

    pub async fn dequeue_payload_data(
        &self,
        receiver: &ProtocolAddress,
        bytes_amount: usize,
    ) -> Result<Vec<DenimChunk>> {
        let chunks = self
            .payload_cache
            .dequeue_payload_data(receiver, Buffer::Receiver, bytes_amount)
            .await?;

        let mut denim_chunks = Vec::new();
        for chunk in chunks {
            denim_chunks.push(DenimChunk {
                chunk: chunk.0,
                flags: chunk.1,
            });
        }

        Ok(denim_chunks)
    }

    /// Store deniable payloads decoded from chunks in incomming buffer
    pub async fn set_deniable_payloads(
        &self,
        receiver: &ProtocolAddress,
        payloads: Vec<DeniablePayload>,
    ) -> Result<u64> {
        let mut count = 0;
        for payload in payloads {
            count += self
                .payload_cache
                .insert(
                    receiver,
                    Buffer::Receiver,
                    &payload,
                    &Uuid::new_v4().to_string(),
                )
                .await?
        }
        Ok(count)
    }

    /// Create deniable payloads from chunks in incoming buffer
    pub fn create_deniable_payloads(
        &self,
        chunks: Vec<DenimChunk>,
    ) -> Result<(Vec<DeniablePayload>, Vec<DenimChunk>)> {
        let mut payloads: Vec<DeniablePayload> = Vec::new();
        let mut pending_chunks: Vec<DenimChunk> = Vec::new();
        let mut iterator = chunks.into_iter();

        while let Some(mut chunk) = iterator.next() {
            match ChunkType::from(chunk.flags) {
                ChunkType::Dummy => continue,
                ChunkType::Final => {
                    pending_chunks.sort();
                    let mut payload_data = pending_chunks
                        .iter()
                        .flat_map(|d| d.chunk.clone())
                        .collect::<Vec<u8>>();
                    payload_data.append(&mut chunk.chunk);
                    payloads.push(bincode::deserialize(&payload_data)?);
                    pending_chunks.clear();
                }
                ChunkType::Data(_) => pending_chunks.push(chunk.clone()),
            }
        }
        Ok((payloads, pending_chunks))
    }
}

#[cfg(test)]
pub mod denim_manager_tests {
    use common::web_api::{PayloadData, SignalMessage};
    use rand::seq::SliceRandom;

    use super::*;
    use crate::test_utils::{
        message_cache::{generate_payload, teardown, DeniablePayloadType, MockWebSocketConnection},
        user::new_account_and_address,
    };

    async fn init_manager() -> DenIMManager<MockWebSocketConnection> {
        DenIMManager::<MockWebSocketConnection> {
            chunk_cache: ChunkCache::connect(),
            payload_cache: PayloadCache::connect(),
            chunker: Chunker::default(),
        }
    }

    fn create_chunks(size: usize, flag: i32) -> Vec<DenimChunk> {
        let mut chunks = Vec::new();
        for i in 0..size {
            chunks.push(DenimChunk {
                flags: flag,
                chunk: vec![i as u8],
            });
        }
        chunks
    }

    fn create_payload_chunks(payload_data: PayloadData) -> (Vec<DenimChunk>, Vec<DenimChunk>) {
        let mut result = Vec::new();

        let (incoming_chunks, _size, mut pending_data) =
            common::deniable::chunk::Chunker::create_ordered_chunks(0.6, 40.0, payload_data);

        result.append(&mut incoming_chunks.clone());

        let final_incoming_chunks = loop {
            if pending_data.chunk.is_empty() {
                break Vec::new();
            }

            let (incoming_chunks, _size, new_pending_data) =
                common::deniable::chunk::Chunker::create_ordered_chunks(
                    0.6,
                    40.0,
                    pending_data.clone(),
                );

            pending_data = new_pending_data;

            if incoming_chunks.iter().find(|d| d.flags == 2).is_some() {
                break incoming_chunks;
            }

            result.append(&mut incoming_chunks.clone())
        };

        (result, final_incoming_chunks)
    }

    fn create_deniable_payload(payload: DeniablePayload, text: &str) -> DeniablePayload {
        match payload {
            DeniablePayload::SignalMessage(_) => DeniablePayload::SignalMessage(SignalMessage {
                r#type: 1,
                destination_device_id: 1,
                destination_registration_id: 1,
                content: text.to_string(),
            }),
            _ => DeniablePayload::SignalMessage(SignalMessage::default()),
        }
    }

    #[tokio::test]
    async fn test_incoming_and_outgoing_buffer() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, sender_address) = new_account_and_address();
        let incoming_chunks = create_chunks(1, 0);

        let (_, receiver_address) = new_account_and_address();
        let outgoing_chunks = create_chunks(2, 0);

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks)
            .await;

        let _ = denim_manager
            .set_outgoing_chunks(&receiver_address, outgoing_chunks)
            .await;

        let result_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let result_outgoing_chunks = denim_manager
            .get_outgoing_chunks(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(result_incoming_chunks.len(), 1);
        assert_eq!(result_outgoing_chunks.len(), 2);
    }

    #[tokio::test]
    async fn test_multiple_incoming_buffers() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, sender_address1) = new_account_and_address();
        let incoming_chunks1 = create_chunks(1, 0);

        let (_, sender_address2) = new_account_and_address();
        let incoming_chunks2 = create_chunks(2, 0);

        let _ = denim_manager
            .set_incoming_chunks(&sender_address1, incoming_chunks1)
            .await;

        let _ = denim_manager
            .set_incoming_chunks(&sender_address2, incoming_chunks2)
            .await;

        let result_incoming_chunks1 = denim_manager
            .get_incoming_chunks(&sender_address1)
            .await
            .unwrap();

        let result_incoming_chunks2 = denim_manager
            .get_incoming_chunks(&sender_address2)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(result_incoming_chunks1.len(), 1);
        assert_eq!(result_incoming_chunks2.len(), 2);
    }

    #[tokio::test]
    async fn test_multiple_outgoing_buffers() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address1) = new_account_and_address();
        let outgoing_chunks1 = create_chunks(1, 0);

        let (_, receiver_address2) = new_account_and_address();
        let outgoing_chunks2 = create_chunks(2, 0);

        let _ = denim_manager
            .set_outgoing_chunks(&receiver_address1, outgoing_chunks1)
            .await;

        let _ = denim_manager
            .set_outgoing_chunks(&receiver_address2, outgoing_chunks2)
            .await;

        let result_outgoing_chunks1 = denim_manager
            .get_outgoing_chunks(&receiver_address1)
            .await
            .unwrap();

        let result_outgoing_chunks2 = denim_manager
            .get_outgoing_chunks(&receiver_address2)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(result_outgoing_chunks1.len(), 1);
        assert_eq!(result_outgoing_chunks2.len(), 2);
    }

    #[tokio::test]
    async fn test_create_deniable_payloads_none_pending_data() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let payload = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let data = bincode::serialize(&payload).unwrap();

        let (incoming_chunks, _size, _pending_data) =
            common::deniable::chunk::Chunker::create_ordered_chunks(
                0.6,
                150.0,
                PayloadData::new(data),
            );

        let (_, sender_address) = new_account_and_address();

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks.clone())
            .await;

        let cached_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let (result_payloads, result_pending) = denim_manager
            .create_deniable_payloads(cached_incoming_chunks)
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(incoming_chunks.len(), 2);
        assert_eq!(incoming_chunks[0].flags, 2);
        assert_eq!(incoming_chunks[1].flags, 1);
        assert_eq!(payload, result_payloads[0]);
        assert!(result_pending.is_empty());
    }

    #[tokio::test]
    async fn test_create_deniable_payloads_with_pending_data() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let payload = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let data = bincode::serialize(&payload).unwrap();

        let dummy_chunks = create_chunks(5, 1);

        let (incoming_chunks1, _size1, pending_data1) =
            common::deniable::chunk::Chunker::create_ordered_chunks(
                0.6,
                100.0,
                PayloadData::new(data),
            );

        let (incoming_chunks2, _size2, pending_data2) =
            common::deniable::chunk::Chunker::create_ordered_chunks(0.6, 100.0, pending_data1);

        let (_, sender_address) = new_account_and_address();

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks1)
            .await;

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, dummy_chunks.clone())
            .await;

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks2)
            .await;

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, dummy_chunks)
            .await;

        let cached_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let (result_payloads, result_pending_chunks) = denim_manager
            .create_deniable_payloads(cached_incoming_chunks)
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(payload, result_payloads[0]);
        assert!(pending_data2.chunk.is_empty());
        assert!(result_pending_chunks.is_empty());
    }

    #[tokio::test]
    async fn test_create_deniable_payloads_multiple_payloads() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );
        let data1 = bincode::serialize(&payload1).unwrap();

        let payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Eve is here written",
        );
        let data2 = bincode::serialize(&payload2).unwrap();

        let (incoming_chunks1, _size1, pending_data1) =
            common::deniable::chunk::Chunker::create_ordered_chunks(
                0.6,
                150.0,
                PayloadData::new(data1),
            );

        let (incoming_chunks2, _size2, pending_data2) =
            common::deniable::chunk::Chunker::create_ordered_chunks(
                0.6,
                150.0,
                PayloadData::new(data2),
            );

        let (_, sender_address) = new_account_and_address();

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks1)
            .await;

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks2)
            .await;

        let cached_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let (result_payloads, result_pending_chunks) = denim_manager
            .create_deniable_payloads(cached_incoming_chunks)
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(payload1, result_payloads[0]);
        assert_eq!(payload2, result_payloads[1]);
        assert!(pending_data1.chunk.is_empty());
        assert!(pending_data2.chunk.is_empty());
        assert!(result_pending_chunks.is_empty());
    }

    #[tokio::test]
    async fn test_create_deniable_payloads_multiple_data_chunks() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, sender_address) = new_account_and_address();

        let payload = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );
        let data = bincode::serialize(&payload).unwrap();

        let (incoming_chunks, _size, mut pending_data) =
            common::deniable::chunk::Chunker::create_ordered_chunks(
                0.6,
                40.0,
                PayloadData::new(data),
            );

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks)
            .await;

        // Exhaust pending data to create and store chunks
        while !pending_data.chunk.is_empty() {
            let (incoming_chunks, _size, new_pending_data) =
                common::deniable::chunk::Chunker::create_ordered_chunks(
                    0.6,
                    40.0,
                    pending_data.clone(),
                );
            pending_data = new_pending_data;
            let _ = denim_manager
                .set_incoming_chunks(&sender_address, incoming_chunks)
                .await;
        }

        let cached_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let (result_payloads, result_pending_chunks) = denim_manager
            .create_deniable_payloads(cached_incoming_chunks)
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(payload, result_payloads[0]);
        assert!(result_pending_chunks.is_empty());
    }

    #[tokio::test]
    async fn test_create_deniable_payloads_out_of_order_data_chunks() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, sender_address) = new_account_and_address();

        let payload = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );
        let data = bincode::serialize(&payload).unwrap();

        let (incoming_chunks, _size, mut pending_data) =
            common::deniable::chunk::Chunker::create_ordered_chunks(
                0.6,
                40.0,
                PayloadData::new(data),
            );

        // Store chunks to later insert into cache
        let mut buffer = Vec::<DenimChunk>::new();

        // Add some dummy chunks
        let mut dummy_chunks = create_chunks(5, 1);
        buffer.append(&mut dummy_chunks);

        // Add first payload chunks
        buffer.append(&mut incoming_chunks.clone());

        // Exhaust pending data to store data chunks and get final chunk
        let mut final_incoming_chunks = loop {
            if pending_data.chunk.is_empty() {
                break Vec::new();
            }

            let (incoming_chunks, _size, new_pending_data) =
                common::deniable::chunk::Chunker::create_ordered_chunks(
                    0.6,
                    40.0,
                    pending_data.clone(),
                );

            pending_data = new_pending_data;

            if incoming_chunks.iter().find(|d| d.flags == 2).is_some() {
                break incoming_chunks;
            }

            buffer.append(&mut incoming_chunks.clone());
        };

        // Change order of data chunks
        let mut rng = rand::thread_rng();
        buffer.shuffle(&mut rng);

        // Last chunk in payload should always be known
        buffer.append(&mut final_incoming_chunks);

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, buffer)
            .await;

        let cached_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let (result_payloads, result_pending_chunks) = denim_manager
            .create_deniable_payloads(cached_incoming_chunks)
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(payload, result_payloads[0]);
        assert!(result_pending_chunks.is_empty());
    }

    #[tokio::test]
    async fn test_create_deniable_payloads_multiple_payload_data_chunks_out_of_order() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, sender_address) = new_account_and_address();

        let payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let data1 = bincode::serialize(&payload1).unwrap();
        let (mut payload_chunks1, mut final_payload_chunks1) =
            create_payload_chunks(PayloadData::new(data1));

        let payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let data2 = bincode::serialize(&payload2).unwrap();
        let (mut payload_chunks2, mut final_payload_chunks2) =
            create_payload_chunks(PayloadData::new(data2));

        // Add some dummy chunks
        let dummy_chunks = create_chunks(5, 1);
        payload_chunks1.append(&mut dummy_chunks.clone());
        payload_chunks2.append(&mut dummy_chunks.clone());

        // Change order of data chunks
        let mut rng = rand::thread_rng();
        payload_chunks1.shuffle(&mut rng);
        let mut rng = rand::thread_rng();
        payload_chunks2.shuffle(&mut rng);

        // Last chunk in payload should always be known
        payload_chunks1.append(&mut final_payload_chunks1);
        payload_chunks2.append(&mut final_payload_chunks2);

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, payload_chunks1.clone())
            .await;

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, payload_chunks2.clone())
            .await;

        let cached_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let (result_payloads, result_pending_chunks) = denim_manager
            .create_deniable_payloads(cached_incoming_chunks)
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(payload1, result_payloads[0]);
        assert_eq!(payload2, result_payloads[1]);
        assert!(result_pending_chunks.is_empty());
    }

    #[tokio::test]
    async fn test_multiple_outgoing_payloads() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address1) = new_account_and_address();
        let outgoing_payload1 = generate_payload(DeniablePayloadType::SignalMessage);

        let (_, receiver_address2) = new_account_and_address();
        let outgoing_payload2 = generate_payload(DeniablePayloadType::SignalMessage);
        let outgoing_payload3 = generate_payload(DeniablePayloadType::KeyResponse);

        let _ = denim_manager
            .set_deniable_payloads(&receiver_address1, vec![outgoing_payload1.clone()])
            .await;

        let _ = denim_manager
            .set_deniable_payloads(
                &receiver_address2,
                vec![outgoing_payload2.clone(), outgoing_payload3.clone()],
            )
            .await;

        let result_outgoing_payloads1 = denim_manager
            .get_deniable_payloads(&receiver_address1)
            .await
            .unwrap();

        let result_outgoing_payloads2 = denim_manager
            .get_deniable_payloads(&receiver_address2)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert_eq!(result_outgoing_payloads1.len(), 1);
        assert_eq!(result_outgoing_payloads1[0], outgoing_payload1);
        assert_eq!(result_outgoing_payloads2.len(), 2);
        assert_eq!(result_outgoing_payloads2[0], outgoing_payload2);
        assert_eq!(result_outgoing_payloads2[1], outgoing_payload3);
    }

    // TODO: Test when DenIMChunk has all data, meaning flags = 0

    #[tokio::test]
    async fn test_get_deniable_payload_raw() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(&receiver_address, vec![outgoing_payload.clone()])
            .await
            .unwrap();

        let result_outgoing_payloads_raw = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let (payload_chunks, final_payload_chunks) =
            create_payload_chunks(PayloadData::new(result_outgoing_payloads_raw[0].clone()));

        let (result_payloads, pending_chunks) = denim_manager
            .create_deniable_payloads(vec![payload_chunks, final_payload_chunks].concat())
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert!(pending_chunks.is_empty());
        assert_eq!(result_payloads, vec![outgoing_payload]);
    }

    #[tokio::test]
    async fn test_get_multiple_deniable_payloads_raw() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let outgoing_payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Eve is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(
                &receiver_address,
                vec![outgoing_payload1.clone(), outgoing_payload2.clone()],
            )
            .await
            .unwrap();

        let result_outgoing_payloads_raw = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let (payload_chunks1, final_payload_chunks1) =
            create_payload_chunks(PayloadData::new(result_outgoing_payloads_raw[0].clone()));

        let (payload_chunks2, final_payload_chunks2) =
            create_payload_chunks(PayloadData::new(result_outgoing_payloads_raw[1].clone()));

        let (result_payloads, pending_chunks) = denim_manager
            .create_deniable_payloads(
                vec![
                    payload_chunks1,
                    final_payload_chunks1,
                    payload_chunks2,
                    final_payload_chunks2,
                ]
                .concat(),
            )
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert!(pending_chunks.is_empty());
        assert_eq!(result_payloads.len(), 2);
        assert_eq!(result_payloads, vec![outgoing_payload1, outgoing_payload2]);
    }

    #[tokio::test]
    async fn test_take_deniable_payload_data() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(&receiver_address, vec![outgoing_payload1.clone()])
            .await
            .unwrap();

        let outgoing_payloads = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let result_taken_values1 = denim_manager
            .dequeue_payload_data(&receiver_address, 25)
            .await
            .unwrap();

        let result_taken_values2 = denim_manager
            .dequeue_payload_data(&receiver_address, 25)
            .await
            .unwrap();

        let result_taken_values3 = denim_manager
            .dequeue_payload_data(&receiver_address, 6)
            .await
            .unwrap();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        let result_taken_values = vec![
            result_taken_values1,
            result_taken_values2,
            result_taken_values3,
        ]
        .concat()
        .into_iter()
        .flat_map(|data| data.chunk)
        .collect::<Vec<u8>>();

        assert!(result_outgoing_payloads_buffer.is_empty());
        assert_eq!(vec![result_taken_values], outgoing_payloads);
    }

    #[tokio::test]
    async fn test_take_multiple_deniable_payloads_data_exact_buffer_amount() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let outgoing_payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Eve is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(
                &receiver_address,
                vec![outgoing_payload1.clone(), outgoing_payload2.clone()],
            )
            .await
            .unwrap();

        let outgoing_payloads = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let result_taken_values = denim_manager
            .dequeue_payload_data(&receiver_address, 112)
            .await
            .unwrap()
            .into_iter()
            .map(|data| data.chunk)
            .collect::<Vec<Vec<u8>>>();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert!(result_outgoing_payloads_buffer.is_empty());
        assert_eq!(result_taken_values, outgoing_payloads);
    }

    #[tokio::test]
    async fn test_take_multiple_deniable_payloads_data_over_buffer_amount() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let outgoing_payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Eve is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(
                &receiver_address,
                vec![outgoing_payload1.clone(), outgoing_payload2.clone()],
            )
            .await
            .unwrap();

        let outgoing_payloads = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let result_taken_values = denim_manager
            .dequeue_payload_data(&receiver_address, 212)
            .await
            .unwrap()
            .into_iter()
            .map(|data| data.chunk)
            .collect::<Vec<Vec<u8>>>();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert!(result_outgoing_payloads_buffer.is_empty());
        assert_eq!(result_taken_values, outgoing_payloads);
    }

    #[tokio::test]
    async fn test_take_multiple_deniable_payloads_data_under_buffer_amount() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let outgoing_payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Eve is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(
                &receiver_address,
                vec![outgoing_payload1.clone(), outgoing_payload2.clone()],
            )
            .await
            .unwrap();

        let outgoing_payloads = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let result_taken_values = denim_manager
            .dequeue_payload_data(&receiver_address, 12)
            .await
            .unwrap()
            .into_iter()
            .map(|data| data.chunk)
            .collect::<Vec<Vec<u8>>>();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert!(!result_outgoing_payloads_buffer.is_empty());
        assert_eq!(result_taken_values[0], outgoing_payloads[0][0..12]);
    }

    #[tokio::test]
    async fn test_take_multiple_deniable_payloads_data_zero_buffer_amount() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let outgoing_payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Eve is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(
                &receiver_address,
                vec![outgoing_payload1.clone(), outgoing_payload2.clone()],
            )
            .await
            .unwrap();

        let outgoing_payloads = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let result_taken_values = denim_manager
            .dequeue_payload_data(&receiver_address, 0)
            .await
            .unwrap();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert!(result_taken_values.is_empty());
        assert_eq!(result_outgoing_payloads_buffer, outgoing_payloads);
    }

    #[tokio::test]
    async fn test_take_deniable_payload_data_inorder() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(&receiver_address, vec![outgoing_payload1.clone()])
            .await
            .unwrap();

        let outgoing_payloads = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        let result_taken_values1 = denim_manager
            .dequeue_payload_data(&receiver_address, 20)
            .await
            .unwrap();

        let result_taken_values2 = denim_manager
            .dequeue_payload_data(&receiver_address, 20)
            .await
            .unwrap();

        let result_taken_values3 = denim_manager
            .dequeue_payload_data(&receiver_address, 15)
            .await
            .unwrap();

        let result_taken_values4 = denim_manager
            .dequeue_payload_data(&receiver_address, 1)
            .await
            .unwrap();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        let result_taken_values = vec![
            result_taken_values1[0].clone().chunk,
            result_taken_values2[0].clone().chunk,
            result_taken_values3[0].clone().chunk,
            result_taken_values4[0].clone().chunk,
        ]
        .concat();

        assert!(result_outgoing_payloads_buffer.is_empty());
        assert_eq!(result_taken_values1[0].flags, 0);
        assert_eq!(result_taken_values2[0].flags, -1);
        assert_eq!(result_taken_values3[0].flags, -2);
        assert_eq!(result_taken_values4[0].flags, 2);
        assert_eq!(vec![result_taken_values], outgoing_payloads);
    }

    #[tokio::test]
    async fn test_take_multiple_deniable_payloads_data_inorder_with_overlap() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let outgoing_payload2 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Eve is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(
                &receiver_address,
                vec![outgoing_payload1.clone(), outgoing_payload2.clone()],
            )
            .await
            .unwrap();

        let outgoing_payloads = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap()
            .into_iter()
            .flat_map(|payload| payload)
            .collect::<Vec<u8>>();

        let result_taken_values1 = denim_manager
            .dequeue_payload_data(&receiver_address, 20)
            .await
            .unwrap();

        let result_taken_values2 = denim_manager
            .dequeue_payload_data(&receiver_address, 20)
            .await
            .unwrap();

        let result_taken_values3 = denim_manager
            .dequeue_payload_data(&receiver_address, 15)
            .await
            .unwrap();

        let result_taken_values4 = denim_manager
            .dequeue_payload_data(&receiver_address, 15)
            .await
            .unwrap();

        let result_taken_values5 = denim_manager
            .dequeue_payload_data(&receiver_address, 20)
            .await
            .unwrap();

        let result_taken_values6 = denim_manager
            .dequeue_payload_data(&receiver_address, 20)
            .await
            .unwrap();

        let result_taken_values7 = denim_manager
            .dequeue_payload_data(&receiver_address, 2)
            .await
            .unwrap();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        let result_taken = vec![
            result_taken_values1[0].clone().chunk,
            result_taken_values2[0].clone().chunk,
            result_taken_values3[0].clone().chunk,
            result_taken_values4[0].clone().chunk,
            result_taken_values4[1].clone().chunk,
            result_taken_values5[0].clone().chunk,
            result_taken_values6[0].clone().chunk,
            result_taken_values7[0].clone().chunk,
        ]
        .concat();

        assert!(result_outgoing_payloads_buffer.is_empty());
        assert_eq!(result_taken_values1[0].flags, 0);
        assert_eq!(result_taken_values2[0].flags, -1);
        assert_eq!(result_taken_values3[0].flags, -2);
        assert_eq!(result_taken_values4[0].flags, 2);
        assert_eq!(result_taken_values4[1].flags, 0); // Overlap testet
        assert_eq!(result_taken_values5[0].flags, -1);
        assert_eq!(result_taken_values6[0].flags, -2);
        assert_eq!(result_taken_values7[0].flags, 2);
        assert_eq!(vec![result_taken], vec![outgoing_payloads]);
    }

    #[tokio::test]
    async fn test_dequeue_outgoing_chunk_buffer() {
        let denim_manager = init_manager().await;
        let connection = denim_manager.chunk_cache.get_connection().await.unwrap();

        let (_, receiver_address) = new_account_and_address();

        let outgoing_payload1 = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let data = bincode::serialize(&outgoing_payload1).unwrap();

        let (payload_chunks1, final_payload_chunks1) =
            create_payload_chunks(PayloadData::new(data));

        let _ = denim_manager
            .set_outgoing_chunks(
                &receiver_address,
                [payload_chunks1, final_payload_chunks1].concat(),
            )
            .await
            .unwrap();

        let result_payloads = denim_manager
            .dequeue_outgoing_chunk_buffer(&receiver_address)
            .await
            .unwrap();

        let result_outgoing_payloads_buffer = denim_manager
            .get_deniable_payloads_raw(&receiver_address)
            .await
            .unwrap();

        // Teardown cache
        teardown(&denim_manager.chunk_cache.test_key, connection).await;

        assert!(result_outgoing_payloads_buffer.is_empty());
        assert_eq!(result_payloads[0], outgoing_payload1);
    }
}
