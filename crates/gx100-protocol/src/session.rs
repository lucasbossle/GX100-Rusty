//! Talking to a pedal over any MIDI transport: request/response, timeouts,
//! pacing, and guarded writes.

use crate::address::Address;
use crate::sysex::{self, IdentityReply, Message};
use crate::tables::{self, ParamDef};
use crate::value::ValueError;
use std::fmt;
use std::time::{Duration, Instant};

/// A bidirectional MIDI connection. Implemented by the CLI with `midir`,
/// and by [`crate::sim::SimulatedGx100`] for tests.
pub trait Transport {
    fn send(&mut self, msg: &[u8]) -> std::result::Result<(), String>;
    /// The next complete MIDI message, or `None` once `timeout` elapses.
    fn recv(&mut self, timeout: Duration) -> std::result::Result<Option<Vec<u8>>, String>;
}

#[derive(Debug)]
pub enum Error {
    Transport(String),
    Timeout(String),
    /// Some, but not all, of the requested bytes arrived.
    Incomplete {
        addr: Address,
        got: u32,
        want: u32,
    },
    /// A device answered the identity request, but it is not a GX-100.
    NotGx100(IdentityReply),
    Value(ValueError),
    Unknown(String),
    /// Blocked by a safety rule.
    Refused(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport(e) => write!(f, "MIDI error: {e}"),
            Error::Timeout(what) => write!(f, "no answer from the pedal ({what})"),
            Error::Incomplete { addr, got, want } => write!(f, "read at {addr}: got {got} of {want} bytes"),
            Error::NotGx100(r) => write!(
                f,
                "the connected device is not a GX-100 (manufacturer {:02X}, family {:02X}{:02X})",
                r.manufacturer, r.family[0], r.family[1]
            ),
            Error::Value(e) => write!(f, "{e}"),
            Error::Unknown(what) => write!(f, "unknown {what}"),
            Error::Refused(why) => write!(f, "refused: {why}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<ValueError> for Error {
    fn from(e: ValueError) -> Self {
        Error::Value(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A parameter value as read from or written to the pedal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParamValue {
    Num(i32),
    Text(String),
}

impl fmt::Display for ParamValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParamValue::Num(n) => write!(f, "{n}"),
            ParamValue::Text(s) => write!(f, "{s:?}"),
        }
    }
}

/// Largest data payload per DT1 we send (the editor's `SYSEX_MAXLEN`).
pub const MAX_DT1_DATA: usize = 128;
/// MIDI DIN speed in bytes/s; the editor paces its output to this.
const BYTES_PER_SEC: u64 = 3125;
/// Commands that may be sent through [`Device::send_command`].
const ALLOWED_COMMANDS: &[&str] = &["EDITOR_COMMUNICATION_MODE", "RUNNING_MODE", "EDITOR_PAGE", "TUNER"];

/// A GX-100 reached through a [`Transport`].
pub struct Device<T: Transport> {
    transport: T,
    device_id: u8,
    timeout: Duration,
    pace: bool,
    next_send: Instant,
}

impl<T: Transport> Device<T> {
    pub fn new(transport: T) -> Self {
        Device {
            transport,
            device_id: sysex::DEFAULT_DEVICE_ID,
            timeout: Duration::from_secs(2),
            pace: true,
            next_send: Instant::now(),
        }
    }

    /// How long to wait for a reply (reset whenever data arrives).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Disables the 3125 bytes/s pacing (useful with simulators).
    pub fn without_pacing(mut self) -> Self {
        self.pace = false;
        self
    }

    pub fn device_id(&self) -> u8 {
        self.device_id
    }

    pub fn transport(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    fn send(&mut self, msg: &[u8]) -> Result<()> {
        if self.pace {
            let now = Instant::now();
            if self.next_send > now {
                std::thread::sleep(self.next_send - now);
            }
        }
        self.transport.send(msg).map_err(Error::Transport)?;
        self.next_send = Instant::now() + Duration::from_micros(msg.len() as u64 * 1_000_000 / BYTES_PER_SEC);
        Ok(())
    }

    fn next_message(&mut self, deadline: Instant) -> Result<Option<Message>> {
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            match self.transport.recv(left).map_err(Error::Transport)? {
                None => return Ok(None),
                // Malformed messages (bad checksum…) are skipped, as the editor does.
                Some(raw) => {
                    if let Ok(m) = sysex::parse(&raw) {
                        return Ok(Some(m));
                    }
                }
            }
        }
    }

    /// Sends the universal identity request and adopts the device ID the pedal reports.
    pub fn identify(&mut self) -> Result<IdentityReply> {
        self.send(&sysex::identity_request())?;
        let deadline = Instant::now() + self.timeout;
        let mut other = None;
        while let Some(m) = self.next_message(deadline)? {
            if let Message::IdentityReply(r) = m {
                if r.is_gx100() {
                    self.device_id = r.device_id;
                    return Ok(r);
                }
                other = Some(r);
            }
        }
        Err(other.map(Error::NotGx100).unwrap_or_else(|| Error::Timeout("identity request".into())))
    }

    /// Reads `size` bytes at `addr`. The pedal may answer with several DT1s.
    pub fn read(&mut self, addr: Address, size: u32) -> Result<Vec<u8>> {
        if size == 0 {
            return Ok(Vec::new());
        }
        self.send(&sysex::rq1(self.device_id, addr, size))?;
        let (start, end) = (addr.linear(), addr.linear() + size);
        let mut buf = vec![0u8; size as usize];
        let mut have = vec![false; size as usize];
        let mut got = 0u32;
        let mut deadline = Instant::now() + self.timeout;
        while got < size {
            let Some(m) = self.next_message(deadline)? else {
                return Err(if got == 0 {
                    Error::Timeout(format!("read {size} bytes at {addr}"))
                } else {
                    Error::Incomplete { addr, got, want: size }
                });
            };
            let Message::Dt1 { device_id, addr: at, data } = m else { continue };
            if device_id != self.device_id && device_id != 0x7F {
                continue;
            }
            for (i, &b) in data.iter().enumerate() {
                let pos = at.linear() + i as u32;
                if (start..end).contains(&pos) {
                    let j = (pos - start) as usize;
                    if !have[j] {
                        have[j] = true;
                        buf[j] = b;
                        got += 1;
                    }
                }
            }
            deadline = Instant::now() + self.timeout;
        }
        Ok(buf)
    }

    /// Writes raw data. Refuses the `7F xx xx xx` command space: use
    /// [`Device::send_command`] for the commands that are safe to send.
    pub fn write(&mut self, addr: Address, data: &[u8]) -> Result<()> {
        if addr.to_wire()[0] == 0x7F {
            return Err(Error::Refused(format!("{addr} is in the command space")));
        }
        self.write_unchecked(addr, data)
    }

    fn write_unchecked(&mut self, addr: Address, data: &[u8]) -> Result<()> {
        for (i, chunk) in data.chunks(MAX_DT1_DATA).enumerate() {
            let at = addr.offset((i * MAX_DT1_DATA) as u32);
            self.send(&sysex::dt1(self.device_id, at, chunk))?;
        }
        Ok(())
    }

    /// Sends one of a small allowlist of commands (never patch write, init or factory reset).
    pub fn send_command(&mut self, name: &str, data: &[u8]) -> Result<()> {
        if !ALLOWED_COMMANDS.contains(&name) {
            return Err(Error::Refused(format!("command {name} is not on the allowlist")));
        }
        let addr = tables::command(name).ok_or_else(|| Error::Unknown(format!("command {name}")))?;
        self.write_unchecked(addr, data)
    }

    /// The editor protocol level the pedal speaks (the official editor expects 3).
    pub fn communication_level(&mut self) -> Result<u8> {
        let addr = tables::command("EDITOR_COMMUNICATION_LEVEL")
            .ok_or_else(|| Error::Unknown("command EDITOR_COMMUNICATION_LEVEL".into()))?;
        Ok(self.read(addr, 1)?[0])
    }

    /// Tells the pedal an editor is connected (the official app does this after the handshake).
    pub fn set_editor_mode(&mut self, on: bool) -> Result<()> {
        self.send_command("EDITOR_COMMUNICATION_MODE", &[on as u8])
    }

    pub fn read_param(&mut self, p: &ParamDef) -> Result<ParamValue> {
        let bytes = self.read(p.addr, p.encoding.size())?;
        Ok(if p.encoding.is_text() {
            ParamValue::Text(p.spec().decode_text(&bytes)?)
        } else {
            ParamValue::Num(p.spec().decode(&bytes)?)
        })
    }

    pub fn write_param(&mut self, p: &ParamDef, value: &ParamValue) -> Result<()> {
        let data = match value {
            ParamValue::Num(n) => p.spec().encode(*n)?,
            ParamValue::Text(s) => p.spec().encode_text(s)?,
        };
        self.write(p.addr, &data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimulatedGx100;

    fn dev(sim: SimulatedGx100) -> Device<SimulatedGx100> {
        Device::new(sim).without_pacing().with_timeout(Duration::from_millis(50))
    }

    #[test]
    fn identify_adopts_device_id() {
        let mut d = dev(SimulatedGx100::new().with_device_id(0x11));
        let r = d.identify().unwrap();
        assert!(r.is_gx100());
        assert_eq!(d.device_id(), 0x11);
        d.read(Address::from_wire([0, 0, 0, 0]), 1).unwrap();
        assert_eq!(d.transport().sent().last().unwrap()[2], 0x11);
    }

    #[test]
    fn identify_rejects_other_devices() {
        let mut d = dev(SimulatedGx100::new().with_family([0x01, 0x02]));
        assert!(matches!(d.identify(), Err(Error::NotGx100(_))));
    }

    #[test]
    fn read_reassembles_chunked_replies() {
        let mut sim = SimulatedGx100::new().with_reply_chunk(7);
        let base = Address::from_wire([0x10, 0x00, 0x7E, 0x00]); // crosses a 7-bit boundary
        let data: Vec<u8> = (0..40).collect();
        sim.poke(base, &data);
        let mut d = dev(sim);
        assert_eq!(d.read(base, 40).unwrap(), data);
    }

    #[test]
    fn silent_pedal_times_out() {
        let mut d = dev(SimulatedGx100::new().silent());
        assert!(matches!(d.read(Address::from_wire([0, 0, 0, 0]), 4), Err(Error::Timeout(_))));
        assert!(matches!(d.identify(), Err(Error::Timeout(_))));
    }

    #[test]
    fn write_then_read_param() {
        let mut d = dev(SimulatedGx100::new());
        let num = tables::PARAMS
            .iter()
            .find(|p| p.path.starts_with("Temp_patch.") && !p.encoding.is_text() && p.max > p.min)
            .expect("a numeric live-patch parameter");
        d.write_param(num, &ParamValue::Num(num.max)).unwrap();
        assert_eq!(d.read_param(num).unwrap(), ParamValue::Num(num.max));
        assert!(matches!(d.write_param(num, &ParamValue::Num(num.max + 1)), Err(Error::Value(_))));
        let name = tables::param("Temp_patch.common.Name").unwrap();
        d.write_param(name, &ParamValue::Text("CRUNCH".into())).unwrap();
        assert_eq!(d.read_param(name).unwrap(), ParamValue::Text("CRUNCH          ".into()));
    }

    #[test]
    fn large_writes_are_split() {
        let mut d = dev(SimulatedGx100::new());
        let base = Address::from_wire([0x10, 0, 0, 0]);
        let data = vec![0x55; 300];
        d.write(base, &data).unwrap();
        assert_eq!(d.transport().sent().len(), 3);
        assert_eq!(d.read(base, 300).unwrap(), data);
    }

    #[test]
    fn command_space_is_guarded() {
        let mut d = dev(SimulatedGx100::new());
        let reset = tables::command("FACTORY_RESET").unwrap();
        assert!(matches!(d.write(reset, &[1]), Err(Error::Refused(_))));
        assert!(matches!(d.send_command("FACTORY_RESET", &[1]), Err(Error::Refused(_))));
        assert!(matches!(d.send_command("PATCH_WRITE", &[0]), Err(Error::Refused(_))));
        assert!(d.transport().sent().is_empty());
        d.set_editor_mode(true).unwrap();
        assert_eq!(d.transport().sent().len(), 1);
    }

    #[test]
    fn communication_level() {
        let mut d = dev(SimulatedGx100::new());
        assert_eq!(d.communication_level().unwrap(), 3);
    }
}
