use crate::{
    codec::{decode_packet, parse_fixed_header},
    protocol::v4::error::Error,
    runtime::task::RuntimeTaskError,
    types::Packet,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameState {
    Need(usize),
    Ready(Vec<u8>),
}

#[derive(Default, Debug)]
pub struct FrameParser {
    buf: Vec<u8>,
}

impl FrameParser {
    pub fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }
    pub fn next_frame(&mut self) -> Result<FrameState, Error> {
        match parse_fixed_header(&self.buf) {
            Err(Error::InsufficientBytes(n)) => return Ok(FrameState::Need(n)),
            Err(e) => return Err(e),
            Ok(fh) => {
                if self.buf.len() < fh.frame_len() {
                    return Ok(FrameState::Need(fh.frame_len() - self.buf.len()));
                }
                let frame = self.buf.drain(..fh.frame_len()).collect::<Vec<u8>>();
                Ok(FrameState::Ready(frame))
            }
        }
    }
    pub fn next_packet(&mut self) -> Result<Option<Packet>, Error> {
        match self.next_frame()? {
            FrameState::Need(_) => Ok(None),
            FrameState::Ready(frame) => Ok(Some(decode_packet(&frame)?)),
        }
    }
}

pub fn try_parse_frame(buf: &Vec<u8>) -> Result<Option<(Packet, usize)>, RuntimeTaskError> {
    unimplemented!()
}
