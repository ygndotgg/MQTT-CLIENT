use mqtt_client::codec::{decode_packet, encode_packet};
use mqtt_client::protocol::v4::error::Error;
use mqtt_client::types::{Packet, Qos, SubAck, Subscribe, UnsubAck, Unsubscribe};

#[test]
fn t_subscribe_roundtrip_and_golden() {
    let p = Packet::Subscribe(Subscribe {
        pkid: 10,
        filters: vec![
            ("a".into(), Qos::AtMostOnce),
            ("bc".into(), Qos::ExactlyOnce),
        ],
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(
        out,
        vec![
            0x82, 0x0b, // fixed header
            0x00, 0x0a, // pkid
            0x00, 0x01, b'a', 0x00, // topic + qos0
            0x00, 0x02, b'b', b'c', 0x02, // topic + qos2
        ]
    );
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_decode_subscribe_reject_wrong_flags() {
    let pkt = vec![0x80, 0x05, 0x00, 0x01, 0x00, 0x01, b'a'];
    assert_eq!(
        decode_packet(&pkt).unwrap_err(),
        Error::InvalidFixedHeaderFlags {
            packet_type: 8,
            flags: 0,
        }
    );
}

#[test]
fn t_decode_subscribe_reject_pkid_zero() {
    let pkt = vec![0x82, 0x05, 0x00, 0x00, 0x00, 0x01, b'a', 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::PacketIdZero);
}

#[test]
fn t_decode_subscribe_reject_qos_3() {
    let pkt = vec![0x82, 0x06, 0x00, 0x01, 0x00, 0x01, b'a', 0x03];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::InvalidQos(3));
}

#[test]
fn t_unsubscribe_roundtrip_and_golden() {
    let p = Packet::UnSubscribe(Unsubscribe {
        pkid: 7,
        filters: vec!["a".into(), "bc".into()],
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(
        out,
        vec![
            0xA2, 0x09, // fixed header
            0x00, 0x07, // pkid
            0x00, 0x01, b'a', // topic1
            0x00, 0x02, b'b', b'c', // topic2
        ]
    );
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_decode_unsubscribe_reject_wrong_flags() {
    let pkt = vec![0xA0, 0x05, 0x00, 0x01, 0x00, 0x01, b'a'];
    assert_eq!(
        decode_packet(&pkt).unwrap_err(),
        Error::InvalidFixedHeaderFlags {
            packet_type: 10,
            flags: 0,
        }
    );
}

#[test]
fn t_decode_unsubscribe_reject_pkid_zero() {
    let pkt = vec![0xA2, 0x05, 0x00, 0x00, 0x00, 0x01, b'a'];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::PacketIdZero);
}

#[test]
fn t_suback_roundtrip_and_golden() {
    let p = Packet::SubAck(SubAck {
        pkid: 9,
        return_codes: vec![0x00, 0x01, 0x80],
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(out, vec![0x90, 0x05, 0x00, 0x09, 0x00, 0x01, 0x80]);
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_decode_suback_reject_invalid_return_code() {
    let pkt = vec![0x90, 0x03, 0x00, 0x01, 0x03];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_unsuback_roundtrip_and_golden() {
    let p = Packet::UnsubAck(UnsubAck { pkid: 9 });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(out, vec![0xB0, 0x02, 0x00, 0x09]);
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_decode_unsuback_reject_remaining_len_not_2() {
    let pkt = vec![0xB0, 0x01, 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_decode_unsuback_reject_pkid_zero() {
    let pkt = vec![0xB0, 0x02, 0x00, 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::PacketIdZero);
}
