use std::{
    collections::HashMap,
    io::Error,
    sync::mpsc::{Receiver, Sender},
    time::{Duration, Instant},
};

use crate::{
    client::ClientHandle,
    codec::{FrameParser, encode_packet},
    runtime::{
        driver::{DriverAction, RuntimeDriver},
        events::RuntimeEvent,
        state::{IncomingQos2Result, RuntimeError, RuntimeState},
        transport::Transport,
    },
    types::{Command, Packet, Publish},
};

pub struct ClientRegistry {
    next_id: usize,
    event_txs: HashMap<usize, Sender<RuntimeEvent>>,
}

impl ClientRegistry {
    pub fn register(&mut self, event_tx: Sender<RuntimeEvent>) -> usize {
        self.next_id += 1;
        let client_id = self.next_id;
        self.event_txs.insert(client_id, event_tx);
        client_id
    }

    pub fn remove(&mut self, client_id: usize) -> Option<Sender<RuntimeEvent>> {
        self.event_txs.remove(&client_id)
    }
    pub fn send_to(
        &self,
        client_id: usize,
        event: RuntimeEvent,
    ) -> Result<(), std::sync::mpsc::SendError<RuntimeEvent>> {
        match self.event_txs.get(&client_id) {
            Some(tx) => tx.send(event),
            None => Ok(()),
        }
    }
    pub fn broadcast(&self, event: RuntimeEvent) {
        for tx in self.event_txs.values() {
            let _ = tx.send(event.clone());
        }
    }
}

#[derive(Debug)]
pub enum RuntimeTaskError {
    Runtime(RuntimeError),
    Protocol(crate::protocol::v4::error::Error),
    Io(Error),
    CommandChannelClosed,
    EventChannelClosed,
}

pub struct RuntimeTask<T: Transport> {
    pub command_rx: Receiver<Command>,
    pub event_tx: Sender<RuntimeEvent>,
    pub driver: RuntimeDriver,
    // pub outbox: Vec<Packet>,
    pub transport: T,
    parser: FrameParser,
    pub read_buf: [u8; 4096],
    pub tick_interval: Duration,
    pub last_tick: Instant,
}

// pub trait Transport {
//     fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
//     fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()>;
// }

impl<T: Transport> RuntimeTask<T> {
    pub fn new(
        command_rx: Receiver<Command>,
        event_tx: Sender<RuntimeEvent>,
        driver: RuntimeDriver,
        transport: T,
        tick_interval: Duration,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            driver,
            transport,
            parser: FrameParser::default(),
            read_buf: [0; 4096],
            tick_interval,
            last_tick: Instant::now(),
        }
    }
    fn read_one_packet(&mut self) -> Result<Option<Packet>, RuntimeTaskError> {
        let n = self
            .transport
            .read(&mut self.read_buf)
            .map_err(RuntimeTaskError::Io)?;
        if n == 0 {
            return Ok(None);
        }
        self.parser.push(&self.read_buf[..n]);
        self.parser
            .next_packet()
            .map_err(RuntimeTaskError::Protocol)
    }
    pub fn recv_cmd(&mut self) -> Result<Command, RuntimeTaskError> {
        match self.command_rx.recv() {
            Ok(c) => Ok(c),
            Err(_e) => Err(RuntimeTaskError::CommandChannelClosed),
        }
    }

    pub fn run_once(&mut self) -> Result<(), RuntimeTaskError> {
        let cmd = self.recv_cmd()?;
        let actions = self
            .driver
            .handle_event(crate::runtime::driver::DriverEvent::Command(cmd))
            .map_err(RuntimeTaskError::Runtime)?;
        self.handle_actions(actions)
    }

    fn handle_incoming_publish(
        &mut self,
        publish: Publish,
        now: Instant,
    ) -> Result<(), RuntimeTaskError> {
        self.driver.state.note_incoming_activity(now);
        match publish.qos {
            crate::types::Qos::AtMostOnce => {
                self.event_tx
                    .send(RuntimeEvent::IncomingPublish(publish))
                    .map_err(|_| RuntimeTaskError::EventChannelClosed)?;
                Ok(())
            }
            crate::types::Qos::AtLeastOnce => {
                self.event_tx
                    .send(RuntimeEvent::IncomingPublish(publish.clone()))
                    .map_err(|_| RuntimeTaskError::EventChannelClosed)?;
                let puback = Packet::PubAck(crate::types::PubAck { pkid: publish.pkid });
                self.send_packet(puback)?;
                Ok(())
            }
            crate::types::Qos::ExactlyOnce => {
                match self
                    .driver
                    .state
                    .on_incoming_qos2_publish_checked(publish.pkid)
                    .map_err(RuntimeTaskError::Runtime)?
                {
                    IncomingQos2Result::FirstSeen { pubrec } => {
                        self.send_packet(pubrec)?;
                        self.event_tx
                            .send(RuntimeEvent::IncomingPublish(publish))
                            .map_err(|_| RuntimeTaskError::EventChannelClosed)?;
                    }
                    IncomingQos2Result::Duplicate { pubrec } => {
                        self.send_packet(pubrec)?;
                    }
                }
                Ok(())
            }
        }
    }

    fn reconnect(&mut self) -> Result<(), RuntimeTaskError> {
        let now = Instant::now();
        let actions = self
            .driver
            .handle_event(super::driver::DriverEvent::ConnectionRestored(now))
            .map_err(RuntimeTaskError::Runtime)?;
        self.handle_actions(actions)?;
        self.event_tx
            .send(RuntimeEvent::Reconnected)
            .map_err(|_| RuntimeTaskError::EventChannelClosed)
    }

    fn poll_incoming(&mut self) -> Result<(), RuntimeTaskError> {
        let Some(packet) = self.read_one_packet()? else {
            return Ok(());
        };
        let now = Instant::now();
        match packet {
            Packet::Publish(publish) => self.handle_incoming_publish(publish, now),
            other => {
                let actions = self
                    .driver
                    .handle_event(super::driver::DriverEvent::Incoming(other, now))
                    .map_err(RuntimeTaskError::Runtime)?;
                self.handle_actions(actions)
            }
        }
    }

    fn poll_command(&mut self) -> Result<(), RuntimeTaskError> {
        match self.command_rx.try_recv() {
            Ok(cmd) => {
                let actions = self
                    .driver
                    .handle_event(super::driver::DriverEvent::Command(cmd))
                    .map_err(RuntimeTaskError::Runtime)?;
                self.handle_actions(actions)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(()),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(RuntimeTaskError::CommandChannelClosed)
            }
        }
    }

    fn poll_tick(&mut self) -> Result<(), RuntimeTaskError> {
        let now = Instant::now();
        if now.duration_since(self.last_tick) < self.tick_interval {
            return Ok(());
        }
        self.last_tick = now;
        let actions = self
            .driver
            .handle_event(super::driver::DriverEvent::Tick(now))
            .map_err(RuntimeTaskError::Runtime)?;
        self.handle_actions(actions)
    }

    fn handle_actions(&mut self, actions: Vec<DriverAction>) -> Result<(), RuntimeTaskError> {
        for action in actions {
            match action {
                DriverAction::Send(packet) => {
                    self.send_packet(packet)?;
                }
                DriverAction::Complete(c) => {
                    if let Err(_e) = self.event_tx.send(RuntimeEvent::Completion(c)) {
                        return Err(RuntimeTaskError::EventChannelClosed);
                    }
                }
                DriverAction::TriggerReconnect => {
                    if let Err(_e) = self.event_tx.send(RuntimeEvent::Disconnected) {
                        return Err(RuntimeTaskError::EventChannelClosed);
                    }
                    self.reconnect()?;
                }
            }
        }
        Ok(())
    }

    pub fn apply_actions_for_test(
        &mut self,
        actions: Vec<DriverAction>,
    ) -> Result<(), RuntimeTaskError> {
        self.handle_actions(actions)
    }

    pub fn run(mut self) -> Result<(), RuntimeTaskError> {
        loop {
            self.poll_command()?;
            self.poll_incoming()?;
            self.poll_tick()?;
        }
    }
    fn send_packet(&mut self, packet: Packet) -> Result<(), RuntimeTaskError> {
        let mut bytes = Vec::new();
        encode_packet(&packet, &mut bytes).map_err(|e| RuntimeTaskError::Protocol(e))?;

        self.transport
            .write_all(&bytes)
            .map_err(RuntimeTaskError::Io)?;

        self.driver.state.note_outgoing_activity(Instant::now());
        Ok(())
    }
    // fn reconnect(&mut self) -> Result<(), RuntimeTaskError> {
    //     // TODO: close/recreate transport and add backoff/retry.
    //     let now = Instant::now();
    //     let actions = self
    //         .driver
    //         .handle_event(super::driver::DriverEvent::ConnectionRestored(now))
    //         .map_err(RuntimeTaskError::Runtime)?;
    //     self.handle_actions(actions)?;
    //     self.event_tx
    //         .send(RuntimeEvent::Reconnected)
    //         .map_err(|_| RuntimeTaskError::EventChannelClosed)?;
    //     Ok(())
    // }
}

pub fn start_runtime<T: Transport + Send + 'static>(
    transport: T,
    max_inflight: u16,
    clean_session: bool,
) -> ClientHandle {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let state = RuntimeState::new(max_inflight);
    let driver = RuntimeDriver::new(state, clean_session);
    let task = RuntimeTask::new(
        command_rx,
        event_tx,
        driver,
        transport,
        Duration::from_millis(100),
    );
    std::thread::spawn(move || {
        let _ = task.run();
    });
    ClientHandle::new(command_tx, event_rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        codec::{decode_packet, encode_packet},
        runtime::events::RuntimeEvent,
        runtime::state::Completion,
        types::{Command, Packet, PubAck, Publish, Qos},
    };
    use std::{
        sync::mpsc::channel,
        time::{Duration, Instant},
    };

    #[derive(Default)]
    struct FakeTransport {
        reads: Vec<Vec<u8>>,
        writes: Vec<Vec<u8>>,
    }

    impl Transport for FakeTransport {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.reads.is_empty() {
                return Ok(0);
            }

            let next = self.reads.remove(0);
            let n = next.len().min(buf.len());
            buf[..n].copy_from_slice(&next[..n]);
            Ok(n)
        }

        fn write_all(&mut self, buf: &[u8]) -> std::io::Result<()> {
            self.writes.push(buf.to_vec());
            Ok(())
        }
    }

    fn build_task() -> (
        RuntimeTask<FakeTransport>,
        std::sync::mpsc::Sender<Command>,
        std::sync::mpsc::Receiver<RuntimeEvent>,
    ) {
        let (command_tx, command_rx) = channel();
        let (event_tx, event_rx) = channel();
        let mut state = RuntimeState::new(4);
        state.set_active(true);
        let driver = RuntimeDriver::new(state, false);
        let transport = FakeTransport::default();
        let task = RuntimeTask::new(
            command_rx,
            event_tx,
            driver,
            transport,
            Duration::from_millis(1),
        );
        (task, command_tx, event_rx)
    }

    #[test]
    fn command_publish_writes_encoded_publish() {
        let (mut task, command_tx, _event_rx) = build_task();
        let publish = Publish {
            dup: false,
            qos: Qos::AtLeastOnce,
            retain: false,
            topic: "a/b".into(),
            pkid: 0,
            payload: b"hello".to_vec(),
        };

        command_tx
            .send(Command::Publish {
                token_id: 1,
                publish: publish.clone(),
            })
            .unwrap();

        task.poll_command().unwrap();
        assert_eq!(task.transport.writes.len(), 1);

        let packet = decode_packet(&task.transport.writes[0]).unwrap();
        if let Packet::Publish(decoded) = packet {
            assert_eq!(decoded.topic, publish.topic);
            assert_eq!(decoded.payload, publish.payload);
            assert!(decoded.pkid != 0);
        } else {
            panic!("expected publish");
        }
    }

    #[test]
    fn incoming_puback_emits_completion() {
        let (mut task, command_tx, event_rx) = build_task();
        let publish = Publish {
            dup: false,
            qos: Qos::AtLeastOnce,
            retain: false,
            topic: "x".into(),
            pkid: 0,
            payload: b"y".to_vec(),
        };

        command_tx
            .send(Command::Publish {
                token_id: 7,
                publish,
            })
            .unwrap();
        task.poll_command().unwrap();

        let encoded_publish = task.transport.writes.pop().unwrap();
        let pkid = match decode_packet(&encoded_publish).unwrap() {
            Packet::Publish(p) => p.pkid,
            _ => panic!("expected publish"),
        };

        let mut ack_bytes = Vec::new();
        encode_packet(&Packet::PubAck(PubAck { pkid }), &mut ack_bytes).unwrap();
        task.transport.reads.push(ack_bytes);

        task.poll_incoming().unwrap();
        let event = event_rx.recv().unwrap();
        assert_eq!(
            event,
            RuntimeEvent::Completion(Completion::PubAck { token_id: 7, pkid })
        );
    }

    #[test]
    fn poll_tick_writes_pingreq_after_idle() {
        let (mut task, _, _event_rx) = build_task();
        task.driver.state.set_active(true);
        task.driver.state.last_incoming = Instant::now() - Duration::from_secs(60);
        task.driver.state.last_outgoing = Instant::now() - Duration::from_secs(60);
        task.last_tick = Instant::now() - Duration::from_secs(60);

        task.poll_tick().unwrap();
        assert_eq!(task.transport.writes.len(), 1);

        let packet = decode_packet(&task.transport.writes[0]).unwrap();
        assert_eq!(packet, Packet::PingReq);
    }
}
