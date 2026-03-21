use crate::types::{Command, Packet};

#[derive(Debug, Clone)]
pub struct OutgoingOp {
    pub token_id: usize,
    pub command: Command,
    pub packet: Packet,
}

impl OutgoingOp {
    pub fn new(token_id: usize, command: Command, packet: Packet) -> Self {
        Self {
            token_id,
            command,
            packet,
        }
    }
}

#[derive(Debug)]
pub struct InflightStore {
    pub outgoing: Vec<Option<OutgoingOp>>,
    pub inflight: u16,
    max_inflight: u16,
    pub last_ack: u16,
    pub collision: Option<(u16, OutgoingOp)>,
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
