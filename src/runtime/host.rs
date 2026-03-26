use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender, channel},
    },
    time::Duration,
};

use crate::{
    client::ClientHandle,
    runtime::{
        driver::RuntimeDriver,
        state::RuntimeState,
        task::{ClientRegistry, RuntimeTask},
        transport::Transport,
    },
    types::Command,
};

pub struct RuntimeHost {
    command_tx: Sender<Command>,
    registry: Arc<Mutex<ClientRegistry>>,
}

impl RuntimeHost {
    pub fn new<T: Transport + Send + 'static>(
        transport: T,
        max_inflight: u16,
        clean_session: bool,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let registry = Arc::new(Mutex::new(ClientRegistry::default()));
        let state = RuntimeState::new(max_inflight);
        let driver = RuntimeDriver::new(state, clean_session);
        let task = RuntimeTask::new(
            command_rx,
            Arc::clone(&registry),
            driver,
            transport,
            Duration::from_millis(100),
        );
        std::thread::spawn(move || {
            let _ = task.run();
        });
        Self {
            command_tx,
            registry,
        }
    }
    pub fn new_client(&self) -> ClientHandle {
        let (event_tx, event_rx) = channel();
        let client_id = self.registry.lock().unwrap().register(event_tx);
        ClientHandle::new(client_id, self.command_tx.clone(), event_rx)
    }
}
