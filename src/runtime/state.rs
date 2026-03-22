use crate::{
    runtime::{
        inflight::{InflightStore, OutgoingOp},
        pkid::PacketIdPool,
    },
    types::{Command, Packet, Qos},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    PubAck { token_id: usize, pkid: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AckResult {
    pub completion: Completion,
    pub next_packet: Option<Packet>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    InvalidCommand,
    UnsolicitedAck(u16),
}

#[derive(Debug)]
pub struct RuntimeState {
    pub pkid_pool: PacketIdPool,
    pub inflight: InflightStore,
    pub active: bool,
}

impl RuntimeState {
    pub fn new(max_inflight: u16) -> Self {
        Self {
            pkid_pool: PacketIdPool::new(max_inflight),
            inflight: InflightStore::new(max_inflight),
            active: false,
        }
    }
    pub fn on_puback(&mut self, pkid: u16) -> Option<OutgoingOp> {
        self.inflight.release_ack(pkid)
    }
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    pub fn on_pubrec(&mut self, pkid: u16) -> Option<Packet> {
        self.inflight.outgoing_rel[pkid as usize] = true;
        Some(Packet::PubRel(crate::types::PubRel { pkid }))
    }
    pub fn on_pubcomp(&mut self, pkid: u16) -> Option<OutgoingOp> {
        if !self.inflight.outgoing_rel[pkid as usize] {
            return None;
        }
        self.inflight.outgoing_rel[pkid as usize] = false;
        self.inflight.release_ack(pkid)
    }
    pub fn on_incoming_qos2_publish(&mut self, pkid: u16) {
        self.inflight.incoming_pub[pkid as usize] = true;
    }
    pub fn on_incoming_pubrel(&mut self, pkid: u16) -> bool {
        let seen = self.inflight.incoming_pub[pkid as usize];
        self.inflight.incoming_pub[pkid as usize] = false;
        seen
    }
    pub fn clean_for_reconnect(&mut self, clean_session: bool) {
        for slot in &mut self.inflight.outgoing {
            if let Some(op) = slot.take() {
                self.inflight.pending.push(op);
            }
        }
        self.inflight.inflight = 0;
        self.inflight.collision = None;
        if clean_session {
            self.pkid_pool.reset();
            self.inflight.last_ack = 0;
            self.inflight.outgoing_rel.fill(false);
            self.inflight.incoming_pub.fill(false);
        }
    }
    pub fn on_command_publish(&mut self, command: Command) -> Result<Option<Packet>, RuntimeError> {
        let (token_id, mut publish) = match command {
            Command::Publish { token_id, publish } => (token_id, publish),
            _ => return Err(RuntimeError::InvalidCommand),
        };
        let needs_ack = publish.qos != Qos::AtMostOnce;
        if needs_ack && publish.pkid == 0 {
            publish.pkid = self.pkid_pool.next_candidate();
        }
        let wire = Packet::Publish(publish.clone());
        let op = OutgoingOp::new(
            token_id,
            Command::Publish {
                token_id,
                publish: publish.clone(),
            },
            wire.clone(),
        );
        // check whether the runtime is offline
        if !self.active {
            self.inflight.pending.push(op);
            return Ok(None);
        }
        // qos1/qos2 tracking in inflight
        if needs_ack {
            let inserted = self.inflight.try_insert(publish.pkid, op);
            if !inserted {
                if self.inflight.collision.is_none() {
                    self.inflight.pending.push(OutgoingOp::new(
                        token_id,
                        Command::Publish { token_id, publish },
                        wire,
                    ));
                }
                return Ok(None);
            }
        }
        Ok(Some(wire))
    }
    pub fn resend_pending_on_reconnect(&mut self) -> Vec<Packet> {
        self.active = true;
        let pending = self.inflight.pending.drain(..).collect::<Vec<_>>();
        let mut out = Vec::new();
        for op in pending {
            if let Command::Publish {
                token_id,
                mut publish,
            } = op.command
            {
                if publish.qos != Qos::AtMostOnce {
                    if publish.pkid == 0 {
                        publish.pkid = self.pkid_pool.next_candidate();
                    }
                    publish.dup = true;
                }
                let wire = Packet::Publish(publish.clone());
                let reinjected = OutgoingOp::new(
                    token_id,
                    Command::Publish {
                        token_id,
                        publish: publish.clone(),
                    },
                    wire.clone(),
                );
                if publish.qos != Qos::AtMostOnce {
                    if !self.inflight.try_insert(publish.pkid, reinjected) {
                        if self.inflight.collision.is_none() {
                            self.inflight.pending.push(OutgoingOp::new(
                                token_id,
                                Command::Publish { token_id, publish },
                                wire,
                            ));
                        }
                        continue;
                    }
                }
                out.push(wire);
            }
        }
        out
    }

    pub fn on_puback_checked(&mut self, pkid: u16) -> Result<AckResult, RuntimeError> {
        let released = self
            .inflight
            .release_ack(pkid)
            .ok_or(RuntimeError::UnsolicitedAck(pkid))?;
        let next_packet = self.promote_collision(pkid);
        Ok(AckResult {
            completion: Completion::PubAck {
                token_id: released.token_id,
                pkid,
            },
            next_packet,
        })
    }
    fn promote_collision(&mut self, acked_pkid: u16) -> Option<Packet> {
        let (pkid, op) = self.inflight.collision.take()?;
        if pkid != acked_pkid {
            self.inflight.collision = Some((pkid, op));
            return None;
        }
        let wire = mark_publish_dup(op.packet.clone());
        if self.inflight.try_insert(pkid, op) {
            Some(wire)
        } else {
            None
        }
    }
}

fn mark_publish_dup(packet: Packet) -> Packet {
    match packet {
        Packet::Publish(mut p) => {
            if p.qos != Qos::AtMostOnce {
                p.dup = true;
            }
            Packet::Publish(p)
        }
        p => p,
    }
}
