use crate::{
    runtime::{
        inflight::{InflightStore, OutgoingOp},
        pkid::PacketIdPool,
    },
    types::{Command, Packet},
};

pub enum Completion {
    PubAck { token_id: usize, pkid: u16 },
}

pub struct AckResult {
    pub completion: Completion,
    pub next_packet: Option<Packet>,
}

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
        unimplemented!()
    }
    pub fn resend_pending_on_reconnect(&mut self) -> Vec<Packet> {
        unimplemented!()
    }
    pub fn on_puback_checked(&mut self, pkid: u16) -> Result<AckResult, RuntimeError> {
        unimplemented!()
    }
    fn promote_collision(&mut self, acked_pkid: u16) -> Option<Packet> {
        unimplemented!()
    }
    fn mark_publish_dup(&self) -> Packet {
        unimplemented!()
    }
}
