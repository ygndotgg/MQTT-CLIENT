use mqtt_client::codec::{decode_packet, encode_packet};
use mqtt_client::protocol::v4::error::Error;
use mqtt_client::types::{Packet, PubAck, PubComp, PubRec, PubRel, Publish, Qos};

#[test]
fn t_publish_qos0_no_pkid_on_wire_roundtrip() {
    let p = Packet::Publish(Publish {
        dup: false,
        qos: Qos::AtMostOnce,
        retain: false,
        topic: "t".into(),
        pkid: 0,
        payload: vec![0x01, 0x02],
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(out, vec![0x30, 0x05, 0x00, 0x01, b't', 0x01, 0x02]);
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_publish_qos1_roundtrip() {
    let p = Packet::Publish(Publish {
        dup: false,
        qos: Qos::AtLeastOnce,
        retain: false,
        topic: "ab".into(),
        pkid: 10,
        payload: vec![0xaa, 0xbb],
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(
        out,
        vec![0x32, 0x08, 0x00, 0x02, b'a', b'b', 0x00, 0x0a, 0xaa, 0xbb]
    );
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_publish_qos2_roundtrip() {
    let p = Packet::Publish(Publish {
        dup: true,
        qos: Qos::ExactlyOnce,
        retain: true,
        topic: "z".into(),
        pkid: 7,
        payload: vec![0x10],
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();
    assert_eq!(out, vec![0x3d, 0x06, 0x00, 0x01, b'z', 0x00, 0x07, 0x10]);
    assert_eq!(decode_packet(&out).unwrap(), p);
}

#[test]
fn t_decode_publish_reject_qos_bits_3() {
    let pkt = vec![0x36, 0x03, 0x00, 0x01, b'a'];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::InvalidQos(3));
}

#[test]
fn t_publish_reject_qos1_qos2_pkid_zero_encode_and_decode() {
    for (qos, header) in [(Qos::AtLeastOnce, 0x32u8), (Qos::ExactlyOnce, 0x34u8)] {
        let p = Packet::Publish(Publish {
            dup: false,
            qos,
            retain: false,
            topic: "a".into(),
            pkid: 0,
            payload: vec![],
        });
        let mut out = Vec::new();
        assert_eq!(encode_packet(&p, &mut out).unwrap_err(), Error::PacketIdZero);

        let pkt = vec![header, 0x05, 0x00, 0x01, b'a', 0x00, 0x00];
        assert_eq!(decode_packet(&pkt).unwrap_err(), Error::PacketIdZero);
    }
}

#[test]
fn t_decode_pubrel_reject_flags_not_0b0010() {
    let pkt = vec![0x60, 0x02, 0x00, 0x01];
    assert_eq!(
        decode_packet(&pkt).unwrap_err(),
        Error::InvalidFixedHeaderFlags {
            packet_type: 6,
            flags: 0,
        }
    );
}

#[test]
fn t_decode_puback_pubrec_pubcomp_reject_nonzero_flags() {
    let cases = [(0x41, 4u8, 1u8), (0x51, 5u8, 1u8), (0x71, 7u8, 1u8)];
    for (byte1, packet_type, flags) in cases {
        let pkt = vec![byte1, 0x02, 0x00, 0x01];
        assert_eq!(
            decode_packet(&pkt).unwrap_err(),
            Error::InvalidFixedHeaderFlags { packet_type, flags }
        );
    }
}

#[test]
fn t_decode_ack_family_reject_remaining_length_not_2() {
    let cases = [0x40u8, 0x50u8, 0x62u8, 0x70u8];
    for byte1 in cases {
        let pkt = vec![byte1, 0x01, 0x00];
        assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
    }
}

#[test]
fn t_decode_ack_family_reject_pkid_zero() {
    let cases = [0x40u8, 0x50u8, 0x62u8, 0x70u8];
    for byte1 in cases {
        let pkt = vec![byte1, 0x02, 0x00, 0x00];
        assert_eq!(decode_packet(&pkt).unwrap_err(), Error::PacketIdZero);
    }
}

#[test]
fn t_ack_family_roundtrip_valid() {
    let packets = [
        Packet::PubAck(PubAck { pkid: 1 }),
        Packet::PubRec(PubRec { pkid: 2 }),
        Packet::PubRel(PubRel { pkid: 3 }),
        Packet::PubComp(PubComp { pkid: 4 }),
    ];

    for p in packets {
        let mut out = Vec::new();
        encode_packet(&p, &mut out).unwrap();
        assert_eq!(decode_packet(&out).unwrap(), p);
    }
}
