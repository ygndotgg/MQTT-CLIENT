use mqtt_client::codec::{FrameParser, FrameState};
use mqtt_client::protocol::v4::error::Error;
use mqtt_client::types::Packet;

#[test]
fn t_pingreq_one_byte_chunks_need_then_ready() {
    let mut p = FrameParser::default();

    p.push(&[0xC0]);
    assert_eq!(p.next_frame().unwrap(), FrameState::Need(1));

    p.push(&[0x00]);
    assert_eq!(p.next_frame().unwrap(), FrameState::Ready(vec![0xC0, 0x00]));
}

#[test]
fn t_connect_fragmented_header_and_body() {
    let frame = vec![
        0x10, 0x0f, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x02, 0x00, 0x1e, 0x00, 0x03, b'c',
        b'i', b'd',
    ];

    let mut p = FrameParser::default();

    p.push(&frame[..1]);
    assert_eq!(p.next_frame().unwrap(), FrameState::Need(1));

    p.push(&frame[1..4]); // now fixed header + 2 body bytes are available
    assert_eq!(p.next_frame().unwrap(), FrameState::Need(13));

    p.push(&frame[4..]);
    assert_eq!(p.next_frame().unwrap(), FrameState::Ready(frame));
}

#[test]
fn t_two_frames_in_one_chunk() {
    let mut p = FrameParser::default();
    p.push(&[0xC0, 0x00, 0xD0, 0x00]);

    assert_eq!(p.next_frame().unwrap(), FrameState::Ready(vec![0xC0, 0x00]));
    assert_eq!(p.next_frame().unwrap(), FrameState::Ready(vec![0xD0, 0x00]));
    assert_eq!(p.next_frame().unwrap(), FrameState::Need(1));
}

#[test]
fn t_fragmented_across_packet_boundary() {
    let mut p = FrameParser::default();
    p.push(&[0xC0, 0x00, 0xD0]);

    assert_eq!(p.next_frame().unwrap(), FrameState::Ready(vec![0xC0, 0x00]));
    assert_eq!(p.next_frame().unwrap(), FrameState::Need(1));

    p.push(&[0x00]);
    assert_eq!(p.next_frame().unwrap(), FrameState::Ready(vec![0xD0, 0x00]));
}

#[test]
fn t_next_packet_returns_decoded_packets() {
    let mut p = FrameParser::default();
    p.push(&[0xC0, 0x00, 0xD0, 0x00]);

    assert_eq!(p.next_packet().unwrap(), Some(Packet::PingReq));
    assert_eq!(p.next_packet().unwrap(), Some(Packet::PingResp));
    assert_eq!(p.next_packet().unwrap(), None);
}

#[test]
fn t_malformed_remaining_length_is_propagated() {
    let mut p = FrameParser::default();
    p.push(&[0x30, 0x80, 0x80, 0x80, 0x80, 0x00]);
    assert_eq!(p.next_frame().unwrap_err(), Error::MalformedRemainingLength);
}
