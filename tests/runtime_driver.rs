use std::time::{Duration, Instant};

use mqtt_client::runtime::driver::{DriverAction, DriverEvent, RuntimeDriver};
use mqtt_client::runtime::state::{Completion, RuntimeState};
use mqtt_client::types::{Command, Packet, PubAck, PubComp, PubRec, Publish, Qos};

fn qos1_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::AtLeastOnce,
        retain: false,
        topic: "driver/qos1".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

fn qos2_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::ExactlyOnce,
        retain: false,
        topic: "driver/qos2".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

#[test]
fn tick_timeout_triggers_reconnect_action() {
    let mut state = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    state.set_active(true);
    state.await_pingresp = true;
    let base = Instant::now();
    state.last_outgoing = base;

    let mut driver = RuntimeDriver::new(state, false);
    let out = driver
        .handle_event(DriverEvent::Tick(base + Duration::from_secs(5)))
        .unwrap();

    assert_eq!(out, vec![DriverAction::TriggerReconnect]);
}

#[test]
fn incoming_puback_emits_completion_action() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    driver.state.set_active(true);

    let sent = driver
        .handle_event(DriverEvent::Command(Command::Publish {
            client_id: 1,
            token_id: 7,
            publish: qos1_publish(0),
        }))
        .unwrap();

    let pkid = match &sent[0] {
        DriverAction::Send(Packet::Publish(p)) => p.pkid,
        _ => panic!("expected outgoing publish"),
    };

    let out = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubAck(PubAck { pkid }),
            Instant::now(),
        ))
        .unwrap();

    assert!(out
        .iter()
        .any(|a| matches!(a, DriverAction::CompleteFor { client_id: 1, completion: Completion::PubAck { token_id: 7, pkid: x } } if *x == pkid)));
}

#[test]
fn incoming_pubcomp_emits_completion_action() {
    let mut driver = RuntimeDriver::new(RuntimeState::new(8), false);
    driver.state.set_active(true);

    let sent = driver
        .handle_event(DriverEvent::Command(Command::Publish {
            client_id: 1,
            token_id: 21,
            publish: qos2_publish(0),
        }))
        .unwrap();

    let pkid = match &sent[0] {
        DriverAction::Send(Packet::Publish(p)) => p.pkid,
        _ => panic!("expected outgoing publish"),
    };

    let rel_actions = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubRec(PubRec { pkid }),
            Instant::now(),
        ))
        .unwrap();
    assert!(rel_actions
        .iter()
        .any(|a| matches!(a, DriverAction::Send(Packet::PubRel(_)))));

    let out = driver
        .handle_event(DriverEvent::Incoming(
            Packet::PubComp(PubComp { pkid }),
            Instant::now(),
        ))
        .unwrap();

    assert!(out
        .iter()
        .any(|a| matches!(a, DriverAction::CompleteFor { client_id: 1, completion: Completion::PubComp { token_id: 21, pkid: x } } if *x == pkid)));
}
