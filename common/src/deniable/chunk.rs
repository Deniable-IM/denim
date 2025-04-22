use super::{constants, DeniableSendingBuffer};
use crate::web_api::{DenimChunk, PayloadData};
use bincode::serialize;

pub enum ChunkType {
    Data(i32),
    Dummy,
    Final,
}

impl From<i32> for ChunkType {
    fn from(value: i32) -> Self {
        match value {
            1 => ChunkType::Dummy,
            2 => ChunkType::Final,
            ..=0 => ChunkType::Data(value),
            _ => unreachable!(),
        }
    }
}

impl From<ChunkType> for i32 {
    fn from(value: ChunkType) -> Self {
        match value {
            ChunkType::Dummy => 1,
            ChunkType::Final => 2,
            ChunkType::Data(value) => value,
        }
    }
}

pub struct Chunker;

impl Chunker {
    pub async fn create_chunks<T: DeniableSendingBuffer>(
        q: f32,
        regular_payload_size: f32,
        buffer: &mut T,
    ) -> Result<(Vec<DenimChunk>, usize), String> {
        let mut outgoing_chunks: Vec<DenimChunk> = vec![];
        let total_free_space = (regular_payload_size * q).ceil() as usize;

        let mut free_space = total_free_space - constants::EMPTY_VEC_SIZE;
        while free_space >= constants::EMPTY_DENIMCHUNK_SIZE {
            let chunk_size = free_space - constants::EMPTY_DENIMCHUNK_SIZE;
            let current_outgoing_message = buffer.get_outgoing_message().await.unwrap_or_default();

            let new_chunk;
            if !current_outgoing_message.1.is_empty() && chunk_size != 0 {
                //Deniable
                if current_outgoing_message.1.len() <= chunk_size {
                    new_chunk = DenimChunk {
                        chunk: current_outgoing_message.1.to_vec(),
                        flags: ChunkType::Final.into(),
                    };
                    buffer
                        .remove_outgoing_message(current_outgoing_message.0)
                        .await
                        .map_err(|err| format!("{err}"))?;
                } else {
                    new_chunk = DenimChunk {
                        chunk: current_outgoing_message.1[..chunk_size].to_vec(),
                        flags: ChunkType::Data(0).into(),
                    };
                    let remaining_current_outgoing_message =
                        current_outgoing_message.1[chunk_size..].to_vec();
                    buffer
                        .set_outgoing_message(
                            Some(current_outgoing_message.0),
                            remaining_current_outgoing_message,
                        )
                        .await
                        .map_err(|err| format!("{err}"))?;
                }
            } else {
                //Dummy
                new_chunk = DenimChunk {
                    chunk: vec![0; chunk_size],
                    flags: ChunkType::Dummy.into(),
                };
            }

            outgoing_chunks.push(new_chunk);
            free_space = total_free_space - serialize(&outgoing_chunks).unwrap().len();
        }

        Ok((outgoing_chunks, free_space))
    }

    pub fn create_ordered_chunks(
        q: f32,
        regular_payload_size: f32,
        payload: PayloadData,
    ) -> (Vec<DenimChunk>, usize, PayloadData) {
        let mut result_chunks: Vec<DenimChunk> = vec![];

        let mut payload_data = payload.chunk.clone();
        let mut payload_data_count = payload.flags;

        let total_free_space = (regular_payload_size * q).ceil() as usize;
        let mut free_space = total_free_space - constants::EMPTY_VEC_SIZE;

        while free_space >= constants::EMPTY_DENIMCHUNK_SIZE {
            let chunk_size = free_space - constants::EMPTY_DENIMCHUNK_SIZE;
            let new_chunk;
            if !payload_data.is_empty() && chunk_size != 0 {
                if payload_data.len() <= chunk_size {
                    // Deniable
                    new_chunk = DenimChunk {
                        chunk: payload_data.to_vec(),
                        flags: ChunkType::Final.into(),
                    };
                    payload_data.clear();
                } else {
                    // Data
                    new_chunk = DenimChunk {
                        chunk: payload_data[..chunk_size].to_vec(),
                        flags: ChunkType::Data(payload_data_count).into(),
                    };
                    payload_data_count -= 1;
                    payload_data = payload_data[chunk_size..].to_vec();
                }
            } else {
                // Dummy
                new_chunk = DenimChunk {
                    chunk: vec![0; chunk_size],
                    flags: ChunkType::Dummy.into(),
                };
            }

            result_chunks.push(new_chunk);
            free_space = total_free_space - serialize(&result_chunks).unwrap().len();
        }

        // Return payload data that need to be chunked up at a later time
        let pending_payload = PayloadData {
            chunk: payload_data,
            flags: payload_data_count,
        };

        (result_chunks, free_space, pending_payload)
    }
}

#[cfg(test)]
mod test {
    use axum::async_trait;
    use bincode::serialize;
    use libsignal_protocol::SignalProtocolError;

    use crate::{
        deniable::{chunk::Chunker, constants, DeniableSendingBuffer},
        web_api::DenimChunk,
    };

    struct MockDeniableSendingBuffer;

    #[async_trait(?Send)]
    impl DeniableSendingBuffer for MockDeniableSendingBuffer {
        async fn get_outgoing_message(&mut self) -> Result<(u32, Vec<u8>), SignalProtocolError> {
            let message: [u8; 32] = rand::random();
            Ok((1, message.to_vec()))
        }
        async fn set_outgoing_message(
            &mut self,
            _: Option<u32>,
            _: Vec<u8>,
        ) -> Result<(), SignalProtocolError> {
            Ok(())
        }
        async fn remove_outgoing_message(&mut self, _: u32) -> Result<(), SignalProtocolError> {
            Ok(())
        }
    }

    #[test]
    fn empty_chunk_size_checker() {
        let empty_chunk = DenimChunk {
            chunk: vec![],
            flags: 1,
        };
        let empty_chunk_serialized = serialize(&empty_chunk).unwrap();
        assert_eq!(
            empty_chunk_serialized.len(),
            constants::EMPTY_DENIMCHUNK_SIZE
        );
    }

    #[test]
    fn dummy_chunk_size_checker() {
        let dummy_payload_size = 10;
        let dummy_chunk = DenimChunk {
            chunk: vec![0; dummy_payload_size],
            flags: 1,
        };
        let dummy_serialized = serialize(&dummy_chunk).unwrap();
        assert_eq!(
            dummy_serialized.len(),
            constants::EMPTY_DENIMCHUNK_SIZE + dummy_payload_size
        );
    }

    #[test]
    fn nonempty_chunk_size_checker() {
        let nonempty_chunk = DenimChunk {
            chunk: vec![1],
            flags: 1,
        };
        let nonempty_chunk_serialized = serialize(&nonempty_chunk).unwrap();
        assert_eq!(
            nonempty_chunk_serialized.len(),
            constants::EMPTY_DENIMCHUNK_SIZE + 1
        );
    }

    #[test]
    fn denim_array_with_empty_chunk_size_checker() {
        let denim_array: Vec<DenimChunk> = vec![DenimChunk {
            chunk: vec![],
            flags: 1,
        }];
        let denim_array_serialized = serialize(&denim_array).unwrap();
        assert_eq!(
            denim_array_serialized.len(),
            constants::DENIMARRAY_WITH_EMPTY_DENIMCHUNK_SIZE
        );
    }

    #[tokio::test]
    async fn create_chunks_no_chunks() {
        let expected_deniable_payload_length = (30.0_f32 * 0.6_f32).ceil() as usize;

        let chunks = Chunker::create_chunks(0.6, 30.0, &mut MockDeniableSendingBuffer {})
            .await
            .unwrap();

        let denim_array_serialized = serialize(&chunks.0).unwrap();
        assert!(chunks.0.is_empty());
        assert_eq!(
            denim_array_serialized.len() + chunks.1,
            expected_deniable_payload_length
        );
    }

    #[tokio::test]
    async fn create_chunks_dummy_chunk() {
        let expected_deniable_payload_length = (40.0_f32 * 0.6_f32).ceil() as usize;

        let chunks = Chunker::create_chunks(0.6, 40.0, &mut MockDeniableSendingBuffer {})
            .await
            .unwrap();

        let denim_array_serialized = serialize(&chunks.0).unwrap();
        assert_eq!(chunks.0.len(), 1);
        assert_eq!(
            denim_array_serialized.len() + chunks.1,
            expected_deniable_payload_length
        );
    }
}
