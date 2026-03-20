use crate::codec::decode_remaining_length;
use crate::protocol::v4::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedHeader {
    pub byte1: u8,
    pub remaining_len: usize,
    pub header_len: usize, // 1 byte control + remaining-length
}

impl FixedHeader {
    pub fn packet_type(&self) -> u8 {
        self.byte1 >> 4
    }
    pub fn flags(&self) -> u8 {
        self.byte1 & 0x0f
    }
    pub fn frame_len(&self) -> usize {
        self.header_len + self.remaining_len
    }
}

pub fn parse_fixed_header(input: &[u8]) -> Result<FixedHeader, Error> {
    if input.is_empty() {
        return Err(Error::InsufficientBytes(1));
    }
    let byte1 = input[0];
    let (remaining_len, len_bytes) = decode_remaining_length(&input[1..])?;
    Ok(FixedHeader {
        byte1,
        remaining_len,
        header_len: 1 + len_bytes,
    })
}
