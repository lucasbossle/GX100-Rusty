//! `opengx100` — a small command-line tool for the BOSS GX-100.

#[cfg(feature = "midi")]
use opengx100_midi as midi;

use opengx100_protocol::assign;
use opengx100_protocol::chain;
use opengx100_protocol::patch::{self, FX_SLOTS};
use opengx100_protocol::session::{Device, ParamValue, Transport};
use opengx100_protocol::sim::SimulatedGx100;
use opengx100_protocol::tables::{self, FxField, TablesSource};
use opengx100_protocol::tuner;
use opengx100_protocol::Address;
use std::error::Error;
use std::time::Duration;

const USAGE: &str = "\
opengx100 — command-line tool for the BOSS GX-100

USAGE: opengx100 [--port <name>] [--sim] [--allow-system] <command> [args]

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
  chain                        show the live chain, its free slots and what the pedal says
                               still fits (read-only)
  patch [<n>]                  read a whole patch (user patch n, default: live) and
                               show its chain and every parameter in it
  monitor [<seconds>]          turn editor mode on and print what the pedal sends by
                               itself (footswitches, expression pedal, patch changes)
  tuner [<seconds>]            show the pedal's tuner (its current kind) and print what it measures,
                               then switch back to play (default 10 s)
  write-check                  verify every kind of write with writes that change nothing:
                               re-select the current patch, WRITE it onto itself, rewrite its
                               stored copy, write a system setting, assign 1 and the chain with
                               their own values. Refuses if the live patch has unsaved changes.
  fx-get <slot>                show every parameter of FX slot <slot>
  fx-set <slot> <param> <value>
                               set an FX parameter (label or number)

Options:
  --port <name>    MIDI port to use (substring; default: \"GX-100\")
  --sim            use an in-memory simulated pedal instead of MIDI
  --allow-system   let `set` write outside the live patch (system settings)
  --bts <path>     BOSS TONE STUDIO for GX-100.app to read the parameter tables from
                   (default: found in /Applications; read once, then cached)

Writes only touch the live patch: nothing is saved to a memory slot, so
switching patches on the pedal discards them.";

type Res<T = ()> = Result<T, Box<dyn Error>>;

struct Opts {
    port: Option<String>,
    sim: bool,
    allow_system: bool,
    bts: Option<std::path::PathBuf>,
    args: Vec<String>,
}

fn parse_args() -> Res<Opts> {
    let mut o = Opts { port: None, sim: false, allow_system: false, bts: None, args: Vec::new() };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--port" => o.port = Some(it.next().ok_or("--port needs a value")?),
            "--sim" => o.sim = true,
            "--allow-system" => o.allow_system = true,
            "--bts" => o.bts = Some(it.next().ok_or("--bts needs a path")?.into()),
            "-h" | "--help" | "help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => o.args.push(a),
        }
    }
    Ok(o)
}

/// Reads the parameter tables from BOSS TONE STUDIO (or the cache) before anything looks at them.
fn load_tables(o: &Opts) {
    match opengx100_extract::prepare(o.bts.as_deref()) {
        Ok((t, origin)) => {
            if let opengx100_extract::Origin::Extracted(app) = &origin {
                eprintln!("read the parameter tables from {} (cached for next time)", app.display());
            }
            let _ = tables::install(t);
        }
        Err(e) => eprintln!("warning: {e}. Using the synthetic test tables: real pedals won't make sense."),
    }
    if *tables::SOURCE == TablesSource::Fixture && std::env::var_os("OPENGX100_TABLES").is_none() {
        eprintln!("(install BOSS TONE STUDIO for GX-100, or point --bts at it)");
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Res {
    let o = parse_args()?;
    load_tables(&o);
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
        "identify" | "hello" | "get" | "set" | "slots" | "chain" | "patch" | "monitor" | "tuner" | "write-check"
        | "fx-get" | "fx-set" => {
            let mut dev = connect(&o)?;
            match cmd.as_str() {
                "identify" => Ok(()), // connect() already identified
                "hello" => hello(&mut dev),
                "get" => get(&mut dev, rest),
                "set" => set(&mut dev, rest, o.allow_system),
                "slots" => slots(&mut dev),
                "chain" => show_chain(&mut dev),
                "patch" => show_patch(&mut dev, rest),
                "monitor" => monitor(&mut dev, rest),
                "tuner" => tuner_cmd(&mut dev, rest),
                "write-check" => write_check(&mut dev),
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

fn show_chain<T: Transport>(dev: &mut Device<T>) -> Res {
    let p = dev.read_patch(patch::temp_patch())?;
    let limits = dev.read_chain_limits()?;
    let name = |slot: u8| p.fx_slot(slot).and_then(|s| s.fx_type()).map(|t| t.name).unwrap_or("?");
    println!("live patch: {:?}", p.name());
    let links = chain::Links::read(&p).ok_or("can't read the chain")?;
    println!("links: {:?}", links.0);
    let types = chain::insertable_types().collect::<Vec<_>>();
    for (i, &slot) in p.chain().iter().enumerate() {
        let n = types.iter().filter(|t| limits.can_overwrite(t.id, i)).count();
        println!("  {i:>2}  slot {slot:>2}  {:<22} {n}/{} types could replace it", name(slot), types.len());
    }
    println!("free slots: {:?}", links.free_slots());
    let no: Vec<&str> = types.iter().filter(|t| !limits.can_insert(t.id)).map(|t| t.name).collect();
    match (&limits.insert, no.is_empty()) {
        (None, _) => println!("the tables don't say what fits"),
        (Some(_), true) => println!("every FX type can be added"),
        (Some(_), false) => println!("can't be added now: {}", no.join(", ")),
    }
    Ok(())
}

fn show_patch<T: Transport>(dev: &mut Device<T>, rest: &[String]) -> Res {
    let (label, addr) = match rest.first() {
        None => ("live patch".to_string(), patch::temp_patch()),
        Some(n) => {
            let n: u16 = n.parse().map_err(|_| "patch number must be a number")?;
            let addr = patch::user_patch(n).ok_or_else(|| format!("no user patch {n}"))?;
            (format!("user patch {n}"), addr)
        }
    };
    let p = dev.read_patch(addr)?;
    println!("{label}: {:?}", p.name());
    let chain = p.chain();
    let names: Vec<&str> =
        chain.iter().map(|&s| p.fx_slot(s).and_then(|s| s.fx_type()).map(|t| t.name).unwrap_or("?")).collect();
    println!("chain: {}", names.join(" -> "));
    for &slot in &chain {
        let Some(s) = p.fx_slot(slot) else { continue };
        let Some(t) = s.fx_type() else { continue };
        println!("slot {slot}: {} ({})", t.name, if s.on { "ON" } else { "off" });
        for fp in t.params.iter().filter(|fp| matches!(fp.field, FxField::Param(_))) {
            let v = p.fx_param(slot, fp).map(|v| fp.display(v)).unwrap_or_else(|| "?".into());
            println!("  {:<22} {v}", fp.unique_name);
        }
    }
    // Assigns that do something (on, or pointed at a target).
    for n in 1..=assign::ASSIGNS {
        let Some(target) = assign::target(&p, n) else { continue };
        let on = assign::get(&p, n, "SW") == Some(1);
        if !on && target == assign::Target::None {
            continue;
        }
        let field = |f: &str| match (assign::field(n, f), assign::get(&p, n, f)) {
            (Some(d), Some(v)) => d.display(v),
            _ => "?".into(),
        };
        let range = |f: &str| match (target.param(), assign::get(&p, n, f)) {
            (Some(fp), Some(v)) => fp.display(v),
            (None, Some(v)) => v.to_string(),
            _ => "?".into(),
        };
        println!(
            "assign {n:>2}: {} {}{}  {} -> {}  source {} mode {}",
            if on { "ON " } else { "off" },
            target.label(&p),
            if target.matches_slot() { "" } else { " (slot holds another FX)" },
            range("TARGET_MIN"),
            range("TARGET_MAX"),
            field("SOURCE"),
            field("MODE"),
        );
    }
    Ok(())
}

/// Block names whose bytes differ between two patches.
fn differing_blocks(a: &patch::PatchData, b: &patch::PatchData) -> Vec<String> {
    patch::patch_blocks()
        .iter()
        .filter(|bl| {
            let r = bl.offset as usize..(bl.offset + bl.size) as usize;
            a.bytes()[r.clone()] != b.bytes()[r]
        })
        .map(|bl| bl.name.clone())
        .collect()
}

fn shown(p: &tables::ParamDef, data: &patch::PatchData) -> String {
    match data.value(p) {
        Some(ParamValue::Num(v)) => p.display(v),
        Some(v) => v.to_string(),
        None => "?".into(),
    }
}

/// Like [`differing_blocks`], without the led block (lamp state, not a setting).
fn differing_settings(a: &patch::PatchData, b: &patch::PatchData) -> Vec<String> {
    differing_blocks(a, b).into_iter().filter(|b| b != "led").collect()
}

/// The Temp patch parameters whose bytes differ.
fn differing_params(a: &patch::PatchData, b: &patch::PatchData) -> Vec<&'static str> {
    tables::PARAMS
        .iter()
        .filter_map(|p| {
            let o = patch::temp_offset(p.addr)? as usize;
            let r = o..o + p.encoding.size() as usize;
            (a.bytes().get(r.clone()) != b.bytes().get(r)).then_some(p.path)
        })
        .collect()
}

/// Prints every Temp patch parameter whose bytes differ between `before` and `after`.
fn print_changes(before: &patch::PatchData, after: &patch::PatchData) {
    let (a, b) = (before.bytes(), after.bytes());
    let mut i = 0;
    while i < a.len().min(b.len()) {
        if a[i] == b[i] {
            i += 1;
            continue;
        }
        let p = tables::PARAMS.iter().find(|p| {
            patch::temp_offset(p.addr)
                .is_some_and(|o| (o as usize..o as usize + p.encoding.size() as usize).contains(&i))
        });
        match p {
            Some(p) => {
                let o = patch::temp_offset(p.addr).unwrap_or(0) as usize;
                let r = o..o + p.encoding.size() as usize;
                println!(
                    "    {}: {:02X?} -> {:02X?} ({} -> {})",
                    p.path,
                    &a[r.clone()],
                    &b[r.clone()],
                    shown(p, before),
                    shown(p, after)
                );
                i = r.end;
            }
            None => {
                println!("    offset {i:#x}: {:02X} -> {:02X}", a[i], b[i]);
                i += 1;
            }
        }
    }
}

fn write_check<T: Transport>(dev: &mut Device<T>) -> Res {
    let cur = tables::param("System_common.CurrentPatchNum").ok_or("no CurrentPatchNum")?;
    let n = match dev.read_param(cur)? {
        ParamValue::Num(i) => i as u16 + 1,
        v => return Err(format!("odd patch number {v}").into()),
    };
    let user = patch::user_patch(n).ok_or("current patch isn't a user patch")?;
    let temp = dev.read_patch(patch::temp_patch())?;
    let stored = dev.read_patch(user)?;
    let name = stored.name();
    println!("current patch: {n} {name:?}");
    // Re-selecting the patch reloads the stored copy and WRITE stores the live one, so both are
    // no-ops only while the live patch has no unsaved changes.
    let diff = differing_blocks(&temp, &stored);
    let clean = diff.is_empty();
    if clean {
        println!("live patch is identical to the stored copy: every write below changes nothing");
    } else {
        println!("live patch has unsaved changes in {diff:?}: skipping select and WRITE (they'd lose or store them)");
    }
    let check = |label: &str, ok: bool| -> Res {
        println!("  {label:<56} {}", if ok { "ok" } else { "MISMATCH" });
        if ok {
            Ok(())
        } else {
            Err(format!("{label}: read-back mismatch").into())
        }
    };
    with_editor_mode(dev, |d| {
        if clean {
            d.select_patch(n)?;
            let t = d.read_patch(patch::temp_patch())?;
            check(&format!("select patch {n} (CurrentPatchNum)"), differing_blocks(&t, &stored).is_empty())?;
            check("  …still on the same patch", d.read_param(cur)? == ParamValue::Num(n as i32 - 1))?;
            d.save_temp_patch(n, &name)?;
            let s = d.read_patch(user)?;
            check(
                &format!("WRITE live patch onto patch {n} (PATCH_WRITE, confirmed)"),
                differing_blocks(&s, &stored).is_empty(),
            )?;
        }

        d.write_user_patch(n, &stored)?;
        let s = d.read_patch(user)?;
        check(
            &format!("rewrite stored patch {n} with itself (setlist write)"),
            differing_blocks(&s, &stored).is_empty(),
        )?;

        let p = tables::param("System_common.WarningDelete").ok_or("no WarningDelete")?;
        let before = d.read_param(p)?;
        d.write_param(p, &before)?;
        check(&format!("system setting {} = {before} written back", p.desc), d.read_param(p)? == before)?;

        // The assign editor's messages, each with the assign's own values. The led block (footswitch
        // and FX on/off lamps) is runtime state and is ignored here: changing an assign can light or
        // dim a lamp without anything being stored.
        let send = |d: &mut Device<T>, msgs: Vec<assign::Message>| -> Res {
            for (offset, data) in &msgs {
                d.write(patch::temp_patch().offset(*offset), data)?;
            }
            Ok(())
        };
        let mut live = d.read_patch(patch::temp_patch())?;
        let fields = ["SW", "TARGET", "TARGET_MIN", "TARGET_MAX"].map(|f| assign::get(&live, 1, f));
        if let [Some(sw), Some(target), Some(min), Some(max)] = fields {
            // A field edit: the field, then the whole block.
            send(d, assign::set_field(&mut live, 1, "SW", sw).ok_or("assign 1 SW")?)?;
            let t = d.read_patch(patch::temp_patch())?;
            check("assign 1 SW written back (field + block)", differing_settings(&t, &temp).is_empty())?;

            // A target change: TARGET, then TARGET_FX_ITEM + TARGET. The pedal resets MIN/MAX itself.
            send(d, assign::set_target(&mut live, 1, target).ok_or("assign 1 TARGET")?)?;
            let t = d.read_patch(patch::temp_patch())?;
            print_changes(&temp, &t);
            let changed = differing_params(&temp, &t);
            let expected = ["Temp_patch.assign(1).TARGET_MIN", "Temp_patch.assign(1).TARGET_MAX"];
            check(
                "assign 1 TARGET written back (only MIN/MAX may change)",
                changed.iter().all(|p| expected.contains(p) || p.starts_with("Temp_patch.led.")),
            )?;

            // Put MIN/MAX back as the assign editor would: two field edits.
            let mut back = t.clone();
            send(d, assign::set_field(&mut back, 1, "TARGET_MIN", min).ok_or("assign 1 MIN")?)?;
            send(d, assign::set_field(&mut back, 1, "TARGET_MAX", max).ok_or("assign 1 MAX")?)?;
            let t = d.read_patch(patch::temp_patch())?;
            check("assign 1 MIN/MAX written back", differing_settings(&t, &temp).is_empty())?;
        }

        // A chain edit that keeps the chain as it is: edit trigger on, the whole list, trigger off.
        let live = d.read_patch(patch::temp_patch())?;
        d.edit_chain(&chain::unchanged(&live)?)?;
        let types: Vec<u8> = d.read_fx_slots(patch::temp_patch())?.iter().map(|s| s.type_id).collect();
        while let Some(n) = d.next_notification(Duration::from_millis(300))? {
            let hex: Vec<String> = n.data.iter().map(|b| format!("{b:02X}")).collect();
            println!("    pedal sent {}  {:<24} {}", n.addr, hex.join(" "), describe(n.addr, n.data.len(), &types));
        }
        let t = d.read_patch(patch::temp_patch())?;
        check("chain rewritten with itself (chain edit)", differing_settings(&t, &live).is_empty())?;

        let t = d.read_patch(patch::temp_patch())?;
        check("live patch untouched (lamps aside)", differing_settings(&t, &temp).is_empty())
    })?;
    if !clean {
        println!("select and WRITE not checked: run again on a patch without unsaved changes");
        return Ok(());
    }
    println!("all write paths verified");
    Ok(())
}

fn tuner_cmd<T: Transport>(dev: &mut Device<T>, rest: &[String]) -> Res {
    let secs: u64 = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(10);
    let ty = tables::param("System_common.TunerType").ok_or("no TunerType")?;
    let kind = match dev.read_param(ty)? {
        ParamValue::Num(n) => tuner::Kind::from_type(n),
        v => return Err(format!("odd tuner type {v}").into()),
    };
    println!("tuner {kind:?} for {secs} s — play a string…");
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    with_editor_mode(dev, |d| {
        d.tuner_on(kind)?;
        let mut last = None;
        while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
            let Some(n) = d.next_notification(left)? else { break };
            if Some(n.addr) == tables::command("TUNER") {
                let Some(r) = tuner::Readings::parse(&n.data) else { continue };
                let line = |r: &tuner::Reading| match r.has_signal() {
                    true => format!(
                        "{}{} {:+5}{}",
                        tuner::note_name(r.note),
                        tuner::octave(r.note),
                        r.value as i32 - 5000,
                        if r.in_tune { " ok" } else { "" }
                    ),
                    false => "-".into(),
                };
                let text = format!(
                    "mono {:<16} strings {}",
                    line(&r.mono),
                    r.strings.iter().map(line).collect::<Vec<_>>().join(" | ")
                );
                if last.as_ref() != Some(&text) {
                    println!("  {text}");
                    last = Some(text);
                }
            } else {
                let hex: Vec<String> = n.data.iter().map(|b| format!("{b:02X}")).collect();
                println!("  pedal sent {}  {}", n.addr, hex.join(" "));
            }
        }
        d.tuner_off()?;
        // The pedal confirms the running mode it's in.
        while let Some(n) = d.next_notification(Duration::from_millis(500))? {
            if Some(n.addr) == tables::command("RUNNING_MODE") {
                println!("  running mode now {}", n.data.first().copied().unwrap_or(0));
            }
        }
        Ok(())
    })
}

fn monitor<T: Transport>(dev: &mut Device<T>, rest: &[String]) -> Res {
    let secs: u64 = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(30);
    println!("listening for {secs} s — press footswitches, move the expression pedal, change patches…");
    let temp = patch::temp_patch();
    // Slot types at the start, to name FX params.
    let types: Vec<u8> = dev.read_fx_slots(temp)?.iter().map(|s| s.type_id).collect();
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    with_editor_mode(dev, |d| {
        while let Some(left) = end.checked_duration_since(std::time::Instant::now()) {
            let Some(n) = d.next_notification(left)? else { break };
            let hex: Vec<String> = n.data.iter().map(|b| format!("{b:02X}")).collect();
            let what = describe(n.addr, n.data.len(), &types);
            println!("{}  {:<24} {what}", n.addr, hex.join(" "));
        }
        Ok(())
    })
}

/// Names what lives at `addr`: a table parameter, or an FX parameter of the live patch's slot.
fn describe(addr: Address, len: usize, types: &[u8]) -> String {
    let temp = patch::temp_patch();
    for slot in 1..=FX_SLOTS {
        let Some(base) = patch::fx_slot_base(temp, slot) else { continue };
        let Some(off) = addr.since(base).filter(|&o| o < 0x100) else { continue };
        let ty = types.get(slot as usize - 1).copied().and_then(tables::fx_type);
        let names: Vec<&str> = ty
            .map(|t| {
                t.params
                    .iter()
                    .filter(|p| p.offset.linear() >= off && p.offset.linear() < off + len as u32)
                    .map(|p| p.unique_name)
                    .collect()
            })
            .unwrap_or_default();
        return format!("slot {slot} {} [{}]", ty.map(|t| t.name).unwrap_or("?"), names.join(", "));
    }
    match tables::param_at(addr) {
        Some(p) => format!("{} ({})", p.path, p.desc),
        None => tables::COMMANDS.iter().find(|c| c.addr == addr).map(|c| c.name.to_string()).unwrap_or_default(),
    }
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
