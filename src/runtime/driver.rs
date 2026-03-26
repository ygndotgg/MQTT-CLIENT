use std::time::Instant;

use crate::{
    runtime::state::{Completion, IncomingQos2Result, RuntimeError, RuntimeState},
    types::{Command, Filter, Packet, Qos},
};

#[derive(Debug, Clone)]
pub enum DriverEvent {
    Tick(Instant),
    Command(Command),
    Incoming(Packet, Instant),
    ConnectionLost { clean_session: bool },
    ConnectionRestored(Instant),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverAction {
    Send(Packet),
    TriggerReconnect,
    CompleteFor {
        client_id: usize,
        completion: Completion,
    },
    SubscribeAckFor {
        client_id: usize,
        filters: Vec<Filter>,
        completion: Completion,
    },
    UnsubscribeAckFor {
        client_id: usize,
        filters: Vec<String>,
        completion: Completion,
    },
}

#[derive(Debug)]
pub struct RuntimeDriver {
    pub state: RuntimeState,
    pub clean_session: bool,
}

impl RuntimeDriver {
    pub fn new(state: RuntimeState, clean_session: bool) -> Self {
        Self {
            state,
            clean_session,
        }
    }
    pub fn handle_event(&mut self, event: DriverEvent) -> Result<Vec<DriverAction>, RuntimeError> {
        let mut out = Vec::new();
        match event {
            DriverEvent::Tick(now) => {
                match self.state.on_tick(now) {
                    Ok(Some(pkt)) => {
                        out.push(DriverAction::Send(pkt));
                        self.state.note_outgoing_activity(now);
                    }
                    Ok(None) => {}

                    Err(RuntimeError::AwaitPingRespTimeout) => {
                        self.state.on_connection_lost(self.clean_session);
                        out.push(DriverAction::TriggerReconnect);
                    }
                    Err(e) => return Err(e),
                };
            }
            DriverEvent::Command(c) => match c {
                Command::Publish {
                    token_id,
                    publish,
                    client_id,
                } => {
                    if let Some(pkt) = self.state.on_command_publish(Command::Publish {
                        token_id,
                        publish,
                        client_id,
                    })? {
                        self.state.note_outgoing_activity(Instant::now());
                        out.push(DriverAction::Send(pkt));
                    }
                }
                Command::Subscribe {
                    token_id,
                    subscribe,
                    client_id,
                } => {
                    if let Some(pkt) = self.state.on_command_subscribe(Command::Subscribe {
                        token_id,
                        subscribe,
                        client_id,
                    })? {
                        self.state.note_outgoing_activity(Instant::now());
                        out.push(DriverAction::Send(pkt));
                    }
                }
                Command::Unsubscribe {
                    token_id,
                    unsubscribe,
                    client_id,
                } => {
                    if let Some(pkt) = self.state.on_command_unsubscribe(Command::Unsubscribe {
                        token_id,
                        unsubscribe,
                        client_id,
                    })? {
                        self.state.note_outgoing_activity(Instant::now());
                        out.push(DriverAction::Send(pkt));
                    }
                }
                _ => {}
            },
            DriverEvent::Incoming(packet, now) => {
                self.state.note_incoming_activity(now);
                match packet {
                    Packet::PingResp => self.state.on_pingresp(now),
                    Packet::PubAck(p) => {
                        let ack = self.state.on_puback_checked(p.pkid)?;
                        out.push(DriverAction::CompleteFor {
                            client_id: ack.client_id,
                            completion: ack.completion,
                        });
                        if let Some(next) = ack.next_packet {
                            self.state.note_outgoing_activity(now);
                            out.push(DriverAction::Send(next));
                        }
                    }
                    Packet::PubRec(p) => {
                        let rel = self.state.on_pubrec_checked(p.pkid)?;
                        self.state.note_outgoing_activity(now);
                        out.push(DriverAction::Send(rel));
                    }
                    Packet::PubComp(p) => {
                        let ack = self.state.on_pubcomp_checked(p.pkid)?;
                        out.push(DriverAction::CompleteFor {
                            client_id: ack.client_id,
                            completion: ack.completion,
                        });
                        if let Some(next) = ack.next_packet {
                            self.state.note_outgoing_activity(now);
                            out.push(DriverAction::Send(next));
                        }
                    }
                    Packet::Publish(p) => {
                        if p.qos == Qos::ExactlyOnce {
                            let d = self.state.on_incoming_qos2_publish_checked(p.pkid)?;
                            match d {
                                IncomingQos2Result::FirstSeen { pubrec } => {
                                    self.state.note_outgoing_activity(now);
                                    out.push(DriverAction::Send(pubrec));
                                }
                                IncomingQos2Result::Duplicate { pubrec } => {
                                    self.state.note_outgoing_activity(now);
                                    out.push(DriverAction::Send(pubrec));
                                }
                            }
                        }
                    }
                    Packet::PubRel(p) => {
                        let comp = self.state.on_incoming_pubrel_checked(p.pkid)?;
                        self.state.note_outgoing_activity(now);
                        out.push(DriverAction::Send(comp));
                    }
                    Packet::SubAck(p) => {
                        let ack = self.state.on_suback_checked(p.pkid)?;
                        out.push(DriverAction::SubscribeAckFor {
                            client_id: ack.client_id,
                            filters: ack.filters,
                            completion: ack.completion,
                        });
                    }
                    Packet::UnsubAck(p) => {
                        let ack = self.state.on_unsuback_checked(p.pkid)?;
                        out.push(DriverAction::UnsubscribeAckFor {
                            client_id: ack.client_id,
                            filters: ack.filters,
                            completion: ack.completion,
                        });
                    }
                    _ => {}
                }
            }
            DriverEvent::ConnectionLost { clean_session } => {
                self.state.on_connection_lost(clean_session);
                out.push(DriverAction::TriggerReconnect);
            }
            DriverEvent::ConnectionRestored(now) => {
                self.state.set_active(true);
                self.state.await_pingresp = false;
                self.state.last_incoming = now;
                self.state.last_outgoing = now;
                for pkt in self.state.on_connection_restored() {
                    self.state.note_outgoing_activity(now);
                    out.push(DriverAction::Send(pkt));
                }
            }
        };
        Ok(out)
    }
}
