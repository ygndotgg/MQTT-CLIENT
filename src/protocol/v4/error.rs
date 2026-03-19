#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum Error {
    #[error("need {0} more bytes")]
    InsufficientBytes(usize),
    #[error("malformed remaining length")]
    MalformedRemainingLength,
    #[error("invalid packet type {0}")]
    InvalidPacketType(u8),
    #[error("invalid qos {0}")]
    InvalidQos(u8),
    #[error("malformed packet")]
    MalformedPacket,
    #[error("packet id must be non zero")]
    PacketIdZero,
    #[error("invalid fixed header flags: type={packet_type},flags = {flags:#x}")]
    InvalidFixedHeaderFlags { packet_type: u8, flags: u8 },
    #[error("invalid protocol")]
    InvalidProtocol,
    #[error("invalid protocol level {0}")]
    InvalidProtocolLevel(u8),
}
