use std::sync::mpsc::{Receiver, RecvError, Sender};

use crate::{
    client::ClientHandle,
    runtime::{
        driver::{DriverAction, RuntimeDriver},
        events::RuntimeEvent,
        state::{RuntimeError, RuntimeState},
    },
    types::{Command, Packet},
};

pub enum RuntimeTaskError {
    Runtime(RuntimeError),
    CommandChannelClosed,
    EventChannelClosed,
}

pub struct RuntimeTask {
    pub command_rx: Receiver<Command>,
    pub event_tx: Sender<RuntimeEvent>,
    pub driver: RuntimeDriver,
    pub outbox: Vec<Packet>,
}

impl RuntimeTask {
    pub fn new(
        command_rx: Receiver<Command>,
        event_tx: Sender<RuntimeEvent>,
        driver: RuntimeDriver,
    ) -> Self {
        Self {
            command_rx,
            event_tx,
            driver,
            outbox: Vec::new(),
        }
    }
    pub fn recv_cmd(&mut self) -> Result<Command, RuntimeTaskError> {
        match self.command_rx.recv() {
            Ok(c) => Ok(c),
            Err(_e) => Err(RuntimeTaskError::CommandChannelClosed),
        }
    }

    pub fn run_once(&mut self) -> Result<(), RuntimeTaskError> {
        let cmd = self.recv_cmd();
        match cmd {
            Ok(cmd) => {
                let actions = self
                    .driver
                    .handle_event(crate::runtime::driver::DriverEvent::Command(cmd))
                    .map_err(RuntimeTaskError::Runtime)?;
                self.handle_actions(actions)
            }
            Err(_e) => panic!("Unable to receive the command"),
        }
    }
    fn handle_actions(&mut self, actions: Vec<DriverAction>) -> Result<(), RuntimeTaskError> {
        for action in actions {
            match action {
                DriverAction::Send(packet) => {
                    self.outbox.push(packet);
                }
                DriverAction::Complete(c) => {
                    if let Err(_e) = self.event_tx.send(RuntimeEvent::Completion(c)) {
                        return Err(RuntimeTaskError::EventChannelClosed);
                    }
                }
                DriverAction::TriggerReconnect => {
                    if let Err(_e) = self.event_tx.send(RuntimeEvent::Reconnected) {
                        return Err(RuntimeTaskError::EventChannelClosed);
                    }
                }
            }
        }
        Ok(())
    }
    pub fn run(mut self) -> Result<(), RuntimeTaskError> {
        loop {
            self.run_once()?;
        }
    }
}

pub fn start_runtime(max_inflight: u16, clean_session: bool) -> ClientHandle {
    let (command_tx, command_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let state = RuntimeState::new(max_inflight);
    let driver = RuntimeDriver::new(state, clean_session);
    let task = RuntimeTask::new(command_rx, event_tx, driver);
    std::thread::spawn(move || {
        let _ = task.run();
    });
    ClientHandle::new(command_tx, event_rx)
}
