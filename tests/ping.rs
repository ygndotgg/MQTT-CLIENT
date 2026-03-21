use mqtt_client::codec::{decode_packet, encode_packet};
use mqtt_client::protocol::v4::error::Error;
use mqtt_client::types::Packet;

#[test]
fn t_pingreq_golden_roundtrip() {
    let p = Packet::PingReq;
    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(out, vec![0xC0, 0x00]);
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_pingresp_golden_roundtrip() {
    let p = Packet::PingResp;
    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(out, vec![0xD0, 0x00]);
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_pingreq_reject_nonzero_flags() {
    let pkt = vec![0xC1, 0x00];
    assert_eq!(
        decode_packet(&pkt).unwrap_err(),
        Error::InvalidFixedHeaderFlags {
            packet_type: 12,
            flags: 1
        }
    );
}

#[test]
fn t_pingresp_reject_nonzero_flags() {
    let pkt = vec![0xD1, 0x00];
    assert_eq!(
        decode_packet(&pkt).unwrap_err(),
        Error::InvalidFixedHeaderFlags {
            packet_type: 13,
            flags: 1
        }
    );
}

#[test]
fn t_pingreq_reject_nonzero_remaining_length() {
    let pkt = vec![0xC0, 0x01, 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_pingresp_reject_nonzero_remaining_length() {
    let pkt = vec![0xD0, 0x01, 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}
