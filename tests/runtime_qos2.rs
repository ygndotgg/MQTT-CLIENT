use mqtt_client::runtime::state::{Completion, IncomingQos2Result, RuntimeError, RuntimeState};
use mqtt_client::types::{Command, Packet, Publish, Qos};

fn qos2_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::ExactlyOnce,
        retain: false,
        topic: "qos/2".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

#[test]
fn outgoing_qos2_pubrec_then_pubcomp_completes() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(true);

    let out = rt
        .on_command_publish(Command::Publish {
            token_id: 11,
            publish: qos2_publish(0),
        })
        .unwrap();

    let pkid = match out {
        Some(Packet::Publish(p)) => p.pkid,
        _ => panic!("expected publish"),
    };
    assert_ne!(pkid, 0);

    let rel = rt.on_pubrec_checked(pkid).unwrap();
    assert_eq!(rel, Packet::PubRel(mqtt_client::types::PubRel { pkid }));
    assert!(rt.inflight.outgoing_rel[pkid as usize]);

    let done = rt.on_pubcomp_checked(pkid).unwrap();
    assert_eq!(
        done.completion,
        Completion::PubComp {
            token_id: 11,
            pkid
        }
    );
    assert!(!rt.inflight.outgoing_rel[pkid as usize]);
    assert!(rt.inflight.outgoing[pkid as usize].is_none());
}

#[test]
fn outgoing_qos2_unsolicited_pubrec_is_error() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(true);

    let err = rt.on_pubrec_checked(1).unwrap_err();
    assert_eq!(err, RuntimeError::UnsolicitedPubRec(1));
}

#[test]
fn outgoing_qos2_unsolicited_pubcomp_is_error() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(true);

    let out = rt
        .on_command_publish(Command::Publish {
            token_id: 22,
            publish: qos2_publish(1),
        })
        .unwrap();
    assert!(out.is_some());

    let err = rt.on_pubcomp_checked(1).unwrap_err();
    assert_eq!(err, RuntimeError::UnsolicitedPubComp(1));
}

#[test]
fn incoming_qos2_duplicate_publish_is_suppressed() {
    let mut rt = RuntimeState::new(8);

    let first = rt.on_incoming_qos2_publish_checked(5).unwrap();
    match first {
        IncomingQos2Result::FirstSeen { pubrec } => {
            assert_eq!(pubrec, Packet::PubRec(mqtt_client::types::PubRec { pkid: 5 }))
        }
        _ => panic!("expected FirstSeen"),
    }
    assert!(rt.inflight.incoming_pub[5]);

    let dup = rt.on_incoming_qos2_publish_checked(5).unwrap();
    match dup {
        IncomingQos2Result::Duplicate { pubrec } => {
            assert_eq!(pubrec, Packet::PubRec(mqtt_client::types::PubRec { pkid: 5 }))
        }
        _ => panic!("expected Duplicate"),
    }
    assert!(rt.inflight.incoming_pub[5]);

    let comp = rt.on_incoming_pubrel_checked(5).unwrap();
    assert_eq!(comp, Packet::PubComp(mqtt_client::types::PubComp { pkid: 5 }));
    assert!(!rt.inflight.incoming_pub[5]);
}

#[test]
fn incoming_qos2_unsolicited_pubrel_is_error() {
    let mut rt = RuntimeState::new(8);
    let err = rt.on_incoming_pubrel_checked(7).unwrap_err();
    assert_eq!(err, RuntimeError::UnsolicitedPubRel(7));
}

#[test]
fn outgoing_qos2_pubcomp_promotes_collision() {
    let mut rt = RuntimeState::new(2);
    rt.set_active(true);

    let first = rt
        .on_command_publish(Command::Publish {
            token_id: 1,
            publish: qos2_publish(1),
        })
        .unwrap();
    assert!(first.is_some());

    let second = rt
        .on_command_publish(Command::Publish {
            token_id: 2,
            publish: qos2_publish(1),
        })
        .unwrap();
    assert!(second.is_none());
    assert!(rt.inflight.collision.is_some());

    let rel = rt.on_pubrec_checked(1).unwrap();
    assert_eq!(rel, Packet::PubRel(mqtt_client::types::PubRel { pkid: 1 }));

    let done = rt.on_pubcomp_checked(1).unwrap();
    assert_eq!(
        done.completion,
        Completion::PubComp {
            token_id: 1,
            pkid: 1
        }
    );

    match done.next_packet {
        Some(Packet::Publish(p)) => {
            assert_eq!(p.pkid, 1);
            assert!(p.dup);
        }
        _ => panic!("expected promoted publish"),
    }
}
