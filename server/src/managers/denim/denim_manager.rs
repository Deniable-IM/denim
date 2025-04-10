use super::{buffer::Buffer, chunk_cache::ChunkCache, payload_cache::PayloadCache};
use crate::{
    availability_listener::AvailabilityListener, managers::manager::Manager,
    storage::database::SignalDatabase,
};
use anyhow::{anyhow, Ok, Result};
use common::web_api::{DeniablePayload, DenimChunk, PayloadData};
use libsignal_core::ProtocolAddress;
use uuid::Uuid;

pub struct DenIMManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    db: T,
    chunk_cache: ChunkCache<U>,
    payload_cache: PayloadCache<U>,
}

impl<T, U> Manager for DenIMManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl<T, U> DenIMManager<T, U>
where
    T: SignalDatabase,
    U: AvailabilityListener,
{
    pub fn new(db: T, chunk_cache: ChunkCache<U>, payload_cache: PayloadCache<U>) -> Self {
        Self {
            db,
            chunk_cache,
            payload_cache,
        }
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
            match chunk.flags {
                // Dummy
                1 => continue,
                // Final
                2 => {
                    pending_chunks.sort();
                    let mut payload_data = pending_chunks
                        .iter()
                        .flat_map(|d| d.chunk.clone())
                        .collect::<Vec<u8>>();
                    payload_data.append(&mut chunk.chunk);
                    payloads.push(bincode::deserialize(&payload_data)?);
                    pending_chunks.clear();
                }
                // Data
                ..=0 => {
                    pending_chunks.push(chunk.clone());
                }
                _ => return Err(anyhow!("Flags: {} not supported!", chunk.flags)),
            }
        }
        Ok((payloads, pending_chunks))
    }
}

#[cfg(test)]
pub mod denim_manager_tests {
    use common::deniable::chunk::Chunker;
    use common::web_api::{PayloadData, SignalMessage};
    use rand::seq::SliceRandom;

    use super::*;
    use crate::{
        storage::postgres::PostgresDatabase,
        test_utils::{
            database::database_connect,
            message_cache::{
                generate_payload, teardown, DeniablePayloadType, MockWebSocketConnection,
            },
            user::new_account_and_address,
        },
    };

    async fn init_manager() -> DenIMManager<PostgresDatabase, MockWebSocketConnection> {
        DenIMManager::<PostgresDatabase, MockWebSocketConnection> {
            db: database_connect().await,
            chunk_cache: ChunkCache::connect(),
            payload_cache: PayloadCache::connect(),
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

        let (_, reciver_address) = new_account_and_address();
        let outgoing_chunks = create_chunks(2, 0);

        let _ = denim_manager
            .set_incoming_chunks(&sender_address, incoming_chunks)
            .await;

        let _ = denim_manager
            .set_outgoing_chunks(&reciver_address, outgoing_chunks)
            .await;

        let result_incoming_chunks = denim_manager
            .get_incoming_chunks(&sender_address)
            .await
            .unwrap();

        let result_outgoing_chunks = denim_manager
            .get_outgoing_chunks(&reciver_address)
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

        let (_, reciver_address1) = new_account_and_address();
        let outgoing_chunks1 = create_chunks(1, 0);

        let (_, reciver_address2) = new_account_and_address();
        let outgoing_chunks2 = create_chunks(2, 0);

        let _ = denim_manager
            .set_outgoing_chunks(&reciver_address1, outgoing_chunks1)
            .await;

        let _ = denim_manager
            .set_outgoing_chunks(&reciver_address2, outgoing_chunks2)
            .await;

        let result_outgoing_chunks1 = denim_manager
            .get_outgoing_chunks(&reciver_address1)
            .await
            .unwrap();

        let result_outgoing_chunks2 = denim_manager
            .get_outgoing_chunks(&reciver_address2)
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

        let (_, reciver_address1) = new_account_and_address();
        let outgoing_payload1 = generate_payload(DeniablePayloadType::SignalMessage);

        let (_, reciver_address2) = new_account_and_address();
        let outgoing_payload2 = generate_payload(DeniablePayloadType::SignalMessage);
        let outgoing_payload3 = generate_payload(DeniablePayloadType::KeyResponse);

        let _ = denim_manager
            .set_deniable_payloads(&reciver_address1, vec![outgoing_payload1.clone()])
            .await;

        let _ = denim_manager
            .set_deniable_payloads(
                &reciver_address2,
                vec![outgoing_payload2.clone(), outgoing_payload3.clone()],
            )
            .await;

        let result_outgoing_payloads1 = denim_manager
            .get_deniable_payloads(&reciver_address1)
            .await
            .unwrap();

        let result_outgoing_payloads2 = denim_manager
            .get_deniable_payloads(&reciver_address2)
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

        let (_, reciver_address) = new_account_and_address();

        let outgoing_payload = create_deniable_payload(
            DeniablePayload::SignalMessage(SignalMessage::default()),
            "A message to Bob is here written",
        );

        let _ = denim_manager
            .set_deniable_payloads(&reciver_address, vec![outgoing_payload.clone()])
            .await
            .unwrap();

        let result_outgoing_payloads_raw = denim_manager
            .get_deniable_payloads_raw(&reciver_address)
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

        let (_, reciver_address) = new_account_and_address();

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
                &reciver_address,
                vec![outgoing_payload1.clone(), outgoing_payload2.clone()],
            )
            .await
            .unwrap();

        let result_outgoing_payloads_raw = denim_manager
            .get_deniable_payloads_raw(&reciver_address)
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
}
