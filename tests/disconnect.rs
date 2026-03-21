use mqtt_client::codec::{decode_packet, encode_packet};
use mqtt_client::protocol::v4::error::Error;
use mqtt_client::types::Packet;

#[test]
fn t_disconnect_golden_roundtrip() {
    let p = Packet::Disconnect;
    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(out, vec![0xE0, 0x00]);
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_disconnect_reject_nonzero_flags() {
    let pkt = vec![0xE1, 0x00];
    assert_eq!(
        decode_packet(&pkt).unwrap_err(),
        Error::InvalidFixedHeaderFlags {
            packet_type: 14,
            flags: 1
        }
    );
}

#[test]
fn t_disconnect_reject_nonzero_remaining_length() {
    let pkt = vec![0xE0, 0x01, 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_disconnect_reject_truncated_body_when_remaining_length_nonzero() {
    let pkt = vec![0xE0, 0x01];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::InsufficientBytes(1));
}
