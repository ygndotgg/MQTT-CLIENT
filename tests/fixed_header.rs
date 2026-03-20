use mqtt_client::{codec::parse_fixed_header, protocol::v4::error::Error};
#[test]
fn t_short_buffer_need_mroe_bytes() {
    let errr = parse_fixed_header(&[0x30]).unwrap_err();
    assert_eq!(errr, Error::InsufficientBytes(1));
}

#[test]
fn t_frame_length_calculation() {
    let fh = parse_fixed_header(&[0x30, 0x07]).unwrap();
    assert_eq!(fh.header_len, 2);
    assert_eq!(fh.remaining_len, 7);
    assert_eq!(fh.frame_len(), 9);
}

#[test]
fn t_packet_type_and_flags_nibbles() {
    let fh = parse_fixed_header(&[0x3d, 0x00]).unwrap();
    assert_eq!(fh.packet_type(), 0x03);
    assert_eq!(fh.flags(), 0x0d);
}

// #[test]
// fn t_malformed_remaining_length() {
//     let err = parse_fixed_header(&[0x30, 0x80, 0x80, 0x80, 0x00]).unwrap_err();
//     assert_eq!(err, Error::MalformedRemainingLength);
// }
