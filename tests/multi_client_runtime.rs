use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use mqtt_client::{
    client::ClientHandle,
    runtime::{
        events::RuntimeEvent,
        driver::RuntimeDriver,
        state::Completion,
        state::RuntimeState,
        task::{ClientRegistry, RuntimeTask},
        transport::Transport,
    },
    types::{Packet, PubAck, Publish, Qos, SubAck, UnsubAck},
};

#[derive(Clone, Default)]
struct SharedFakeTransport {
    inner: Arc<Mutex<TransportState>>,
}

#[derive(Default)]
struct TransportState {
    reads: VecDeque<Vec<u8>>,
    writes: Vec<Vec<u8>>,
}

impl SharedFakeTransport {
    fn push_read_packet(&self, packet: &Packet) {
        let mut bytes = Vec::new();
        mqtt_client::codec::encode_packet(packet, &mut bytes).unwrap();
        self.inner.lock().unwrap().reads.push_back(bytes);
    }

    fn writes_snapshot(&self) -> Vec<Vec<u8>> {
        self.inner.lock().unwrap().writes.clone()
    }
}

impl Transport for SharedFakeTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut inner = self.inner.lock().unwrap();
        let Some(next) = inner.reads.pop_front() else {
            return Ok(0);
        };

        let n = next.len().min(buf.len());
        buf[..n].copy_from_slice(&next[..n]);
        Ok(n)
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.inner.lock().unwrap().writes.push(buf.to_vec());
        Ok(())
    }
}

fn wait_until<F>(timeout: Duration, mut condition: F)
where
    F: FnMut() -> bool,
{
    let start = Instant::now();
    while !condition() {
        if start.elapsed() > timeout {
            panic!("condition not met within {:?}", timeout);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn decode_writes(transport: &SharedFakeTransport) -> Vec<Packet> {
    transport
        .writes_snapshot()
        .into_iter()
        .map(|bytes| mqtt_client::codec::decode_packet(&bytes).unwrap())
        .collect()
}

fn spawn_test_runtime(
    transport: SharedFakeTransport,
) -> (ClientHandle, ClientHandle) {
    spawn_test_runtime_with_timing(
        transport,
        Duration::from_secs(30),
        Duration::from_millis(10),
    )
}

fn spawn_test_runtime_with_timing(
    transport: SharedFakeTransport,
    keep_alive: Duration,
    tick_interval: Duration,
) -> (ClientHandle, ClientHandle) {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let registry = Arc::new(Mutex::new(ClientRegistry::default()));

    let (event_tx_a, event_rx_a) = std::sync::mpsc::channel();
    let (event_tx_b, event_rx_b) = std::sync::mpsc::channel();

    let client_id_a = registry.lock().unwrap().register(event_tx_a);
    let client_id_b = registry.lock().unwrap().register(event_tx_b);

    let mut state = RuntimeState::new_with_keep_alive(16, keep_alive);
    state.set_active(true);
    let driver = RuntimeDriver::new(state, false);
    let task = RuntimeTask::new(
        command_rx,
        Arc::clone(&registry),
        driver,
        transport,
        tick_interval,
    );

    thread::spawn(move || {
        let _ = task.run();
    });

    (
        ClientHandle::new(client_id_a, command_tx.clone(), event_rx_a),
        ClientHandle::new(client_id_b, command_tx, event_rx_b),
    )
}

#[test]
fn multi_client_completions_and_publish_fanout_are_routed_correctly() {
    let transport = SharedFakeTransport::default();
    let (mut client_a, mut client_b) = spawn_test_runtime(transport.clone());

    let sub_token_a = client_a.subscribe("sensor/+", Qos::AtMostOnce).unwrap();
    let sub_token_b = client_b.subscribe("alerts/#", Qos::AtMostOnce).unwrap();

    wait_until(Duration::from_secs(1), || transport.writes_snapshot().len() >= 2);

    let writes = decode_writes(&transport);
    let mut sub_pkids = Vec::new();
    for packet in writes {
        if let Packet::Subscribe(subscribe) = packet {
            sub_pkids.push(subscribe.pkid);
        }
    }
    assert_eq!(sub_pkids.len(), 2);

    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: sub_pkids[0],
        return_codes: vec![0x00],
    }));
    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: sub_pkids[1],
        return_codes: vec![0x00],
    }));

    let event_a = client_a.recv_event().unwrap();
    let event_b = client_b.recv_event().unwrap();

    assert_eq!(
        event_a,
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_a,
            pkid: sub_pkids[0]
        })
    );
    assert_eq!(
        event_b,
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_b,
            pkid: sub_pkids[1]
        })
    );

    transport.push_read_packet(&Packet::Publish(Publish {
        dup: false,
        qos: Qos::AtMostOnce,
        retain: false,
        topic: "sensor/temp".into(),
        pkid: 0,
        payload: b"t1".to_vec(),
    }));

    assert_eq!(
        client_a.recv_event().unwrap(),
        RuntimeEvent::IncomingPublish(Publish {
            dup: false,
            qos: Qos::AtMostOnce,
            retain: false,
            topic: "sensor/temp".into(),
            pkid: 0,
            payload: b"t1".to_vec(),
        })
    );
    assert!(client_b.event_rx.recv_timeout(Duration::from_millis(200)).is_err());

    let publish_token = client_a
        .publish("cmd/out", Qos::AtLeastOnce, false, b"hello".to_vec())
        .unwrap();

    wait_until(Duration::from_secs(1), || {
        decode_writes(&transport)
            .iter()
            .any(|packet| matches!(packet, Packet::Publish(p) if p.topic == "cmd/out"))
    });

    let publish_pkid = decode_writes(&transport)
        .into_iter()
        .find_map(|packet| match packet {
            Packet::Publish(p) if p.topic == "cmd/out" => Some(p.pkid),
            _ => None,
        })
        .unwrap();

    transport.push_read_packet(&Packet::PubAck(PubAck { pkid: publish_pkid }));

    assert_eq!(
        client_a.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::PubAck {
            token_id: publish_token,
            pkid: publish_pkid,
        })
    );
    assert!(client_b.event_rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn unsubscribe_removes_route_for_only_that_client() {
    let transport = SharedFakeTransport::default();
    let (mut client_a, mut client_b) = spawn_test_runtime(transport.clone());

    let sub_token_a = client_a.subscribe("sensor/+", Qos::AtMostOnce).unwrap();
    let sub_token_b = client_b.subscribe("sensor/+", Qos::AtMostOnce).unwrap();

    wait_until(Duration::from_secs(1), || transport.writes_snapshot().len() >= 2);

    let subscribe_pkids: Vec<u16> = decode_writes(&transport)
        .into_iter()
        .filter_map(|packet| match packet {
            Packet::Subscribe(subscribe) => Some(subscribe.pkid),
            _ => None,
        })
        .collect();
    assert_eq!(subscribe_pkids.len(), 2);

    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: subscribe_pkids[0],
        return_codes: vec![0x00],
    }));
    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: subscribe_pkids[1],
        return_codes: vec![0x00],
    }));

    assert_eq!(
        client_a.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_a,
            pkid: subscribe_pkids[0],
        })
    );
    assert_eq!(
        client_b.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_b,
            pkid: subscribe_pkids[1],
        })
    );

    let unsub_token_a = client_a.unsubscribe("sensor/+").unwrap();

    wait_until(Duration::from_secs(1), || {
        decode_writes(&transport)
            .iter()
            .any(|packet| matches!(packet, Packet::UnSubscribe(_)))
    });

    let unsub_pkid = decode_writes(&transport)
        .into_iter()
        .find_map(|packet| match packet {
            Packet::UnSubscribe(unsub) => Some(unsub.pkid),
            _ => None,
        })
        .unwrap();

    transport.push_read_packet(&Packet::UnsubAck(UnsubAck { pkid: unsub_pkid }));

    assert_eq!(
        client_a.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::UnsubAck {
            token_id: unsub_token_a,
            pkid: unsub_pkid,
        })
    );
    assert!(client_b.event_rx.recv_timeout(Duration::from_millis(100)).is_err());

    transport.push_read_packet(&Packet::Publish(Publish {
        dup: false,
        qos: Qos::AtMostOnce,
        retain: false,
        topic: "sensor/temp".into(),
        pkid: 0,
        payload: b"t2".to_vec(),
    }));

    assert_eq!(
        client_b.recv_event().unwrap(),
        RuntimeEvent::IncomingPublish(Publish {
            dup: false,
            qos: Qos::AtMostOnce,
            retain: false,
            topic: "sensor/temp".into(),
            pkid: 0,
            payload: b"t2".to_vec(),
        })
    );
    assert!(client_a.event_rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn reconnect_broadcasts_and_preserves_fanout_after_replay() {
    let transport = SharedFakeTransport::default();
    let (mut client_a, mut client_b) = spawn_test_runtime_with_timing(
        transport.clone(),
        Duration::from_millis(250),
        Duration::from_millis(10),
    );

    let sub_token_a = client_a.subscribe("sensor/+", Qos::AtMostOnce).unwrap();
    let sub_token_b = client_b.subscribe("alerts/#", Qos::AtMostOnce).unwrap();

    wait_until(Duration::from_secs(1), || transport.writes_snapshot().len() >= 2);

    let subscribe_pkids: Vec<u16> = decode_writes(&transport)
        .into_iter()
        .filter_map(|packet| match packet {
            Packet::Subscribe(subscribe) => Some(subscribe.pkid),
            _ => None,
        })
        .collect();
    assert_eq!(subscribe_pkids.len(), 2);

    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: subscribe_pkids[0],
        return_codes: vec![0x00],
    }));
    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: subscribe_pkids[1],
        return_codes: vec![0x00],
    }));

    assert_eq!(
        client_a.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_a,
            pkid: subscribe_pkids[0],
        })
    );
    assert_eq!(
        client_b.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_b,
            pkid: subscribe_pkids[1],
        })
    );

    let publish_token = client_a
        .publish("cmd/out", Qos::AtLeastOnce, false, b"hello".to_vec())
        .unwrap();

    wait_until(Duration::from_secs(1), || {
        decode_writes(&transport)
            .iter()
            .filter(|packet| matches!(packet, Packet::Publish(p) if p.topic == "cmd/out"))
            .count()
            >= 1
    });

    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Disconnected
    );
    assert_eq!(
        client_b
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Disconnected
    );
    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Reconnected
    );
    assert_eq!(
        client_b
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Reconnected
    );

    let cmd_out_publishes: Vec<Publish> = decode_writes(&transport)
        .into_iter()
        .filter_map(|packet| match packet {
            Packet::Publish(p) if p.topic == "cmd/out" => Some(p),
            _ => None,
        })
        .collect();
    assert!(cmd_out_publishes.len() >= 2);
    assert!(cmd_out_publishes.iter().any(|p| !p.dup));
    assert!(cmd_out_publishes.iter().any(|p| p.dup));

    let replay_pkid = cmd_out_publishes.iter().find(|p| p.dup).unwrap().pkid;
    transport.push_read_packet(&Packet::PubAck(PubAck { pkid: replay_pkid }));

    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        RuntimeEvent::Completion(Completion::PubAck {
            token_id: publish_token,
            pkid: replay_pkid,
        })
    );
    assert!(client_b.event_rx.recv_timeout(Duration::from_millis(200)).is_err());

    transport.push_read_packet(&Packet::Publish(Publish {
        dup: false,
        qos: Qos::AtMostOnce,
        retain: false,
        topic: "sensor/temp".into(),
        pkid: 0,
        payload: b"after-reconnect".to_vec(),
    }));

    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        RuntimeEvent::IncomingPublish(Publish {
            dup: false,
            qos: Qos::AtMostOnce,
            retain: false,
            topic: "sensor/temp".into(),
            pkid: 0,
            payload: b"after-reconnect".to_vec(),
        })
    );
    assert!(client_b.event_rx.recv_timeout(Duration::from_millis(200)).is_err());
}

#[test]
fn reconnect_replays_unsubscribe_and_removes_route_after_unsuback() {
    let transport = SharedFakeTransport::default();
    let (mut client_a, mut client_b) = spawn_test_runtime_with_timing(
        transport.clone(),
        Duration::from_millis(250),
        Duration::from_millis(10),
    );

    let sub_token_a = client_a.subscribe("sensor/+", Qos::AtMostOnce).unwrap();
    let sub_token_b = client_b.subscribe("sensor/+", Qos::AtMostOnce).unwrap();

    wait_until(Duration::from_secs(1), || transport.writes_snapshot().len() >= 2);

    let subscribe_pkids: Vec<u16> = decode_writes(&transport)
        .into_iter()
        .filter_map(|packet| match packet {
            Packet::Subscribe(subscribe) => Some(subscribe.pkid),
            _ => None,
        })
        .collect();
    assert_eq!(subscribe_pkids.len(), 2);

    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: subscribe_pkids[0],
        return_codes: vec![0x00],
    }));
    transport.push_read_packet(&Packet::SubAck(SubAck {
        pkid: subscribe_pkids[1],
        return_codes: vec![0x00],
    }));

    assert_eq!(
        client_a.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_a,
            pkid: subscribe_pkids[0],
        })
    );
    assert_eq!(
        client_b.recv_event().unwrap(),
        RuntimeEvent::Completion(Completion::SubAck {
            token_id: sub_token_b,
            pkid: subscribe_pkids[1],
        })
    );

    let unsub_token_a = client_a.unsubscribe("sensor/+").unwrap();

    wait_until(Duration::from_secs(1), || {
        decode_writes(&transport)
            .iter()
            .filter(|packet| matches!(packet, Packet::UnSubscribe(unsub) if unsub.filters == vec!["sensor/+".to_string()]))
            .count()
            >= 1
    });

    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Disconnected
    );
    assert_eq!(
        client_b
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Disconnected
    );
    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Reconnected
    );
    assert_eq!(
        client_b
            .event_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap(),
        RuntimeEvent::Reconnected
    );

    let replayed_unsubscribes: Vec<_> = decode_writes(&transport)
        .into_iter()
        .filter_map(|packet| match packet {
            Packet::UnSubscribe(unsub) if unsub.filters == vec!["sensor/+".to_string()] => {
                Some(unsub)
            }
            _ => None,
        })
        .collect();
    assert!(replayed_unsubscribes.len() >= 2);

    transport.push_read_packet(&Packet::Publish(Publish {
        dup: false,
        qos: Qos::AtMostOnce,
        retain: false,
        topic: "sensor/temp".into(),
        pkid: 0,
        payload: b"before-unsuback".to_vec(),
    }));

    let expected_publish = RuntimeEvent::IncomingPublish(Publish {
        dup: false,
        qos: Qos::AtMostOnce,
        retain: false,
        topic: "sensor/temp".into(),
        pkid: 0,
        payload: b"before-unsuback".to_vec(),
    });
    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        expected_publish.clone()
    );
    assert_eq!(
        client_b
            .event_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        expected_publish
    );

    let replay_unsub_pkid = replayed_unsubscribes.last().unwrap().pkid;
    transport.push_read_packet(&Packet::UnsubAck(UnsubAck {
        pkid: replay_unsub_pkid,
    }));

    assert_eq!(
        client_a
            .event_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        RuntimeEvent::Completion(Completion::UnsubAck {
            token_id: unsub_token_a,
            pkid: replay_unsub_pkid,
        })
    );
    assert!(client_b.event_rx.recv_timeout(Duration::from_millis(200)).is_err());

    transport.push_read_packet(&Packet::Publish(Publish {
        dup: false,
        qos: Qos::AtMostOnce,
        retain: false,
        topic: "sensor/temp".into(),
        pkid: 0,
        payload: b"after-unsuback".to_vec(),
    }));

    assert_eq!(
        client_b
            .event_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap(),
        RuntimeEvent::IncomingPublish(Publish {
            dup: false,
            qos: Qos::AtMostOnce,
            retain: false,
            topic: "sensor/temp".into(),
            pkid: 0,
            payload: b"after-unsuback".to_vec(),
        })
    );
    assert!(client_a.event_rx.recv_timeout(Duration::from_millis(200)).is_err());
}
