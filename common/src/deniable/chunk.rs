use super::constants;
use crate::web_api::DenimChunk;
use bincode::serialize;

pub struct Chunker;

impl Chunker {
    pub fn create_chunks(q: f32, regular_payload_size: f32) -> (Vec<DenimChunk>, usize) {
        let mut outgoing_chunks: Vec<DenimChunk> = vec![];
        let total_free_space = (regular_payload_size * q).ceil() as usize;

        let mut free_space = total_free_space - constants::EMPTY_VEC_SIZE;
        while free_space >= constants::EMPTY_DENIMCHUNK_SIZE {
            let chunk_size = free_space - constants::EMPTY_DENIMCHUNK_SIZE;
            let current_outgoing_message: &[u8] = &vec![]; //Get current outgoing message here

            let new_chunk;
            if !current_outgoing_message.is_empty() && chunk_size != 0 {
                //Deniable
                if current_outgoing_message.len() <= chunk_size {
                    new_chunk = DenimChunk {
                        chunk: current_outgoing_message.to_vec(),
                        flags: 2,
                    };
                    // Replace current outgoing message here
                } else {
                    new_chunk = DenimChunk {
                        chunk: current_outgoing_message[..chunk_size].to_vec(),
                        flags: 0,
                    };
                    let _remaining_current_outgoing_message =
                        current_outgoing_message[chunk_size..].to_vec(); //Set current outgoing message to remainder here
                }
            } else {
                //Dummy
                new_chunk = DenimChunk {
                    chunk: vec![0; chunk_size],
                    flags: 1,
                };
            }

            outgoing_chunks.push(new_chunk);
            free_space = total_free_space - serialize(&outgoing_chunks).unwrap().len();
        }

        (outgoing_chunks, free_space)
    }

    pub fn create_chunks_clone(
        q: f32,
        regular_payload_size: f32,
        current_outgoing_message: (Vec<u8>, i32),
    ) -> (Vec<DenimChunk>, usize, (Vec<u8>, i32)) {
        let mut data_count = current_outgoing_message.1;

        let mut outgoing_chunks: Vec<DenimChunk> = vec![];
        let total_free_space = (regular_payload_size * q).ceil() as usize;

        let mut current_outgoing_message = current_outgoing_message.0.clone();

        let mut free_space = total_free_space - constants::EMPTY_VEC_SIZE;
        // return error
        while free_space >= constants::EMPTY_DENIMCHUNK_SIZE {
            let chunk_size = free_space - constants::EMPTY_DENIMCHUNK_SIZE;
            // let current_outgoing_message: &[u8] = &vec![]; //Get current outgoing message here

            let new_chunk;
            if !current_outgoing_message.is_empty() && chunk_size != 0 {
                //Deniable
                if current_outgoing_message.len() <= chunk_size {
                    new_chunk = DenimChunk {
                        chunk: current_outgoing_message.to_vec(),
                        flags: 2,
                    };
                    // Replace current outgoing message here
                    current_outgoing_message.clear();
                } else {
                    new_chunk = DenimChunk {
                        chunk: current_outgoing_message[..chunk_size].to_vec(),
                        flags: data_count,
                    };
                    data_count -= 1;
                    current_outgoing_message = current_outgoing_message[chunk_size..].to_vec();
                    // let _remaining_current_outgoing_message =
                    //     current_outgoing_message[chunk_size..].to_vec(); //Set current outgoing message to remainder here
                }
            } else {
                //Dummy
                new_chunk = DenimChunk {
                    chunk: vec![0; chunk_size],
                    flags: 1,
                };
            }

            outgoing_chunks.push(new_chunk);
            free_space = total_free_space - serialize(&outgoing_chunks).unwrap().len();
        }

        (
            outgoing_chunks,
            free_space,
            (current_outgoing_message, data_count),
        )
    }
}

#[cfg(test)]
mod test {
    use bincode::serialize;

    use crate::{
        deniable::{chunk::Chunker, constants},
        web_api::DenimChunk,
    };

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

    #[test]
    fn create_chunks_no_chunks() {
        let expected_deniable_payload_length = (30.0_f32 * 0.6_f32).ceil() as usize;

        let chunks = Chunker::create_chunks(0.6, 30.0);

        let denim_array_serialized = serialize(&chunks.0).unwrap();
        assert!(chunks.0.is_empty());
        assert_eq!(
            denim_array_serialized.len() + chunks.1,
            expected_deniable_payload_length
        );
    }

    #[test]
    fn create_chunks_dummy_chunk() {
        let expected_deniable_payload_length = (40.0_f32 * 0.6_f32).ceil() as usize;

        let chunks = Chunker::create_chunks(0.6, 40.0);

        let denim_array_serialized = serialize(&chunks.0).unwrap();
        assert_eq!(chunks.0.len(), 1);
        assert_eq!(
            denim_array_serialized.len() + chunks.1,
            expected_deniable_payload_length
        );
    }
}
