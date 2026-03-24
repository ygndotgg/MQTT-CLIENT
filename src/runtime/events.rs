use crate::{runtime::state::Completion, types::Publish};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEvent {
    Completion(Completion), // outgoing operation finished
    IncomingPublish(Publish), // runtime task should emit --> state handling
    Disconnected, // task detected connection lost
    Reconnected, // task successfully restored
}
// future outer runtime task to notifiy the client
