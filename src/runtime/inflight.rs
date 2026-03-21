use crate::types::{Command, Packet};

#[derive(Debug, Clone)]
pub struct OutgoingOp {
    token_id: u32,
    command: Command,
    packet: Packet,
}

#[derive(Debug)]
pub struct InflightStore {
    outgoing: Vec<Option<OutgoingOp>>,
    inflight: u16,
    max_inflight: u16,
    last_ack: u16,
    collision: Option<(u16, OutgoingOp)>,
    pub pending: Vec<OutgoingOp>,
    pub outgoing_rel: Vec<bool>, // qos2 sender side
    pub incoming_pub: Vec<bool>, // qos2 receiver side
}

impl InflightStore {
    pub fn new(max_inflight: u16) -> Self {
        let n = max_inflight as usize + 1;
        Self {
            outgoing: vec![None; n],
            inflight: 0,
            max_inflight,
            last_ack: 0,
            collision: None,
            pending: Vec::new(),
            outgoing_rel: vec![false; n],
            incoming_pub: vec![false; n],
        }
    }
    pub fn try_insert(&mut self, pkid: u16, op: OutgoingOp) -> bool {
        if self.inflight >= self.max_inflight {
            return false;
        }
        let i = pkid as usize;
        if self.outgoing[i].is_some() {
            self.collision = Some((pkid, op));
            return false;
        }
        self.outgoing[i] = Some(op);
        self.inflight += 1;
        true
    }
    pub fn release_ack(&mut self, pkid: u16) -> Option<OutgoingOp> {
        let i = pkid as usize;
        self.last_ack = pkid;
        let out = self.outgoing[i].take();
        if out.is_some() {
            self.inflight -= 1;
        }
        out
    }
}
