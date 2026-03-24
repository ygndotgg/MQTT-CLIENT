use mqtt_client::{runtime::events::RuntimeEvent, types::Publish};

#[test]
fn runtime_event_variants_are_constructible() {
    let publish = Publish {
        dup: false,
        qos: mqtt_client::types::Qos::AtMostOnce,
        retain: false,
        topic: "t".into(),
        pkid: 0,
        payload: b"x".to_vec(),
    };
    let a = RuntimeEvent::Completion(mqtt_client::runtime::state::Completion::PubAck {
        token_id: 1,
        pkid: 2,
    });
    let b = RuntimeEvent::IncomingPublish(publish);
    let c = RuntimeEvent::Disconnected;
    let d = RuntimeEvent::Reconnected;

    match a {
        RuntimeEvent::Completion(_) => {}
        _ => {
            panic!("expected completion")
        }
    }
    assert!(matches!(b, RuntimeEvent::IncomingPublish(_)));
    assert_eq!(c, RuntimeEvent::Disconnected);
    assert_eq!(d, RuntimeEvent::Reconnected);
}
