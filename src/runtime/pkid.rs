#[derive(Debug, Clone)]
pub struct PacketIdPool {
    last_pkid: u16,
    max_inflight: u16,
}

impl PacketIdPool {
    pub fn new(max_inflight: u16) -> Self {
        assert!(max_inflight > 0);
        Self {
            last_pkid: 0,
            max_inflight,
        }
    }

    pub fn next_candidate(&mut self) -> u16 {
        let mut next = self.last_pkid + 1;
        if next == 0 || next > self.max_inflight {
            next = 1;
        }
        self.last_pkid = next;
        next
    }
    pub fn reset(&mut self) {
        self.last_pkid = 0;
    }
}
