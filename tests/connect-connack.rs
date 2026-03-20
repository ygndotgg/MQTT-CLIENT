
use mqtt_client::codec::{decode_packet, encode_packet};
use mqtt_client::protocol::v4::error::Error;
use mqtt_client::types::{ConnAck, Connect, LastWill, Packet, Qos};

#[test]
fn t_connect_golden_minimal_clean_session() {
    let p = Packet::Connect(Connect {
        clean_session: true,
        keep_alive: 30,
        client_id: "cid".into(),
        will: None,
        username: None,
        password: None,
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();

    let golden = vec![
        0x10, 0x0f, // fixed header
        0x00, 0x04, b'M', b'Q', b'T', b'T', // protocol name
        0x04, // level
        0x02, // connect flags (clean session)
        0x00, 0x1e, // keep alive
        0x00, 0x03, b'c', b'i', b'd', // client id
    ];
    assert_eq!(out, golden);

    let decoded = decode_packet(&golden).unwrap();
    assert_eq!(decoded, p);
}

#[test]
fn t_connect_golden_with_will_username_password() {
    let p = Packet::Connect(Connect {
        clean_session: true,
        keep_alive: 0,
        client_id: "c".into(),
        will: Some(LastWill {
            topic: "wt".into(),
            message: vec![1, 2],
            qos: Qos::AtLeastOnce,
            retain: false,
        }),
        username: Some("u".into()),
        password: Some(b"pw".to_vec()),
    });

    let mut out = Vec::new();
    encode_packet(&p, &mut out).unwrap();

    let golden = vec![
        0x10, 0x1c, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04,
        0xce, // user+pass+will(qos1)+clean
        0x00, 0x00, 0x00, 0x01, b'c', // client id
        0x00, 0x02, b'w', b't', // will topic
        0x00, 0x02, 0x01, 0x02, // will message
        0x00, 0x01, b'u', // username
        0x00, 0x02, b'p', b'w', // password bytes
    ];
    assert_eq!(out, golden);

    let decoded = decode_packet(&golden).unwrap();
    assert_eq!(decoded, p);
}

#[test]
fn t_connack_golden_success_and_error() {
    let ok = Packet::ConnAck(ConnAck {
        session_present: false,
        code: 0x00,
    });
    let err = Packet::ConnAck(ConnAck {
        session_present: false,
        code: 0x02,
    });

    let mut out_ok = Vec::new();
    encode_packet(&ok, &mut out_ok).unwrap();
    assert_eq!(out_ok, vec![0x20, 0x02, 0x00, 0x00]);
    assert_eq!(decode_packet(&out_ok).unwrap(), ok);

    let mut out_err = Vec::new();
    encode_packet(&err, &mut out_err).unwrap();
    assert_eq!(out_err, vec![0x20, 0x02, 0x00, 0x02]);
    assert_eq!(decode_packet(&out_err).unwrap(), err);
}

#[test]
fn t_connect_roundtrip_variants() {
    let cases = vec![
        Packet::Connect(Connect {
            clean_session: true,
            keep_alive: 5,
            client_id: "a".into(),
            will: None,
            username: None,
            password: None,
        }),
        Packet::Connect(Connect {
            clean_session: false,
            keep_alive: 10,
            client_id: "b".into(),
            will: Some(LastWill {
                topic: "topic".into(),
                message: vec![9, 8, 7],
                qos: Qos::ExactlyOnce,
                retain: true,
            }),
            username: Some("user".into()),
            password: Some(vec![0xaa, 0xbb]),
        }),
    ];

    for p in cases {
        let mut out = Vec::new();
        encode_packet(&p, &mut out).unwrap();
        let got = decode_packet(&out).unwrap();
        assert_eq!(got, p);
    }
}

#[test]
fn t_connack_roundtrip_valid_codes() {
    for code in 0x00..=0x05 {
        let p = Packet::ConnAck(ConnAck {
            session_present: code == 0,
            code,
        });
        let mut out = Vec::new();
        encode_packet(&p, &mut out).unwrap();
        assert_eq!(decode_packet(&out).unwrap(), p);
    }
}

#[test]
fn t_decode_connect_reject_wrong_protocol() {
    let mut pkt = vec![
        0x10, 0x0f, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x02, 0x00, 0x1e, 0x00, 0x03, b'c',
        b'i', b'd',
    ];
    pkt[4] = b'X';
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::InvalidProtocol);
}

#[test]
fn t_decode_connect_reject_wrong_level() {
    let mut pkt = vec![
        0x10, 0x0f, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x02, 0x00, 0x1e, 0x00, 0x03, b'c',
        b'i', b'd',
    ];
    pkt[8] = 0x05;
    assert_eq!(
        decode_packet(&pkt).unwrap_err(),
        Error::InvalidProtocolLevel(0x05)
    );
}

#[test]
fn t_decode_connect_reject_reserved_flag_bit0() {
    let mut pkt = vec![
        0x10, 0x0f, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x02, 0x00, 0x1e, 0x00, 0x03, b'c',
        b'i', b'd',
    ];
    pkt[9] = 0x03; // clean + reserved bit0
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_decode_connect_reject_will_matrix_invalid() {
    // will_flag=0 but will_qos=1
    let pkt = vec![
        0x10, 0x0f, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x08, 0x00, 0x1e, 0x00, 0x03, b'c',
        b'i', b'd',
    ];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_decode_connect_reject_password_without_username() {
    // flags: clean + password (username missing)
    let pkt = vec![
        0x10, 0x0f, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x42, 0x00, 0x1e, 0x00, 0x03, b'c',
        b'i', b'd',
    ];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_decode_connect_reject_truncated_or_missing_will_payload() {
    // will_flag set but no will topic/message in payload
    let pkt = vec![
        0x10, 0x0f, 0x00, 0x04, b'M', b'Q', b'T', b'T', 0x04, 0x06, 0x00, 0x1e, 0x00, 0x03, b'c',
        b'i', b'd',
    ];
    assert!(matches!(
        decode_packet(&pkt),
        Err(Error::InsufficientBytes(_))
    ));
}

#[test]
fn t_decode_connack_reject_len_not_2() {
    let pkt = vec![0x20, 0x01, 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_decode_connack_reject_invalid_ack_flags() {
    let pkt = vec![0x20, 0x02, 0x02, 0x00];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_decode_connack_reject_invalid_code() {
    let pkt = vec![0x20, 0x02, 0x00, 0x06];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}

#[test]
fn t_decode_connack_reject_error_code_with_session_present() {
    let pkt = vec![0x20, 0x02, 0x01, 0x02];
    assert_eq!(decode_packet(&pkt).unwrap_err(), Error::MalformedPacket);
}
