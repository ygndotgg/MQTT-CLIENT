use std::sync::mpsc::{Receiver, RecvError, Sender};

use crate::{
    runtime::events::RuntimeEvent,
    types::{Command, Publish, Qos},
};

pub struct ClientHandle {
    pub command_tx: Sender<Command>,
    pub event_rx: Receiver<RuntimeEvent>,
    next_token_id: usize,
}

impl ClientHandle {
    pub fn new(command_tx: Sender<Command>, event_rx: Receiver<RuntimeEvent>) -> Self {
        Self {
            command_tx,
            event_rx,
            next_token_id: 1,
        }
    }
    fn next_token_id(&mut self) -> usize {
        let id = self.next_token_id;
        self.next_token_id += 1;
        id
    }
    pub fn publish(
        &mut self,
        topic: impl Into<String>,
        qos: Qos,
        retain: bool,
        payload: impl Into<Vec<u8>>,
    ) -> Result<usize, std::sync::mpsc::SendError<Command>> {
        let token_id = self.next_token_id();
        let publish = Publish {
            dup: false,
            qos,
            retain,
            topic: topic.into(),
            pkid: 0,
            payload: payload.into(),
        };
        self.command_tx
            .send(Command::Publish { token_id, publish })?;
        Ok(token_id)
    }

    pub fn recv_event(&self) -> Result<RuntimeEvent, RecvError> {
        self.event_rx.recv()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn publish_sends_command_and_returns_token() {
        let (command_tx, command_rx) = channel();
        let (_event_tx, event_rx) = channel();
        let mut client = ClientHandle::new(command_tx, event_rx);
        let token = client
            .publish("a/b", Qos::AtMostOnce, false, b"hello".to_vec())
            .unwrap();
        assert_eq!(token, 1);
        let cmd = command_rx.recv().unwrap();
        match cmd {
            Command::Publish { token_id, publish } => {
                assert_eq!(token_id, 1);
                assert_eq!(publish.topic, "a/b");
                assert_eq!(publish.qos, Qos::AtMostOnce);
                assert_eq!(publish.payload, b"hello".to_vec());
                assert_eq!(publish.pkid, 0);
                assert!(!publish.dup)
            }
            _ => panic!("expected publish command"),
        }
    }

    #[test]
    fn token_id_increments() {
        let (command_tx, _command_rx) = channel();
        let (_event_tx, event_rx) = channel();
        let mut client = ClientHandle::new(command_tx, event_rx);

        let first = client
            .publish("a", Qos::AtMostOnce, false, Vec::<u8>::new())
            .unwrap();
        let second = client
            .publish("b", Qos::AtMostOnce, false, Vec::<u8>::new())
            .unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn recv_event_returns_runtime_event() {
        let (command_tx, _command_rx) = channel();
        let (event_tx, event_rx) = channel();
        let client = ClientHandle::new(command_tx, event_rx);
        event_tx.send(RuntimeEvent::Disconnected).unwrap();
        let event = client.recv_event().unwrap();
        assert_eq!(event, RuntimeEvent::Disconnected)
    }
}
