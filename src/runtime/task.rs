use std::{
    io::Error,
    sync::mpsc::{Receiver, Sender},
    time::{Duration, Instant},
};

use crate::{
    client::ClientHandle,
    codec::{self, encode_packet},
    runtime::{
        driver::{DriverAction, RuntimeDriver},
        events::RuntimeEvent,
        state::{RuntimeError, RuntimeState},
        transport::Transport,
    },
    types::{Command, Packet},
};

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
    pub read_buf: [u8; 4096],
    pub pendeing_read: Vec<u8>,
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
            read_buf: [0; 4096],
            pendeing_read: Vec::new(),
            tick_interval,
            last_tick: Instant::now(),
        }
    }
    fn read_one_packet(&mut self)->Result<Option<Packet>,RuntimeTaskError> {
        
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
    fn handle_actions(&mut self, actions: Vec<DriverAction>) -> Result<(), RuntimeTaskError> {
        for action in actions {
            match action {
                DriverAction::Send(packet) => {
                    unimplemented!()
                    // self.outbox.push(packet);
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
            self.run_once()?;
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
