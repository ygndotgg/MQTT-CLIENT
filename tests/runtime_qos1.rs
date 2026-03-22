use mqtt_client::runtime::state::{Completion, RuntimeError, RuntimeState};
use mqtt_client::types::{Command, Packet, Publish, Qos};

fn qos1_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::AtLeastOnce,
        retain: false,
        topic: "qos/1".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

#[test]
fn command_publish_assigns_pkid_inserts_inflight_and_returns_wire_packet() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(true);

    let cmd = Command::Publish {
        token_id: 42,
        publish: qos1_publish(0),
    };
    let out = rt.on_command_publish(cmd).unwrap();
    match out {
        Some(Packet::Publish(p)) => {
            assert_ne!(p.pkid, 0);
            assert!(!p.dup);
            assert_eq!(rt.inflight.inflight, 1);
            assert!(rt.inflight.outgoing[p.pkid as usize].is_some());
        }
        _ => panic!("expected outgoing publish"),
    }
}

#[test]
fn offline_publish_goes_to_pending() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(false);

    let out = rt
        .on_command_publish(Command::Publish {
            token_id: 7,
            publish: qos1_publish(0),
        })
        .unwrap();
    assert!(out.is_none());
    assert_eq!(rt.inflight.pending.len(), 1);
}

#[test]
fn reconnect_resend_sets_dup_true_for_qos1() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(false);
    rt.on_command_publish(Command::Publish {
        token_id: 9,
        publish: qos1_publish(0),
    })
    .unwrap();

    let out = rt.resend_pending_on_reconnect();
    assert_eq!(out.len(), 1);
    match &out[0] {
        Packet::Publish(p) => {
            assert!(p.dup);
            assert_ne!(p.pkid, 0);
        }
        _ => panic!("expected publish on resend"),
    }
}

#[test]
fn puback_returns_completion_with_token_id() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(true);

    let out = rt
        .on_command_publish(Command::Publish {
            token_id: 99,
            publish: qos1_publish(0),
        })
        .unwrap();
    let pkid = match out {
        Some(Packet::Publish(p)) => p.pkid,
        _ => panic!("expected publish"),
    };

    let ack = rt.on_puback_checked(pkid).unwrap();
    assert_eq!(
        ack.completion,
        Completion::PubAck {
            token_id: 99,
            pkid
        }
    );
}

#[test]
fn puback_promotes_collision_and_returns_next_packet() {
    let mut rt = RuntimeState::new(2);
    rt.set_active(true);

    // First publish occupies pkid=1
    let first = rt
        .on_command_publish(Command::Publish {
            token_id: 1,
            publish: qos1_publish(1),
        })
        .unwrap();
    assert!(first.is_some());

    // Same pkid collision gets deferred
    let second = rt
        .on_command_publish(Command::Publish {
            token_id: 2,
            publish: qos1_publish(1),
        })
        .unwrap();
    assert!(second.is_none());
    assert!(rt.inflight.collision.is_some());

    let ack = rt.on_puback_checked(1).unwrap();
    match ack.next_packet {
        Some(Packet::Publish(p)) => {
            assert_eq!(p.pkid, 1);
            assert!(p.dup);
        }
        _ => panic!("expected promoted publish"),
    }
}

#[test]
fn unsolicited_puback_returns_error() {
    let mut rt = RuntimeState::new(8);
    rt.set_active(true);
    let err = rt.on_puback_checked(77).unwrap_err();
    assert_eq!(err, RuntimeError::UnsolicitedAck(77));
}
