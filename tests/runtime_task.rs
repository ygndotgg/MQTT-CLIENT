use std::sync::mpsc::channel;

use mqtt_client::runtime::{
    driver::RuntimeDriver,
    events::RuntimeEvent,
    state::RuntimeState,
    task::{RuntimeTask, RuntimeTaskError},
};
use mqtt_client::types::{Command, Packet, Publish, Qos};

fn qos1_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::AtLeastOnce,
        retain: false,
        topic: "task/qos1".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

#[test]
fn run_once_on_publish_pushes_wire_packet_to_outbox() {
    let (command_tx, command_rx) = channel();
    let (event_tx, _event_rx) = channel();

    let mut state = RuntimeState::new(8);
    state.set_active(true);
    let driver = RuntimeDriver::new(state, false);
    let mut task = RuntimeTask::new(command_rx, event_tx, driver);

    command_tx
        .send(Command::Publish {
            token_id: 1,
            publish: qos1_publish(0),
        })
        .unwrap();

    task.run_once().unwrap();

    assert_eq!(task.outbox.len(), 1);
    match &task.outbox[0] {
        Packet::Publish(p) => {
            assert_ne!(p.pkid, 0);
            assert!(!p.dup);
        }
        _ => panic!("expected publish in outbox"),
    }
}

#[test]
fn run_once_forwards_completion_event() {
    let (command_tx, command_rx) = channel();
    let (event_tx, event_rx) = channel();

    let mut state = RuntimeState::new(8);
    state.set_active(true);
    let driver = RuntimeDriver::new(state, false);
    let mut task = RuntimeTask::new(command_rx, event_tx, driver);

    command_tx
        .send(Command::Publish {
            token_id: 7,
            publish: qos1_publish(0),
        })
        .unwrap();
    task.run_once().unwrap();

    let pkid = match &task.outbox[0] {
        Packet::Publish(p) => p.pkid,
        _ => panic!("expected publish in outbox"),
    };

    let actions = task
        .driver
        .handle_event(mqtt_client::runtime::driver::DriverEvent::Incoming(
            Packet::PubAck(mqtt_client::types::PubAck { pkid }),
            std::time::Instant::now(),
        ))
        .unwrap();
    task.apply_actions_for_test(actions).unwrap();

    let event = event_rx.recv().unwrap();
    match event {
        RuntimeEvent::Completion(mqtt_client::runtime::state::Completion::PubAck {
            token_id,
            pkid: acked,
        }) => {
            assert_eq!(token_id, 7);
            assert_eq!(acked, pkid);
        }
        _ => panic!("expected completion event"),
    }
}

#[test]
fn closed_command_channel_returns_typed_error() {
    let (command_tx, command_rx) = channel::<Command>();
    let (event_tx, _event_rx) = channel();
    drop(command_tx);

    let driver = RuntimeDriver::new(RuntimeState::new(8), false);
    let mut task = RuntimeTask::new(command_rx, event_tx, driver);

    let err = task.run_once().unwrap_err();
    assert!(matches!(err, RuntimeTaskError::CommandChannelClosed));
}

#[test]
fn trigger_reconnect_maps_to_disconnected_event() {
    let (_command_tx, command_rx) = channel::<Command>();
    let (event_tx, event_rx) = channel();

    let mut state = RuntimeState::new_with_keep_alive(8, std::time::Duration::from_secs(5));
    state.set_active(true);
    state.await_pingresp = true;
    let base = std::time::Instant::now();
    state.last_outgoing = base;

    let driver = RuntimeDriver::new(state, false);
    let mut task = RuntimeTask::new(command_rx, event_tx, driver);

    let actions = task
        .driver
        .handle_event(mqtt_client::runtime::driver::DriverEvent::Tick(
            base + std::time::Duration::from_secs(5),
        ))
        .unwrap();
    task.apply_actions_for_test(actions).unwrap();

    let event = event_rx.recv().unwrap();
    assert_eq!(event, RuntimeEvent::Disconnected);
}
