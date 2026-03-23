use std::time::{Duration, Instant};

use mqtt_client::runtime::driver::{DriverAction, DriverEvent, RuntimeDriver};
use mqtt_client::runtime::state::{Completion, RuntimeError, RuntimeState};
use mqtt_client::types::{Command, Packet, PubAck, PubRec, PubRel, Publish, Qos};

fn qos1_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::AtLeastOnce,
        retain: false,
        topic: "driver/matrix/qos1".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

fn qos2_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::ExactlyOnce,
        retain: false,
        topic: "driver/matrix/qos2".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

#[test]
fn tick_idle_emits_pingreq() {
    let mut state = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    state.set_active(true);
    let base = Instant::now();
    state.last_incoming = base;
    state.last_outgoing = base;
    let mut driver = RuntimeDriver::new(state, false);

    let out = driver
        .handle_event(DriverEvent::Tick(base + Duration::from_secs(5)))
        .unwrap();
    assert_eq!(out, vec![DriverAction::Send(Packet::PingReq)]);
}

#[test]
fn tick_inactive_emits_no_actions() {
    let mut state = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    state.set_active(false);
    let mut driver = RuntimeDriver::new(state, false);

    let out = driver.handle_event(DriverEvent::Tick(Instant::now())).unwrap();
    assert!(out.is_empty());
}

#[test]
fn connection_lost_event_triggers_reconnect_and_moves_to_pending() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    driver.state.set_active(true);

    let first = driver
        .handle_event(DriverEvent::Command(Command::Publish {
            token_id: 1,
            publish: qos1_publish(1),
        }))
        .unwrap();
    assert!(matches!(
        first.as_slice(),
        [DriverAction::Send(Packet::Publish(_))]
    ));

    let second = driver
        .handle_event(DriverEvent::Command(Command::Publish {
            token_id: 2,
            publish: qos1_publish(1),
        }))
        .unwrap();
    assert!(second.is_empty());
    assert!(driver.state.inflight.collision.is_some());

    let out = driver
        .handle_event(DriverEvent::ConnectionLost { clean_session: false })
        .unwrap();
    assert_eq!(out, vec![DriverAction::TriggerReconnect]);
    assert_eq!(driver.state.inflight.pending.len(), 2);
}

#[test]
fn connection_restored_replays_pending_with_dup_for_qos1() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    driver.state.set_active(false);
    driver
        .handle_event(DriverEvent::Command(Command::Publish {
            token_id: 77,
            publish: qos1_publish(0),
        }))
        .unwrap();
    assert_eq!(driver.state.inflight.pending.len(), 1);

    let out = driver
        .handle_event(DriverEvent::ConnectionRestored(Instant::now()))
        .unwrap();
    assert_eq!(out.len(), 1);
    match &out[0] {
        DriverAction::Send(Packet::Publish(p)) => {
            assert!(p.dup);
            assert_ne!(p.pkid, 0);
        }
        _ => panic!("expected replayed publish"),
    }
}

#[test]
fn incoming_pingresp_clears_wait_flag() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    driver.state.await_pingresp = true;
    let out = driver
        .handle_event(DriverEvent::Incoming(Packet::PingResp, Instant::now()))
        .unwrap();
    assert!(out.is_empty());
    assert!(!driver.state.await_pingresp);
}

#[test]
fn incoming_pubrec_emits_pubrel() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    driver.state.set_active(true);

    let sent = driver
        .handle_event(DriverEvent::Command(Command::Publish {
            token_id: 4,
            publish: qos2_publish(0),
        }))
        .unwrap();
    let pkid = match &sent[0] {
        DriverAction::Send(Packet::Publish(p)) => p.pkid,
        _ => panic!("expected outgoing publish"),
    };

    let out = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubRec(PubRec { pkid }),
            Instant::now(),
        ))
        .unwrap();
    assert_eq!(
        out,
        vec![DriverAction::Send(Packet::PubRel(mqtt_client::types::PubRel {
            pkid
        }))]
    );
}

#[test]
fn incoming_qos2_publish_first_and_duplicate_both_emit_pubrec() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    let publish = qos2_publish(5);

    let first = driver
        .handle_event(DriverEvent::Incoming(
            Packet::Publish(publish.clone()),
            Instant::now(),
        ))
        .unwrap();
    assert_eq!(
        first,
        vec![DriverAction::Send(Packet::PubRec(mqtt_client::types::PubRec {
            pkid: 5
        }))]
    );

    let dup = driver
        .handle_event(DriverEvent::Incoming(Packet::Publish(publish), Instant::now()))
        .unwrap();
    assert_eq!(
        dup,
        vec![DriverAction::Send(Packet::PubRec(mqtt_client::types::PubRec {
            pkid: 5
        }))]
    );
}

#[test]
fn incoming_pubrel_tracked_emits_pubcomp_and_unsolicited_errors() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);

    driver
        .handle_event(DriverEvent::Incoming(
            Packet::Publish(qos2_publish(6)),
            Instant::now(),
        ))
        .unwrap();

    let out = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubRel(PubRel { pkid: 6 }),
            Instant::now(),
        ))
        .unwrap();
    assert_eq!(
        out,
        vec![DriverAction::Send(Packet::PubComp(mqtt_client::types::PubComp {
            pkid: 6
        }))]
    );

    let err = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubRel(PubRel { pkid: 7 }),
            Instant::now(),
        ))
        .unwrap_err();
    assert_eq!(err, RuntimeError::UnsolicitedPubRel(7));
}

#[test]
fn puback_completion_and_collision_promotion_are_both_emitted() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(2), false);
    driver.state.set_active(true);

    let first = driver
        .handle_event(DriverEvent::Command(Command::Publish {
            token_id: 10,
            publish: qos1_publish(1),
        }))
        .unwrap();
    assert!(matches!(
        first.as_slice(),
        [DriverAction::Send(Packet::Publish(_))]
    ));

    let second = driver
        .handle_event(DriverEvent::Command(Command::Publish {
            token_id: 11,
            publish: qos1_publish(1),
        }))
        .unwrap();
    assert!(second.is_empty());

    let out = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubAck(PubAck { pkid: 1 }),
            Instant::now(),
        ))
        .unwrap();

    assert!(out.iter().any(|a| {
        matches!(
            a,
            DriverAction::Complete(Completion::PubAck {
                token_id: 10,
                pkid: 1
            })
        )
    }));
    assert!(out
        .iter()
        .any(|a| matches!(a, DriverAction::Send(Packet::Publish(p)) if p.pkid == 1 && p.dup)));
}

#[test]
fn unsolicited_puback_propagates_error() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    let err = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubAck(PubAck { pkid: 77 }),
            Instant::now(),
        ))
        .unwrap_err();
    assert_eq!(err, RuntimeError::UnsolicitedAck(77));
}
