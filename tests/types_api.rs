use mqtt_client::types::{
    Command, ConnAck, Filter, Packet, PacketType, Publish, Qos, Subscribe, Unsubscribe,
};

#[test]
fn t_packet_type_mapping() {
    let packet = Packet::ConnAck(ConnAck {
        session_present: false,
        code: 0,
    });
    assert_eq!(packet.packet_type(), PacketType::ConnAck);
    assert_eq!(u8::from(PacketType::ConnAck), 2);
    assert_eq!(PacketType::try_from(2).unwrap(), PacketType::ConnAck);
}

#[test]
fn t_minimal_command_variants_are_constructible() {
    let publish = Publish {
        dup: false,
        qos: Qos::AtLeastOnce,
        retain: false,
        topic: "a/b".into(),
        pkid: 1,
        payload: b"hello".to_vec(),
    };
    let subscribe = Subscribe {
        pkid: 2,
        filters: vec![Filter {
            path: "a/#".into(),
            qos: Qos::AtMostOnce,
        }],
    };
    let unsubscribe = Unsubscribe {
        pkid: 3,
        filters: vec!["a/#".into()],
    };

    let _ = Command::Publish {
        token_id: 1,
        publish,
    };
    let _ = Command::Subscribe {
        token_id: 2,
        subscribe,
    };
    let _ = Command::Unsubscribe {
        token_id: 3,
        unsubscribe,
    };
    let _ = Command::Ping;
    let _ = Command::Disconnect;
}
