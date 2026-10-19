//! `gx100` — a small command-line tool for the BOSS GX-100.

#[cfg(feature = "midi")]
mod midi;

use gx100_protocol::patch::{self, FX_SLOTS};
use gx100_protocol::session::{Device, ParamValue, Transport};
use gx100_protocol::sim::SimulatedGx100;
use gx100_protocol::tables::{self, FxField, TablesSource};
use gx100_protocol::Address;
use std::error::Error;
use std::time::Duration;

const USAGE: &str = "\
gx100 — command-line tool for the BOSS GX-100

USAGE: gx100 [--port <name>] [--sim] [--allow-system] <command> [args]

Offline:
  find <text>                  search parameters by path or description
  fx-types [<type>]            list FX types, or the parameters of one type
  commands                     list special command addresses
  ports                        list MIDI ports

Talking to the pedal (or the simulator with --sim):
  identify                     identity request/reply
  hello                        handshake + current patch summary (read-only)
  get <path|address> [size]    read a parameter, or raw bytes at an address
  set <path> <value>           write a parameter of the live (Temp) patch
  slots                        list the live patch's FX slots
  fx-get <slot>                show every parameter of FX slot <slot>
  fx-set <slot> <param> <value>
                               set an FX parameter (label or number)

Options:
  --port <name>    MIDI port to use (substring; default: \"GX-100\")
  --sim            use an in-memory simulated pedal instead of MIDI
  --allow-system   let `set` write outside the live patch (system settings)

Writes only touch the live patch: nothing is saved to a memory slot, so
switching patches on the pedal discards them.";

type Res<T = ()> = Result<T, Box<dyn Error>>;

struct Opts {
    port: Option<String>,
    sim: bool,
    allow_system: bool,
    args: Vec<String>,
}

fn parse_args() -> Res<Opts> {
    let mut o = Opts { port: None, sim: false, allow_system: false, args: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => o.port = Some(it.next().ok_or("--port needs a value")?),
            "--sim" => o.sim = true,
            "--allow-system" => o.allow_system = true,
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => o.args.push(a),
        }
    }
    Ok(o)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res {
    let o = parse_args()?;
    if tables::SOURCE == TablesSource::Fixture {
        eprintln!("warning: built with the synthetic test tables; run `node tools/extract/extract.mjs` and rebuild.");
    }
    let Some(cmd) = o.args.first() else {
        println!("{USAGE}");
        return Ok(());
    };
    let rest = &o.args[1..];
    match cmd.as_str() {
        "find" => find(rest),
        "fx-types" => fx_types(rest),
        "commands" => commands(),
        "ports" => ports(),
        "identify" | "hello" | "get" | "set" | "slots" | "fx-get" | "fx-set" => {
            let mut dev = connect(&o)?;
            match cmd.as_str() {
                "identify" => Ok(()), // connect() already identified
                "hello" => hello(&mut dev),
                "get" => get(&mut dev, rest),
                "set" => set(&mut dev, rest, o.allow_system),
                "slots" => slots(&mut dev),
                "fx-get" => fx_get(&mut dev, rest),
                _ => fx_set(&mut dev, rest),
            }
        }
        other => Err(format!("unknown command {other:?} (try --help)").into()),
    }
}

// ------------------------------------------------------------------ transport

enum Conn {
    Sim(SimulatedGx100),
    #[cfg(feature = "midi")]
    Midi(midi::MidiTransport),
}

impl Transport for Conn {
    fn send(&mut self, msg: &[u8]) -> Result<(), String> {
        match self {
            Conn::Sim(t) => t.send(msg),
            #[cfg(feature = "midi")]
            Conn::Midi(t) => t.send(msg),
        }
    }
    fn recv(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, String> {
        match self {
            Conn::Sim(t) => t.recv(timeout),
            #[cfg(feature = "midi")]
            Conn::Midi(t) => t.recv(timeout),
        }
    }
}

fn connect(o: &Opts) -> Res<Device<Conn>> {
    let conn = if o.sim {
        println!("using the simulated pedal");
        Conn::Sim(SimulatedGx100::new())
    } else {
        open_midi(o.port.as_deref())?
    };
    let mut dev = Device::new(conn);
    let id = dev.identify()?;
    println!(
        "GX-100 found: device id {:02X}, revision {}",
        id.device_id,
        id.revision.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
    );
    Ok(dev)
}

#[cfg(feature = "midi")]
fn open_midi(port: Option<&str>) -> Res<Conn> {
    let t = midi::open(port)?;
    println!("MIDI in: {}  |  out: {}", t.in_name, t.out_name);
    Ok(Conn::Midi(t))
}

#[cfg(not(feature = "midi"))]
fn open_midi(_port: Option<&str>) -> Res<Conn> {
    Err("built without the `midi` feature; use --sim".into())
}

/// Runs `f` with the pedal in editor mode, always switching it back off.
fn with_editor_mode<T: Transport, R>(dev: &mut Device<T>, f: impl FnOnce(&mut Device<T>) -> Res<R>) -> Res<R> {
    dev.set_editor_mode(true)?;
    let r = f(dev);
    let off = dev.set_editor_mode(false);
    let r = r?;
    off?;
    Ok(r)
}

// ------------------------------------------------------------------ offline

fn find(rest: &[String]) -> Res {
    let q = rest.first().ok_or("usage: find <text>")?;
    let mut n = 0;
    for p in tables::search_params(q) {
        println!("{}  {:<48} {:>6}..{:<6} {}", p.addr, p.path, p.min, p.max, p.desc);
        n += 1;
    }
    println!("{n} match(es)");
    Ok(())
}

fn fx_types(rest: &[String]) -> Res {
    match rest.first() {
        None => {
            for t in tables::FX_TYPES {
                println!("{:>3}  {}", t.id, t.name);
            }
        }
        Some(q) => {
            let t = match q.parse::<u8>() {
                Ok(id) => tables::fx_type(id),
                Err(_) => tables::fx_type_by_name(&rest.join(" ")),
            }
            .ok_or_else(|| format!("no FX type {q:?}"))?;
            println!("{} (type {}):", t.name, t.id);
            for p in t.params {
                let field = match p.field {
                    FxField::Param(i) => format!("param({i})"),
                    f => format!("{f:?}"),
                };
                println!("  {:<22} {:<11} {:>6}..{:<6} {}", p.unique_name, field, p.min, p.max, p.template);
            }
        }
    }
    Ok(())
}

fn commands() -> Res {
    for c in tables::COMMANDS {
        println!("{}  {}", c.addr, c.name);
    }
    Ok(())
}

#[cfg(feature = "midi")]
fn ports() -> Res {
    let (ins, outs) = midi::port_names()?;
    println!("inputs:");
    ins.iter().for_each(|n| println!("  {n}"));
    println!("outputs:");
    outs.iter().for_each(|n| println!("  {n}"));
    Ok(())
}

#[cfg(not(feature = "midi"))]
fn ports() -> Res {
    Err("built without the `midi` feature".into())
}

// ------------------------------------------------------------------ live

fn hello<T: Transport>(dev: &mut Device<T>) -> Res {
    let level = dev.communication_level()?;
    let want = tables::PRODUCT.communication_level;
    println!("editor protocol level: {level} (this tool targets {want})");
    if level != want {
        println!("warning: level mismatch — addresses may differ from this pedal's firmware");
    }
    if let Some(p) = tables::param("System_common.CurrentPatchNum") {
        println!("current patch number: {}", dev.read_param(p)?);
    }
    if let Some(p) = tables::param("Temp_patch.common.Name") {
        println!("live patch name: {}", dev.read_param(p)?);
    }
    slots(dev)
}

fn get<T: Transport>(dev: &mut Device<T>, rest: &[String]) -> Res {
    let target = rest.first().ok_or("usage: get <path|address> [size]")?;
    if let Some(p) = tables::param(target) {
        println!("{} @ {} = {}", p.path, p.addr, dev.read_param(p)?);
        return Ok(());
    }
    let addr =
        Address::parse(target).ok_or_else(|| format!("{target:?} is neither a parameter path nor an address"))?;
    let size: u32 = rest.get(1).map(|s| s.parse()).transpose()?.unwrap_or(1);
    let data = dev.read(addr, size)?;
    for (i, chunk) in data.chunks(16).enumerate() {
        let at = addr.offset(i as u32 * 16);
        let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02X}")).collect();
        let label = tables::param_at(at).map(|p| p.path).unwrap_or("");
        println!("{at}  {:<47}  {label}", hex.join(" "));
    }
    Ok(())
}

fn set<T: Transport>(dev: &mut Device<T>, rest: &[String], allow_system: bool) -> Res {
    let [path, value, ..] = rest else { return Err("usage: set <path> <value>".into()) };
    let p = tables::param(path).ok_or_else(|| format!("unknown parameter {path:?} (try `find`)"))?;
    if !p.path.starts_with("Temp_patch.") && !allow_system {
        return Err(format!("{} is outside the live patch; pass --allow-system to write it", p.path).into());
    }
    let v = if p.encoding.is_text() {
        ParamValue::Text(value.clone())
    } else {
        ParamValue::Num(value.parse().map_err(|_| format!("{value:?} is not a number"))?)
    };
    let after = with_editor_mode(dev, |d| {
        d.write_param(p, &v)?;
        Ok(d.read_param(p)?)
    })?;
    println!("{} = {after}", p.path);
    Ok(())
}

fn slots<T: Transport>(dev: &mut Device<T>) -> Res {
    for s in dev.read_fx_slots(patch::temp_patch())? {
        let name = s.fx_type().map(|t| t.name).unwrap_or("?");
        println!("  slot {:>2}  {:<3}  {name}", s.slot, if s.on { "ON" } else { "off" });
    }
    Ok(())
}

fn slot_arg(s: Option<&String>) -> Res<u8> {
    let n: u8 = s.ok_or("missing slot number")?.parse().map_err(|_| "slot must be a number")?;
    if !(1..=FX_SLOTS).contains(&n) {
        return Err(format!("slot must be 1..={FX_SLOTS}").into());
    }
    Ok(n)
}

fn fx_get<T: Transport>(dev: &mut Device<T>, rest: &[String]) -> Res {
    let slot = slot_arg(rest.first())?;
    let patch = patch::temp_patch();
    let s = dev.read_fx_slot(patch, slot)?;
    let t = s.fx_type().ok_or_else(|| format!("slot {slot} has unknown type {}", s.type_id))?;
    println!("slot {slot}: {} ({})", t.name, if s.on { "ON" } else { "off" });
    for p in t.params.iter().filter(|p| matches!(p.field, FxField::Param(_))) {
        let v = dev.read_fx_param(patch, slot, p)?;
        println!("  {:<22} {}", p.unique_name, p.display(v));
    }
    Ok(())
}

fn fx_set<T: Transport>(dev: &mut Device<T>, rest: &[String]) -> Res {
    let slot = slot_arg(rest.first())?;
    let (Some(name), Some(value)) = (rest.get(1), rest.get(2..).filter(|v| !v.is_empty())) else {
        return Err("usage: fx-set <slot> <param> <value>".into());
    };
    let value = value.join(" ");
    let patch = patch::temp_patch();
    let s = dev.read_fx_slot(patch, slot)?;
    let t = s.fx_type().ok_or_else(|| format!("slot {slot} has unknown type {}", s.type_id))?;
    let p = t.param(name).ok_or_else(|| {
        let names: Vec<&str> = t.params.iter().map(|p| p.unique_name).collect();
        format!("{} has no parameter {name:?}; it has: {}", t.name, names.join(", "))
    })?;
    let v = p.parse_value(&value).ok_or_else(|| format!("{value:?} is not a valid value for {}", p.unique_name))?;
    let after = with_editor_mode(dev, |d| {
        d.write_fx_param(patch, slot, p, v)?;
        Ok(d.read_fx_param(patch, slot, p)?)
    })?;
    println!("slot {slot} {} {} = {}", t.name, p.unique_name, p.display(after));
    Ok(())
}
