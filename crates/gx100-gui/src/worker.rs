//! The thread that owns the pedal connection. The UI sends [`Cmd`]s and gets [`Event`]s back,
//! so slow MIDI round trips never block drawing.
//!
//! Live updates without polling: once connected, the pedal is put in editor mode (as the official
//! editor does) and then sends a DT1 by itself whenever something changes on it: a footswitch, the
//! expression pedal, a knob, a patch change. The MIDI input thread wakes this one with
//! [`Cmd::MidiArrived`], so it sleeps until there is either a UI request or pedal data.
//!
//! Writes go through dedicated commands, each the way the official editor does it: parameter edits
//! of the live patch and the system settings ([`Cmd::WriteBytes`]), switching patches, WRITE (storing
//! the live patch), loading a patch into the live patch, saving an input setting, and writing a
//! setlist (a background job that reads every patch back to verify it).

use crate::store::{self, Area, Backup};
use gx100_protocol::backup::{system_areas, AreaData};
use gx100_protocol::patch::{self, PatchData, USER_PATCHES};
use gx100_protocol::session::{Device, Notification, ParamValue, Transport};
use gx100_protocol::sim::SimulatedGx100;
use gx100_protocol::tables;
use gx100_protocol::Address;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PatchId {
    /// The live edit buffer (what the pedal is playing now).
    Temp,
    /// Stored user patch, 1-based.
    User(u16),
}

impl PatchId {
    fn addr(self) -> Option<gx100_protocol::Address> {
        match self {
            PatchId::Temp => Some(patch::temp_patch()),
            PatchId::User(n) => patch::user_patch(n),
        }
    }
}

pub enum Cmd {
    Connect {
        sim: bool,
    },
    /// Re-reads every user patch name.
    LoadNames,
    LoadPatch(PatchId),
    /// Read everything a full backup holds (system areas + every user patch), in the background.
    Backup,
    CancelBackup,
    /// Writes a parameter of the live patch or of the system settings. Anything else is refused
    /// (stored patches are only written by [`Cmd::SavePatch`] and [`Cmd::WriteSetlist`]).
    WriteBytes {
        addr: Address,
        data: Vec<u8>,
    },
    /// Re-reads `size` bytes of the live patch at `offset` (reported as [`Event::TempChanged`]): the
    /// pedal changes some values itself, e.g. an assign's TARGET MIN/MAX when its TARGET is written.
    ReadTemp {
        offset: u32,
        size: u32,
    },
    /// Re-reads the system settings.
    ReadSystem,
    /// Switches the pedal to user patch n (1-based).
    SelectPatch(u16),
    /// Stores the live patch into user patch `target` under `name` (the pedal's WRITE).
    SavePatch {
        target: u16,
        name: String,
    },
    /// Loads a patch into the live patch without storing it (e.g. to drop unsaved edits).
    LoadIntoTemp(Box<PatchData>),
    /// Stores the current input settings into the selected input-setting slot.
    SaveInputSetting {
        name: String,
    },
    /// Writes patches into user patch slots, in the background: (target, data).
    WriteSetlist(Vec<(u16, PatchData)>),
    CancelSetlistWrite,
    /// Leave editor mode and stop.
    Shutdown,
    /// Sent by the MIDI input thread: the pedal sent something.
    MidiArrived,
}

pub enum Event {
    Connected {
        desc: String,
        /// The user patch the pedal is on, if it reports one (1-based).
        current: Option<u16>,
        /// Whether the pedal will push changes (editor mode is on).
        live: bool,
    },
    Name(u16, String),
    Patch(PatchId, PatchData),
    /// The pedal changed bytes of the live patch: `data` at `offset` from its start.
    TempChanged {
        offset: u32,
        data: Vec<u8>,
    },
    /// The pedal switched to another user patch (1-based). The live patch is re-read after this.
    CurrentPatch(u16),
    BackupProgress {
        done: usize,
        total: usize,
    },
    BackupDone(Box<Backup>),
    BackupCancelled,
    System(Vec<AreaData>),
    /// The pedal changed system settings: `data` at `addr`.
    SystemChanged {
        addr: Address,
        data: Vec<u8>,
    },
    /// User patch n was stored (WRITE).
    Saved(u16),
    SetlistProgress {
        done: usize,
        total: usize,
    },
    /// The setlist write ended: how many patches were written (and verified).
    SetlistDone {
        written: usize,
        total: usize,
    },
    Error(String),
}

type Dev = Device<Box<dyn Transport>>;

/// A backup in progress: the system areas first, then one user patch per step.
struct BackupJob {
    system: Option<Vec<Area>>,
    patches: Vec<(u16, String, PatchData)>,
}

impl BackupJob {
    const TOTAL: usize = 1 + USER_PATCHES as usize;

    fn done(&self) -> usize {
        self.system.is_some() as usize + self.patches.len()
    }

    /// Reads the next piece. Returns the finished backup after the last one.
    fn step(&mut self, d: &mut Dev, send: &impl Fn(Event)) -> Result<Option<Backup>, String> {
        if self.system.is_none() {
            let areas = d.read_system_areas().map_err(|e| format!("reading system settings: {e}"))?;
            let areas = areas.into_iter().map(|a| Area { name: a.name.into(), addr: a.addr, data: a.data }).collect();
            self.system = Some(areas);
        } else {
            let n = self.patches.len() as u16 + 1;
            let addr = patch::user_patch(n).ok_or_else(|| format!("no user patch {n}"))?;
            let data = d.read_patch(addr).map_err(|e| format!("reading patch {n}: {e}"))?;
            // Also fills the UI's cache: after a backup every patch opens instantly.
            send(Event::Patch(PatchId::User(n), data.clone()));
            self.patches.push((n, data.name(), data));
        }
        send(Event::BackupProgress { done: self.done(), total: Self::TOTAL });
        if self.done() < Self::TOTAL {
            return Ok(None);
        }
        Ok(Some(Backup {
            origin: store::Origin::Editor,
            created: store::now(),
            bts_version: tables::PRODUCT.bts_version.to_string(),
            system: self.system.take().unwrap_or_default(),
            patches: std::mem::take(&mut self.patches),
        }))
    }
}

/// Writing a setlist: one patch per step, each read back and compared.
struct SetlistJob {
    todo: VecDeque<(u16, PatchData)>,
    total: usize,
    written: usize,
}

impl SetlistJob {
    /// Writes the next patch. Returns true once all are written.
    fn step(&mut self, d: &mut Dev, send: &impl Fn(Event)) -> Result<bool, String> {
        let Some((n, data)) = self.todo.pop_front() else { return Ok(true) };
        let addr = patch::user_patch(n).ok_or_else(|| format!("no user patch {n}"))?;
        d.write_user_patch(n, &data).map_err(|e| format!("writing patch {n}: {e}"))?;
        let back = d.read_patch(addr).map_err(|e| format!("reading patch {n} back: {e}"))?;
        if let Some(b) = patch::patch_blocks().iter().find(|b| {
            let r = b.offset as usize..(b.offset + b.size) as usize;
            back.bytes()[r.clone()] != data.bytes()[r]
        }) {
            return Err(format!("patch {n} didn't read back as written (block {})", b.name));
        }
        send(Event::Patch(PatchId::User(n), back));
        self.written += 1;
        send(Event::SetlistProgress { done: self.written, total: self.total });
        Ok(self.todo.is_empty())
    }
}

/// Where a UI write may go: the live patch or a system area, never the stored patches.
fn writable(addr: Address, len: usize) -> bool {
    let end = addr.linear() + len as u32;
    let temp = patch::temp_patch();
    let in_temp = patch::temp_offset(addr).is_some() && end <= temp.linear() + patch::patch_span();
    let in_system = system_areas().any(|r| addr.linear() >= r.addr.linear() && end <= r.addr.linear() + r.size);
    in_temp || in_system
}

/// Runs until [`Cmd::Shutdown`] or the UI drops its [`Sender`]. `wake` is a sender into this
/// thread's own queue, handed to the MIDI input. `notify` is called after every event (to wake the UI).
pub fn run(rx: Receiver<Cmd>, wake: Sender<Cmd>, tx: Sender<Event>, notify: impl Fn()) {
    let mut dev: Option<Dev> = None;
    let mut names: VecDeque<u16> = VecDeque::new();
    let mut backup: Option<BackupJob> = None;
    let mut setlist: Option<SetlistJob> = None;
    let send = |e: Event| {
        let _ = tx.send(e);
        notify();
    };
    loop {
        // Patch loads and pedal data jump ahead of the (long) name scan and backup.
        let cmd = if names.is_empty() && backup.is_none() && setlist.is_none() {
            rx.recv().ok()
        } else {
            match rx.try_recv() {
                Ok(c) => Some(c),
                Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => break,
            }
        };
        match cmd {
            Some(Cmd::Connect { sim }) => {
                if let Some(mut d) = dev.take() {
                    let _ = d.set_editor_mode(false);
                }
                names.clear();
                if backup.take().is_some() {
                    send(Event::BackupCancelled);
                }
                if let Some(job) = setlist.take() {
                    send(Event::SetlistDone { written: job.written, total: job.total });
                }
                match connect(sim, wake.clone()) {
                    Ok((mut d, desc)) => {
                        let current = current_patch(&mut d);
                        // The simulator never pushes anything; a real pedal does once in editor mode.
                        let live = !sim && enter_editor_mode(&mut d, &send);
                        send(Event::Connected { desc, current, live });
                        match d.read_system_areas() {
                            Ok(areas) => send(Event::System(areas)),
                            Err(e) => send(Event::Error(format!("reading system settings: {e}"))),
                        }
                        names.extend(1..=USER_PATCHES);
                        dev = Some(d);
                    }
                    Err(e) => send(Event::Error(e)),
                }
            }
            Some(Cmd::LoadNames) => {
                names.clear();
                names.extend(1..=USER_PATCHES);
            }
            Some(Cmd::LoadPatch(id)) => {
                let Some(d) = dev.as_mut() else {
                    send(Event::Error("not connected".into()));
                    continue;
                };
                let Some(addr) = id.addr() else { continue };
                match d.read_patch(addr) {
                    Ok(p) => send(Event::Patch(id, p)),
                    Err(e) => send(Event::Error(format!("reading {id:?}: {e}"))),
                }
            }
            Some(Cmd::Backup) => {
                if dev.is_none() {
                    send(Event::Error("not connected".into()));
                } else if backup.is_none() {
                    backup = Some(BackupJob { system: None, patches: Vec::new() });
                    send(Event::BackupProgress { done: 0, total: BackupJob::TOTAL });
                }
            }
            Some(Cmd::CancelBackup) => {
                if backup.take().is_some() {
                    send(Event::BackupCancelled);
                }
            }
            Some(Cmd::WriteBytes { addr, data }) => {
                let Some(d) = dev.as_mut() else { continue };
                if !writable(addr, data.len()) {
                    send(Event::Error(format!("refused to write {} bytes at {addr}", data.len())));
                } else if let Err(e) = d.write(addr, &data) {
                    send(Event::Error(format!("writing {addr}: {e}")));
                }
            }
            Some(Cmd::ReadTemp { offset, size }) => {
                let Some(d) = dev.as_mut() else { continue };
                match d.read(patch::temp_patch().offset(offset), size) {
                    Ok(data) => send(Event::TempChanged { offset, data }),
                    Err(e) => send(Event::Error(format!("reading the live patch: {e}"))),
                }
            }
            Some(Cmd::ReadSystem) => {
                let Some(d) = dev.as_mut() else { continue };
                match d.read_system_areas() {
                    Ok(areas) => send(Event::System(areas)),
                    Err(e) => send(Event::Error(format!("reading system settings: {e}"))),
                }
            }
            Some(Cmd::SelectPatch(n)) => {
                let Some(d) = dev.as_mut() else { continue };
                match d.select_patch(n).and_then(|()| d.read_patch(patch::temp_patch())) {
                    Ok(p) => {
                        send(Event::CurrentPatch(n));
                        send(Event::Patch(PatchId::Temp, p));
                    }
                    Err(e) => send(Event::Error(format!("switching to patch {n}: {e}"))),
                }
            }
            Some(Cmd::SavePatch { target, name }) => {
                let Some(d) = dev.as_mut() else { continue };
                let Some(addr) = patch::user_patch(target) else { continue };
                let result = d.save_temp_patch(target, &name).and_then(|()| {
                    let stored = d.read_patch(addr)?;
                    let temp = d.read_patch(patch::temp_patch())?;
                    Ok((stored, temp))
                });
                match result {
                    Ok((stored, temp)) => {
                        send(Event::Patch(PatchId::User(target), stored));
                        send(Event::Patch(PatchId::Temp, temp));
                        send(Event::Saved(target));
                    }
                    Err(e) => send(Event::Error(format!("WRITE to patch {target} failed: {e}"))),
                }
            }
            Some(Cmd::LoadIntoTemp(data)) => {
                let Some(d) = dev.as_mut() else { continue };
                match d.load_into_temp(&data).and_then(|()| d.read_patch(patch::temp_patch())) {
                    Ok(p) => send(Event::Patch(PatchId::Temp, p)),
                    Err(e) => send(Event::Error(format!("loading into the live patch: {e}"))),
                }
            }
            Some(Cmd::SaveInputSetting { name }) => {
                let Some(d) = dev.as_mut() else { continue };
                match d.save_input_setting(&name).and_then(|()| d.read_system_areas()) {
                    Ok(areas) => send(Event::System(areas)),
                    Err(e) => send(Event::Error(format!("saving the input setting: {e}"))),
                }
            }
            Some(Cmd::WriteSetlist(list)) => {
                if dev.is_none() || setlist.is_some() || backup.is_some() {
                    send(Event::Error("can't write the setlist now (not connected, or busy)".into()));
                } else {
                    let total = list.len();
                    setlist = Some(SetlistJob { todo: list.into(), total, written: 0 });
                    send(Event::SetlistProgress { done: 0, total });
                }
            }
            Some(Cmd::CancelSetlistWrite) => {
                if let Some(job) = setlist.take() {
                    send(Event::SetlistDone { written: job.written, total: job.total });
                }
            }
            Some(Cmd::MidiArrived) => {
                if let Some(d) = dev.as_mut() {
                    drain_notifications(d, &send);
                }
            }
            Some(Cmd::Shutdown) => break,
            // `recv` failed: the UI is gone.
            None if names.is_empty() && backup.is_none() && setlist.is_none() => break,
            // A setlist write goes before everything else in the background.
            None if setlist.is_some() => {
                let (Some(d), Some(job)) = (dev.as_mut(), setlist.as_mut()) else { continue };
                match job.step(d, &send) {
                    Ok(false) => {}
                    Ok(true) => {
                        let (written, total) = (job.written, job.total);
                        setlist = None;
                        send(Event::SetlistDone { written, total });
                    }
                    Err(e) => {
                        let (written, total) = (job.written, job.total);
                        setlist = None;
                        send(Event::Error(format!("setlist write stopped: {e}")));
                        send(Event::SetlistDone { written, total });
                    }
                }
            }
            // Nothing else to do: continue the backup once the names are in.
            None if names.is_empty() => {
                let (Some(d), Some(job)) = (dev.as_mut(), backup.as_mut()) else { continue };
                match job.step(d, &send) {
                    Ok(None) => {}
                    Ok(Some(b)) => {
                        backup = None;
                        send(Event::BackupDone(Box::new(b)));
                    }
                    Err(e) => {
                        backup = None;
                        send(Event::BackupCancelled);
                        send(Event::Error(format!("backup failed, nothing saved: {e}")));
                    }
                }
            }
            // Read the next patch name.
            None => {
                let (Some(d), Some(n)) = (dev.as_mut(), names.pop_front()) else { continue };
                let Some(addr) = patch::user_patch(n) else { continue };
                match d.read_patch_name(addr) {
                    Ok(name) => send(Event::Name(n, name)),
                    Err(e) => {
                        names.clear();
                        send(Event::Error(format!("reading patch names: {e}")));
                    }
                }
            }
        }
    }
    if let Some(mut d) = dev {
        let _ = d.set_editor_mode(false);
    }
}

fn connect(sim: bool, wake: Sender<Cmd>) -> Result<(Dev, String), String> {
    let (transport, desc): (Box<dyn Transport>, String) = if sim {
        (Box::new(demo_sim()), "simulated GX-100".into())
    } else {
        let t = gx100_midi::open_with_waker(None, move || {
            let _ = wake.send(Cmd::MidiArrived);
        })?;
        let desc = format!("GX-100 on {}", t.in_name);
        (Box::new(t), desc)
    };
    let mut dev = Device::new(transport);
    if sim {
        dev = dev.without_pacing().with_timeout(Duration::from_millis(100));
    }
    dev.identify().map_err(|e| e.to_string())?;
    Ok((dev, desc))
}

/// The official editor's handshake: check the protocol level, then switch editor mode on.
fn enter_editor_mode(d: &mut Dev, send: &impl Fn(Event)) -> bool {
    let want = tables::PRODUCT.communication_level;
    match d.communication_level() {
        Ok(level) if level == want => match d.set_editor_mode(true) {
            Ok(()) => true,
            Err(e) => {
                send(Event::Error(format!("couldn't enable live updates: {e}")));
                false
            }
        },
        Ok(level) => {
            send(Event::Error(format!("pedal speaks editor protocol {level}, expected {want}; live updates off")));
            false
        }
        Err(e) => {
            send(Event::Error(format!("reading the protocol level: {e}")));
            false
        }
    }
}

/// Forwards everything the pedal has sent on its own. Returns once nothing is waiting.
fn drain_notifications(d: &mut Dev, send: &impl Fn(Event)) {
    let mut patch_changed = false;
    let mut stored_on_pedal = Vec::new();
    while let Ok(Some(n)) = d.next_notification(Duration::ZERO) {
        // WRITE pressed on the pedal: that user patch changed.
        if Some(n.addr) == tables::command("PATCH_WRITE") && n.data.len() >= 2 {
            stored_on_pedal.push((n.data[0] as u16) << 7 | n.data[1] as u16);
        } else if let Some(num) = current_patch_from(&n) {
            send(Event::CurrentPatch(num));
            patch_changed = true;
        } else if let Some(offset) = patch::temp_offset(n.addr) {
            send(Event::TempChanged { offset, data: n.data });
        } else if system_areas()
            .any(|r| n.addr.linear() >= r.addr.linear() && n.addr.linear() < r.addr.linear() + r.size)
        {
            send(Event::SystemChanged { addr: n.addr, data: n.data });
        }
    }
    // A patch change only pushes the number and name, so read the rest, as the official editor does.
    if patch_changed {
        reread_temp(d, send);
    }
    if !stored_on_pedal.is_empty() {
        std::thread::sleep(Duration::from_millis(500));
        let _ = d.wait_until_ready();
        for i in stored_on_pedal {
            let n = i + 1;
            if let Some(Ok(p)) = patch::user_patch(n).map(|a| d.read_patch(a)) {
                send(Event::Patch(PatchId::User(n), p));
                send(Event::Saved(n));
            }
        }
    }
}

/// The pedal needs a moment after a patch change. Like the official editor, wait until it says it
/// accepts reads again (`IS_RQ1_ACCEPT`, retried every 250 ms for up to 10 s), then read.
fn reread_temp(d: &mut Dev, send: &impl Fn(Event)) {
    std::thread::sleep(Duration::from_millis(500));
    let _ = d.wait_until_ready();
    match d.read_patch(patch::temp_patch()) {
        Ok(p) => send(Event::Patch(PatchId::Temp, p)),
        Err(e) => send(Event::Error(format!("re-reading the live patch: {e}"))),
    }
}

fn current_patch_from(n: &Notification) -> Option<u16> {
    let p = tables::param("System_common.CurrentPatchNum")?;
    if n.addr != p.addr || n.data.len() < p.encoding.size() as usize {
        return None;
    }
    let v = p.spec().decode(&n.data[..p.encoding.size() as usize]).ok()?;
    user_patch_number(v)
}

/// `System_common.CurrentPatchNum` is 0-based; user patch numbers are 1-based.
fn user_patch_number(raw: i32) -> Option<u16> {
    (0..USER_PATCHES as i32).contains(&raw).then(|| raw as u16 + 1)
}

fn current_patch(dev: &mut Dev) -> Option<u16> {
    match dev.read_param(tables::param("System_common.CurrentPatchNum")?).ok()? {
        ParamValue::Num(n) => user_patch_number(n),
        ParamValue::Text(_) => None,
    }
}

/// A simulator whose user patches are copies of its (default) Temp patch, so the UI has something to show.
fn demo_sim() -> SimulatedGx100 {
    let mut sim = SimulatedGx100::new();
    let temp = patch::temp_patch();
    let mut bytes = sim.peek(temp, patch::patch_span());
    let at = |path: &str| tables::param(path).and_then(|p| p.addr.since(temp)).map(|o| o as usize);
    let set_type = |bytes: &mut Vec<u8>, slot: u8, name: &str| {
        let base = patch::fx_slot_base(temp, slot).and_then(|b| b.since(temp));
        if let (Some(o), Some(t)) = (base, tables::fx_type_by_name(name)) {
            bytes[o as usize] = t.id;
            bytes[o as usize + 1] = 1;
        }
    };
    // A chain with a parallel section: 1 → DIVIDER(2) → [3 | 5, 6] → MIXER(7) → 8
    for (slot, name) in [(2, "DIVIDER"), (4, "SPLITTER"), (7, "MIXER")] {
        set_type(&mut bytes, slot, name);
    }
    let order = [1u8, 2, 3, 4, 5, 6, 7, 8];
    let mut prev: Option<u8> = None;
    for &s in &order {
        let path = match prev {
            None => "Temp_patch.efct.ChainTopItem".to_string(),
            Some(p) => format!("Temp_patch.efct.ChainNextItem{}", p - 1),
        };
        if let Some(o) = at(&path) {
            bytes[o] = s; // item s - 1, stored + 1
        }
        prev = Some(s);
    }
    if let Some(o) = at("Temp_patch.efct.ChainNextItem7") {
        bytes[o] = 0;
    }
    sim.poke(temp, &bytes);
    for n in 1..=USER_PATCHES {
        let Some(addr) = patch::user_patch(n) else { continue };
        if let Some(o) = at("Temp_patch.common.Name") {
            bytes[o..o + 16].copy_from_slice(format!("DEMO {n:<11}").as_bytes());
        }
        sim.poke(addr, &bytes);
    }
    sim
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// These tests walk the real system areas and patch layout, which the synthetic fixture lacks:
    /// run them with `GX100_TABLES=extracted/tables.json`.
    fn real_tables() -> bool {
        let real = *tables::SOURCE == tables::TablesSource::Extracted;
        if !real {
            eprintln!("skipped: needs the real tables (GX100_TABLES=…/tables.json)");
        }
        real
    }

    #[test]
    fn backup_job_reads_everything_then_finishes() {
        if !real_tables() {
            return;
        }
        let transport: Box<dyn Transport> = Box::new(demo_sim());
        let mut d = Device::new(transport).without_pacing().with_timeout(Duration::from_millis(50));
        let events = RefCell::new(0usize);
        let mut job = BackupJob { system: None, patches: Vec::new() };
        let mut result = None;
        for _ in 0..BackupJob::TOTAL {
            assert!(result.is_none(), "finished early");
            result = job.step(&mut d, &|_| *events.borrow_mut() += 1).unwrap();
        }
        let b = result.expect("finished after the last step");
        assert_eq!(b.patches.len(), USER_PATCHES as usize);
        assert_eq!(b.patches[41].0, 42);
        assert_eq!(b.patches[41].1, "DEMO 42");
        assert!(!b.system.is_empty());
    }

    #[test]
    fn ui_writes_only_reach_the_live_patch_and_system() {
        if !real_tables() {
            return;
        }
        let temp = patch::temp_patch();
        assert!(writable(temp, 16), "patch name");
        let level = tables::param("System_common.WarningDelete").unwrap();
        assert!(writable(level.addr, 1), "a system setting");
        assert!(!writable(patch::user_patch(1).unwrap(), 1), "stored patches only via WRITE / setlists");
        assert!(!writable(tables::command("PATCH_WRITE").unwrap(), 2), "commands");
        assert!(!writable(temp.offset(patch::patch_span() - 1), 4), "runs past the live patch");
        // What the assign editor sends: a field, then the whole block.
        let mut d = patch::PatchData::from_bytes(vec![0; patch::patch_span() as usize]);
        for n in 1..=gx100_protocol::assign::ASSIGNS {
            for (offset, data) in gx100_protocol::assign::set_field(&mut d, n, "SW", 1).unwrap_or_default() {
                assert!(writable(temp.offset(offset), data.len()), "assign {n}");
            }
        }
    }

    #[test]
    fn setlist_job_writes_and_verifies() {
        if !real_tables() {
            return;
        }
        let transport: Box<dyn Transport> = Box::new(demo_sim());
        let mut d = Device::new(transport).without_pacing().with_timeout(Duration::from_millis(50));
        let a = d.read_patch(patch::user_patch(3).unwrap()).unwrap();
        let b = d.read_patch(patch::user_patch(4).unwrap()).unwrap();
        // swap 3 and 4
        let mut job = SetlistJob { todo: vec![(3, b.clone()), (4, a.clone())].into(), total: 2, written: 0 };
        let events = RefCell::new(Vec::new());
        assert!(!job.step(&mut d, &|e| events.borrow_mut().push(e)).unwrap());
        assert!(job.step(&mut d, &|e| events.borrow_mut().push(e)).unwrap());
        assert_eq!(job.written, 2);
        assert_eq!(d.read_patch(patch::user_patch(3).unwrap()).unwrap().name(), "DEMO 4");
        assert_eq!(d.read_patch(patch::user_patch(4).unwrap()).unwrap().name(), "DEMO 3");
        let progress = events.borrow().iter().filter(|e| matches!(e, Event::SetlistProgress { .. })).count();
        assert_eq!(progress, 2);
    }

    #[test]
    fn pushed_changes_become_events() {
        let mut sim = SimulatedGx100::new();
        let sw = tables::param("Temp_patch.fxItem(2).Switch").unwrap();
        sim.push_unsolicited(sw.addr, &[1]);
        let num = tables::param("System_common.CurrentPatchNum").unwrap();
        sim.push_unsolicited(num.addr, &num.spec().encode(131).unwrap());
        let transport: Box<dyn Transport> = Box::new(sim);
        let mut d = Device::new(transport).without_pacing().with_timeout(Duration::from_millis(50));

        let events = RefCell::new(Vec::new());
        drain_notifications(&mut d, &|e| events.borrow_mut().push(e));
        let events = events.into_inner();
        assert!(matches!(&events[0], Event::TempChanged { offset, data }
            if Some(*offset) == patch::temp_offset(sw.addr) && data == &[1]));
        assert!(matches!(events[1], Event::CurrentPatch(132)));
        assert!(matches!(events[2], Event::Patch(PatchId::Temp, _)), "patch change re-reads the live patch");
        assert_eq!(events.len(), 3);
    }
}
