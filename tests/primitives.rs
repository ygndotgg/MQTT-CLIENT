use mqtt_client::codec::*;
use mqtt_client::protocol::v4::error::Error;

#[test]
fn t_read_u16_insufficient() {
    let mut input: &[u8] = &[0x12];
    let err = read_u16(&mut input).unwrap_err();
    assert_eq!(err, Error::InsufficientBytes(1));
}

#[test]
fn t_mqtt_string_roundtrip() {
    let mut out = Vec::new();
    write_mqtt_string(&mut out, "rumqtt");

    let mut input: &[u8] = &out;
    let got = read_mqtt_string(&mut input).unwrap();
    assert_eq!(got, "rumqtt");
    assert!(input.is_empty());
}

#[test]
fn t_mqtt_string_insufficient_len_prefix() {
    let mut input: &[u8] = &[0x00];
    let err = read_mqtt_string(&mut input).unwrap_err();
    assert_eq!(err, Error::InsufficientBytes(1));
}

#[test]
fn t_mqtt_bytes_roundtrip() {
    let mut out = Vec::new();
    write_mqtt_bytes(&mut out, &[1, 2, 3, 4]);

    let mut input: &[u8] = &out;
    let got = read_mqtt_bytes(&mut input).unwrap();
    assert_eq!(got, vec![1, 2, 3, 4]);
    assert!(input.is_empty());
}

#[test]
fn t_remaining_len_vectors() {
    assert_eq!(encode_remaining_length(0).unwrap(), vec![0x00]);
    assert_eq!(encode_remaining_length(127).unwrap(), vec![0x7f]);
    assert_eq!(encode_remaining_length(128).unwrap(), vec![0x80, 0x01]);
    assert_eq!(encode_remaining_length(16383).unwrap(), vec![0xff, 0x7f]);
    assert_eq!(
        encode_remaining_length(268_435_455).unwrap(),
        vec![0xff, 0xff, 0xff, 0x7f]
    );

    assert_eq!(decode_remaining_length(&[0x00]).unwrap(), (0, 1));
    assert_eq!(decode_remaining_length(&[0x7f]).unwrap(), (127, 1));
    assert_eq!(decode_remaining_length(&[0x80, 0x01]).unwrap(), (128, 2));
    assert_eq!(decode_remaining_length(&[0xff, 0x7f]).unwrap(), (16383, 2));
    assert_eq!(
        decode_remaining_length(&[0xff, 0xff, 0xff, 0x7f]).unwrap(),
        (268_435_455, 4)
    );
}

#[test]
fn t_remaining_len_reject_5th_byte() {
    let err = decode_remaining_length(&[0x80, 0x80, 0x80, 0x80, 0x00]).unwrap_err();
    assert_eq!(err, Error::MalformedRemainingLength);
}

#[test]
fn t_remaining_len_truncated_continuation() {
    let err = decode_remaining_length(&[0x80]).unwrap_err();
    assert_eq!(err, Error::InsufficientBytes(1));
}
