use mqtt_client::runtime::{
    inflight::{InflightStore, OutgoingOp},
    pkid::PacketIdPool,
    state::{IncomingQos2Result, RuntimeState},
};
use mqtt_client::types::{Command, Packet, Publish, Qos};

fn make_op(token_id: usize, packet: Packet) -> OutgoingOp {
    OutgoingOp::new(token_id, Command::Ping, packet)
}

#[test]
fn pkid_nonzero_wraparound() {
    let mut pool = PacketIdPool::new(3);
    assert_eq!(pool.next_candidate(), 1);
    assert_eq!(pool.next_candidate(), 2);
    assert_eq!(pool.next_candidate(), 3);
    assert_eq!(pool.next_candidate(), 1);
    assert_eq!(pool.next_candidate(), 2);
}

#[test]
fn inflight_insert_release() {
    let mut store = InflightStore::new(4);
    let op = make_op(1, Packet::PingReq);

    assert!(store.try_insert(1, op));
    assert_eq!(store.inflight, 1);
    assert!(store.outgoing[1].is_some());

    let released = store.release_ack(1);
    assert!(released.is_some());
    assert_eq!(store.inflight, 0);
    assert!(store.outgoing[1].is_none());
    assert_eq!(store.last_ack, 1);
}

#[test]
fn collision_when_slot_occupied() {
    let mut store = InflightStore::new(4);
    assert!(store.try_insert(1, make_op(1, Packet::PingReq)));
    assert!(!store.try_insert(1, make_op(2, Packet::PingResp)));

    assert!(store.collision.is_some());
    let (pkid, op) = store.collision.clone().unwrap();
    assert_eq!(pkid, 1);
    assert_eq!(op.token_id, 2);
}

#[test]
fn clean_moves_outgoing_to_pending() {
    let mut state = RuntimeState::new(4);
    assert!(state
        .inflight
        .try_insert(1, make_op(1, Packet::PingReq)));
    assert!(state
        .inflight
        .try_insert(2, make_op(2, Packet::PingResp)));

    state.clean_for_reconnect(false);

    assert_eq!(state.inflight.pending.len(), 2);
    assert_eq!(state.inflight.inflight, 0);
    assert!(state.inflight.outgoing[1].is_none());
    assert!(state.inflight.outgoing[2].is_none());
}

#[test]
fn qos2_pubrec_sets_rel_and_pubcomp_clears() {
    let mut state = RuntimeState::new(8);
    let publish = Packet::Publish(Publish {
        dup: false,
        qos: Qos::ExactlyOnce,
        retain: false,
        topic: "t/a".into(),
        pkid: 3,
        payload: b"hello".to_vec(),
    });
    assert!(state.inflight.try_insert(3, make_op(88, publish)));

    let next = state.on_pubrec_checked(3).unwrap();
    assert_eq!(next, Packet::PubRel(mqtt_client::types::PubRel { pkid: 3 }));
    assert!(state.inflight.outgoing_rel[3]);

    let done = state.on_pubcomp_checked(3).unwrap();
    assert_eq!(done.completion, mqtt_client::runtime::state::Completion::PubComp { token_id: 88, pkid: 3 });
    assert!(!state.inflight.outgoing_rel[3]);
    assert!(state.inflight.outgoing[3].is_none());
}

#[test]
fn incoming_qos2_publish_then_pubrel() {
    let mut state = RuntimeState::new(8);
    let first = state.on_incoming_qos2_publish_checked(5).unwrap();
    assert!(matches!(first, IncomingQos2Result::FirstSeen { .. }));
    assert!(state.inflight.incoming_pub[5]);

    let seen = state.on_incoming_pubrel_checked(5).unwrap();
    assert_eq!(seen, Packet::PubComp(mqtt_client::types::PubComp { pkid: 5 }));
    assert!(!state.inflight.incoming_pub[5]);

    // second PUBREL without corresponding incoming qos2 publish
    let seen_again = state.on_incoming_pubrel_checked(5);
    assert!(seen_again.is_err());
}
