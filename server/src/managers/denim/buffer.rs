use std::fmt::Display;

#[derive(Clone, Copy)]
pub enum Buffer {
    Sender,
    Reciver,
}

impl Display for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Buffer::Sender => write!(f, "sender"),
            Buffer::Reciver => write!(f, "reciver"),
        }
    }
}
