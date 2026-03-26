use std::time::{Duration, Instant};

use mqtt_client::runtime::{
    inflight::OutgoingOp,
    state::{RuntimeError, RuntimeState},
};
use mqtt_client::types::{Command, Packet, Publish, Qos};

fn qos1_publish(pkid: u16) -> Publish {
    Publish {
        dup: false,
        qos: Qos::AtLeastOnce,
        retain: false,
        topic: "keepalive/test".into(),
        pkid,
        payload: b"payload".to_vec(),
    }
}

#[test]
fn new_with_keep_alive_uses_passed_duration() {
    let rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(12));
    assert_eq!(rt.keep_alive, Duration::from_secs(12));
}

#[test]
fn on_tick_returns_none_when_inactive() {
    let mut rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    rt.set_active(false);
    let out = rt.on_tick(Instant::now()).unwrap();
    assert!(out.is_none());
}

#[test]
fn on_tick_emits_pingreq_after_idle_keepalive() {
    let mut rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    rt.set_active(true);

    let base = Instant::now();
    rt.last_incoming = base;
    rt.last_outgoing = base;

    let out = rt.on_tick(base + Duration::from_secs(5)).unwrap();
    assert_eq!(out, Some(Packet::PingReq));
    assert!(rt.await_pingresp);
    assert_eq!(rt.last_outgoing, base + Duration::from_secs(5));
}

#[test]
fn on_tick_times_out_when_pingresp_missing() {
    let mut rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    rt.set_active(true);
    let base = Instant::now();
    rt.await_pingresp = true;
    rt.last_outgoing = base;

    let err = rt.on_tick(base + Duration::from_secs(5)).unwrap_err();
    assert_eq!(err, RuntimeError::AwaitPingRespTimeout);
}

#[test]
fn on_pingresp_clears_wait_and_updates_last_incoming() {
    let mut rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    let now = Instant::now();
    rt.await_pingresp = true;
    rt.on_pingresp(now);
    assert!(!rt.await_pingresp);
    assert_eq!(rt.last_incoming, now);
}

#[test]
fn on_connection_lost_moves_outgoing_and_collision_to_pending() {
    let mut rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    rt.set_active(true);

    let op1 = OutgoingOp::new(
        1,
        1,
        Command::Publish {
            client_id: 1,
            token_id: 1,
            publish: qos1_publish(1),
        },
        Packet::Publish(qos1_publish(1)),
    );
    assert!(rt.inflight.try_insert(1, op1));

    let op2 = OutgoingOp::new(
        2,
        2,
        Command::Publish {
            client_id: 2,
            token_id: 2,
            publish: qos1_publish(1),
        },
        Packet::Publish(qos1_publish(1)),
    );
    assert!(!rt.inflight.try_insert(1, op2));
    assert!(rt.inflight.collision.is_some());

    rt.on_connection_lost(false);

    assert!(!rt.active);
    assert_eq!(rt.inflight.pending.len(), 2);
    assert!(rt.inflight.outgoing[1].is_none());
    assert!(rt.inflight.collision.is_none());
    assert_eq!(rt.inflight.inflight, 0);
}

#[test]
fn on_connection_lost_clean_session_resets_trackers() {
    let mut rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    rt.inflight.last_ack = 7;
    rt.inflight.outgoing_rel[3] = true;
    rt.inflight.incoming_pub[4] = true;

    rt.on_connection_lost(true);

    assert_eq!(rt.inflight.last_ack, 0);
    assert!(!rt.inflight.outgoing_rel[3]);
    assert!(!rt.inflight.incoming_pub[4]);
}

#[test]
fn on_connection_restored_replays_pending_and_marks_dup_for_qos1() {
    let mut rt = RuntimeState::new_with_keep_alive(8, Duration::from_secs(5));
    rt.set_active(false);

    rt.on_command_publish(Command::Publish {
        client_id: 1,
        token_id: 77,
        publish: qos1_publish(0),
    })
    .unwrap();
    assert_eq!(rt.inflight.pending.len(), 1);

    let replay = rt.on_connection_restored();
    assert!(rt.active);
    assert_eq!(replay.len(), 1);

    match &replay[0] {
        Packet::Publish(p) => {
            assert!(p.dup);
            assert_ne!(p.pkid, 0);
        }
        _ => panic!("expected publish replay"),
    }
}
