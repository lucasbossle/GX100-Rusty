//! An in-memory stand-in for the pedal: answers identity requests and RQ1s,
//! stores DT1s. Lets you exercise everything without hardware.

use crate::address::Address;
use crate::patch;
use crate::session::Transport;
use crate::sysex::{self, FAMILY_CODE, MODEL_ID, ROLAND};
use crate::tables::{self, Init, PARAMS};
use std::collections::{HashMap, VecDeque};
use std::time::Duration;

pub struct SimulatedGx100 {
    mem: HashMap<u32, u8>,
    outbox: VecDeque<Vec<u8>>,
    sent: Vec<Vec<u8>>,
    device_id: u8,
    family: [u8; 2],
    reply_chunk: usize,
    silent: bool,
}

impl Default for SimulatedGx100 {
    fn default() -> Self {
        Self::new()
    }
}

impl SimulatedGx100 {
    /// A pedal whose memory holds each parameter's default value.
    pub fn new() -> Self {
        let mut sim = SimulatedGx100 {
            mem: HashMap::new(),
            outbox: VecDeque::new(),
            sent: Vec::new(),
            device_id: sysex::DEFAULT_DEVICE_ID,
            family: FAMILY_CODE,
            reply_chunk: 128,
            silent: false,
        };
        for p in PARAMS {
            let data = match p.init {
                Init::Num(n) => p.spec().encode(n).ok(),
                Init::Text(t) => p.spec().encode_text(t.trim_end()).ok(),
            };
            if let Some(d) = data {
                sim.poke(p.addr, &d);
            }
        }
        // Each FX slot holds the defaults of the effect type it contains.
        let temp = patch::temp_patch();
        for slot in 1..=patch::FX_SLOTS {
            let Some(base) = patch::fx_slot_base(temp, slot) else { continue };
            let Some(fx) = tables::fx_type(sim.peek(base, 1)[0]) else { continue };
            for p in fx.params {
                if let Init::Num(n) = p.init {
                    if let Ok(d) = p.spec().encode(n) {
                        sim.poke(base.offset(p.offset.linear()), &d);
                    }
                }
            }
        }
        if let Some(lvl) = tables::command("EDITOR_COMMUNICATION_LEVEL") {
            sim.poke(lvl, &[tables::PRODUCT.communication_level]);
        }
        sim
    }

    pub fn with_device_id(mut self, id: u8) -> Self {
        self.device_id = id;
        self
    }

    pub fn with_family(mut self, family: [u8; 2]) -> Self {
        self.family = family;
        self
    }

    /// Maximum data bytes per DT1 reply.
    pub fn with_reply_chunk(mut self, n: usize) -> Self {
        self.reply_chunk = n.max(1);
        self
    }

    /// Never answers.
    pub fn silent(mut self) -> Self {
        self.silent = true;
        self
    }

    pub fn poke(&mut self, addr: Address, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.mem.insert(addr.linear() + i as u32, b);
        }
    }

    pub fn peek(&self, addr: Address, len: u32) -> Vec<u8> {
        (0..len).map(|i| *self.mem.get(&(addr.linear() + i)).unwrap_or(&0)).collect()
    }

    /// Queues a DT1 as if the pedal sent it on its own (a footswitch press, a pedal move…).
    pub fn push_unsolicited(&mut self, addr: Address, data: &[u8]) {
        self.poke(addr, data);
        self.outbox.push_back(sysex::dt1(self.device_id, addr, data));
    }

    /// Every message received from the host, in order.
    pub fn sent(&self) -> &[Vec<u8>] {
        &self.sent
    }

    fn handle(&mut self, msg: &[u8]) {
        if msg == sysex::identity_request().as_slice() {
            let mut r = vec![0xF0, 0x7E, self.device_id, 0x06, 0x02, ROLAND];
            r.extend_from_slice(&self.family);
            r.extend_from_slice(&[0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0xF7]);
            self.outbox.push_back(r);
            return;
        }
        let header_ok = msg.len() >= 15
            && msg[0] == 0xF0
            && msg[1] == ROLAND
            && (msg[2] == self.device_id || msg[2] == 0x7F)
            && msg[3..8] == MODEL_ID;
        if !header_ok {
            return;
        }
        let addr = Address::from_wire([msg[9], msg[10], msg[11], msg[12]]);
        match msg[8] {
            sysex::RQ1 if msg.len() == 19 => {
                let size = Address::from_wire([msg[13], msg[14], msg[15], msg[16]]).linear();
                let data = self.peek(addr, size);
                for (i, chunk) in data.chunks(self.reply_chunk).enumerate() {
                    let at = addr.offset((i * self.reply_chunk) as u32);
                    self.outbox.push_back(sysex::dt1(self.device_id, at, chunk));
                }
            }
            sysex::DT1 if Some(addr) == tables::command("IS_RQ1_ACCEPT") => {
                self.outbox.push_back(sysex::dt1(self.device_id, addr, &[1]));
            }
            // Like the pedal: WRITE stores the Temp patch into a user patch, then confirms.
            sysex::DT1 if Some(addr) == tables::command("PATCH_WRITE") && msg.len() >= 17 => {
                let n = ((msg[13] as u16) << 7 | msg[14] as u16) + 1;
                if let Some(dst) = patch::user_patch(n) {
                    let data = self.peek(patch::temp_patch(), patch::patch_span());
                    self.poke(dst, &data);
                }
                self.outbox.push_back(sysex::dt1(self.device_id, addr, &msg[13..15]));
            }
            sysex::DT1 if Some(addr) == tables::command("SETTING_WRITE_CMD") => {
                self.outbox.push_back(sysex::dt1(self.device_id, addr, &[0]));
            }
            // Selecting a patch loads it into the Temp patch.
            sysex::DT1 if tables::param("System_common.CurrentPatchNum").is_some_and(|p| p.addr == addr) => {
                let data = msg[13..msg.len() - 2].to_vec();
                self.poke(addr, &data);
                let p = tables::param("System_common.CurrentPatchNum").unwrap();
                if let Some(src) = p.spec().decode(&data).ok().and_then(|i| patch::user_patch(i as u16 + 1)) {
                    let bytes = self.peek(src, patch::patch_span());
                    self.poke(patch::temp_patch(), &bytes);
                }
            }
            sysex::DT1 => {
                let data = msg[13..msg.len() - 2].to_vec();
                self.poke(addr, &data);
            }
            _ => {}
        }
    }
}

impl Transport for SimulatedGx100 {
    fn send(&mut self, msg: &[u8]) -> Result<(), String> {
        self.sent.push(msg.to_vec());
        if !self.silent {
            self.handle(msg);
        }
        Ok(())
    }

    fn recv(&mut self, _timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        Ok(self.outbox.pop_front())
    }
}
