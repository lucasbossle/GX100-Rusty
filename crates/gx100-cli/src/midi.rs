//! MIDI I/O through `midir` (CoreMIDI on macOS, WinMM on Windows, ALSA on Linux).

use gx100_protocol::session::Transport;
use gx100_protocol::sysex::SysexAssembler;
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

const CLIENT: &str = "gx100-cli";

pub struct MidiTransport {
    _input: MidiInputConnection<()>,
    output: MidiOutputConnection,
    rx: Receiver<Vec<u8>>,
    pub in_name: String,
    pub out_name: String,
}

/// (input port names, output port names)
pub fn port_names() -> Result<(Vec<String>, Vec<String>), String> {
    let mi = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
    let mo = MidiOutput::new(CLIENT).map_err(|e| e.to_string())?;
    let ins = mi.ports().iter().filter_map(|p| mi.port_name(p).ok()).collect();
    let outs = mo.ports().iter().filter_map(|p| mo.port_name(p).ok()).collect();
    Ok((ins, outs))
}

/// Picks the port whose name contains `filter` (default "GX-100"),
/// preferring one that isn't a DAW-control port.
fn pick(names: &[String], filter: Option<&str>) -> Option<usize> {
    let want = filter.unwrap_or("GX-100").to_lowercase();
    let hits: Vec<usize> = (0..names.len()).filter(|&i| names[i].to_lowercase().contains(&want)).collect();
    hits.iter()
        .copied()
        .find(|&i| {
            let n = names[i].to_uppercase();
            !n.contains("DAW") && !n.contains("CTRL")
        })
        .or(hits.first().copied())
}

fn not_found(kind: &str, filter: Option<&str>, names: &[String]) -> String {
    format!(
        "no MIDI {kind} port matching {:?}. Available: {}. Is the GX-100 connected by USB and its driver installed? \
         Use --port <part of the name> to choose one.",
        filter.unwrap_or("GX-100"),
        if names.is_empty() { "none".to_string() } else { names.join(", ") }
    )
}

pub fn open(filter: Option<&str>) -> Result<MidiTransport, String> {
    let mut mi = MidiInput::new(CLIENT).map_err(|e| e.to_string())?;
    // Keep SysEx; drop clock and active sensing.
    mi.ignore(Ignore::TimeAndActiveSense);
    let mo = MidiOutput::new(CLIENT).map_err(|e| e.to_string())?;

    let in_ports = mi.ports();
    let out_ports = mo.ports();
    let in_names: Vec<String> = in_ports.iter().map(|p| mi.port_name(p).unwrap_or_default()).collect();
    let out_names: Vec<String> = out_ports.iter().map(|p| mo.port_name(p).unwrap_or_default()).collect();
    let i = pick(&in_names, filter).ok_or_else(|| not_found("input", filter, &in_names))?;
    let o = pick(&out_names, filter).ok_or_else(|| not_found("output", filter, &out_names))?;

    let (tx, rx) = mpsc::channel();
    let mut asm = SysexAssembler::new();
    let input = mi
        .connect(
            &in_ports[i],
            "gx100-in",
            move |_stamp, bytes, _| {
                for m in asm.push(bytes) {
                    let _ = tx.send(m);
                }
            },
            (),
        )
        .map_err(|e| e.to_string())?;
    let output = mo.connect(&out_ports[o], "gx100-out").map_err(|e| e.to_string())?;

    Ok(MidiTransport { _input: input, output, rx, in_name: in_names[i].clone(), out_name: out_names[o].clone() })
}

impl Transport for MidiTransport {
    fn send(&mut self, msg: &[u8]) -> Result<(), String> {
        self.output.send(msg).map_err(|e| e.to_string())
    }

    fn recv(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        match self.rx.recv_timeout(timeout) {
            Ok(m) => Ok(Some(m)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err("MIDI input closed".into()),
        }
    }
}
