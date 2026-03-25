use std::{
    cmp::max,
    time::{Duration, Instant},
};

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
    PubComp { token_id: usize, pkid: u16 },
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
    UnsolicitedPubRec(u16),
    UnsolicitedPubComp(u16),
    UnsolicitedPubRel(u16),
    PacketIdOutOfRange(u16),
    AwaitPingRespTimeout,
    ConnectionInactive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncomingQos2Result {
    FirstSeen { pubrec: Packet },
    Duplicate { pubrec: Packet },
}

#[derive(Debug)]
pub struct RuntimeState {
    pub pkid_pool: PacketIdPool,
    pub inflight: InflightStore,
    pub active: bool,
    pub keep_alive: Duration,
    pub await_pingresp: bool,
    pub last_incoming: Instant,
    pub last_outgoing: Instant,
    pub ping_retry_count: u8,
}

impl RuntimeState {
    pub fn new(max_inflight: u16) -> Self {
        Self::new_with_keep_alive(max_inflight, Duration::from_secs(30))
    }
    pub fn new_with_keep_alive(max_inflight: u16, keep_alive: Duration) -> Self {
        Self {
            pkid_pool: PacketIdPool::new(max_inflight),
            inflight: InflightStore::new(max_inflight),
            active: false,
            keep_alive,
            await_pingresp: false,
            last_incoming: Instant::now(),
            last_outgoing: Instant::now(),
            ping_retry_count: 0,
        }
    }
    pub fn on_tick(&mut self, now: Instant) -> Result<Option<Packet>, RuntimeError> {
        if !self.active {
            return Ok(None);
        }
        // i'm waiting
        if self.await_pingresp && now.duration_since(self.last_outgoing) >= self.keep_alive {
            return Err(RuntimeError::AwaitPingRespTimeout);
        }
        // i need to send
        let idle_since = max(self.last_incoming, self.last_outgoing);
        if now.duration_since(idle_since) >= self.keep_alive {
            self.await_pingresp = true;
            self.last_outgoing = now;
            Ok(Some(Packet::PingReq))
        } else {
            Ok(None)
        }
    }

    pub fn on_pingresp(&mut self, now: Instant) {
        self.await_pingresp = false;
        self.last_incoming = now;
    }
    pub fn on_connection_lost(&mut self, clean_session: bool) {
        self.active = false;
        self.await_pingresp = false;
        self.ping_retry_count = 0;

        for slot in &mut self.inflight.outgoing {
            if let Some(op) = slot.take() {
                self.inflight.pending.push(op);
            }
        }

        if let Some((_, ops)) = self.inflight.collision.take() {
            self.inflight.pending.push(ops);
        }

        self.inflight.inflight = 0;
        if clean_session {
            self.pkid_pool.reset();
            self.inflight.last_ack = 0;
            self.inflight.outgoing_rel.fill(false);
            self.inflight.incoming_pub.fill(false);
        }
    }
    pub fn on_connection_restored(&mut self) -> Vec<Packet> {
        self.active = true;
        self.resend_pending_on_reconnect()
    }
    pub fn note_incoming_activity(&mut self, now: Instant) {
        self.last_incoming = now;
    }
    pub fn note_outgoing_activity(&mut self, now: Instant) {
        self.last_outgoing = now;
    }

    pub fn set_active(&mut self, active: bool) {
        self.active = active;
    }

    pub fn on_pubrec_checked(&mut self, pkid: u16) -> Result<Packet, RuntimeError> {
        let i = self.idx(pkid)?;
        if self.inflight.outgoing[i].is_none() {
            return Err(RuntimeError::UnsolicitedPubRec(pkid));
        }
        self.inflight.outgoing_rel[i] = true;
        Ok(Packet::PubRel(crate::types::PubRel { pkid }))
    }

    pub fn on_pubcomp_checked(&mut self, pkid: u16) -> Result<AckResult, RuntimeError> {
        let i = self.idx(pkid)?;
        if !self.inflight.outgoing_rel[i] || self.inflight.outgoing[i].is_none() {
            return Err(RuntimeError::UnsolicitedPubComp(pkid));
        }
        self.inflight.outgoing_rel[i] = false;
        let released = self
            .inflight
            .release_ack(pkid)
            .ok_or(RuntimeError::UnsolicitedPubComp(pkid))?;
        let next_packet = self.promote_collision(pkid);
        Ok(AckResult {
            completion: Completion::PubComp {
                token_id: released.token_id,
                pkid,
            },
            next_packet,
        })
    }

    pub fn on_incoming_qos2_publish_checked(
        &mut self,
        pkid: u16,
    ) -> Result<IncomingQos2Result, RuntimeError> {
        let i = self.idx(pkid)?;
        let pubrec = Packet::PubRec(crate::types::PubRec { pkid });
        if self.inflight.incoming_pub[i] {
            return Ok(IncomingQos2Result::Duplicate { pubrec });
        }
        self.inflight.incoming_pub[i] = true;
        Ok(IncomingQos2Result::FirstSeen { pubrec })
    }

    pub fn on_incoming_pubrel_checked(&mut self, pkid: u16) -> Result<Packet, RuntimeError> {
        let i = self.idx(pkid)?;
        if !self.inflight.incoming_pub[i] {
            return Err(RuntimeError::UnsolicitedPubRel(pkid));
        }
        self.inflight.incoming_pub[i] = false;
        Ok(Packet::PubComp(crate::types::PubComp { pkid }))
    }

    fn idx(&self, pkid: u16) -> Result<usize, RuntimeError> {
        let i = pkid as usize;
        if pkid == 0 || i >= self.inflight.outgoing.len() {
            return Err(RuntimeError::PacketIdOutOfRange(pkid));
        }
        Ok(i)
    }
    pub fn clean_for_reconnect(&mut self, clean_session: bool) {
        self.on_connection_lost(clean_session);
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
