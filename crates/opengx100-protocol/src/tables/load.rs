//! Builds [`Tables`] from the extractor's JSON (`tablesBundle` in `tools/extract/core.js`) and the
//! optional parameter-guide text (`docs.json` from `tools/extract/manual.mjs`). Strings and lists are
//! leaked: tables are loaded once and live as long as the program.

use super::*;
use crate::json::Json;

type Res<T> = Result<T, String>;

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_owned().into_boxed_str())
}

fn leak<T>(v: Vec<T>) -> &'static [T] {
    Box::leak(v.into_boxed_slice())
}

fn field<'a>(j: &'a Json, key: &str, what: &str) -> Res<&'a Json> {
    j.get(key).ok_or_else(|| format!("{what}: missing `{key}`"))
}

fn array<'a>(j: &'a Json, key: &str, what: &str) -> Res<&'a [Json]> {
    field(j, key, what)?.as_array().ok_or_else(|| format!("{what}: `{key}` isn't a list"))
}

/// A string field; missing or null reads as "" (the extractor leaves out undefined values).
fn text(j: &Json, key: &str) -> &'static str {
    match j.get(key) {
        Some(Json::Str(s)) => leak_str(s),
        Some(Json::Num(n)) => leak_str(&n.to_string()),
        _ => "",
    }
}

/// An integer, given as a number or a numeric string (like JavaScript's `Number(v)`).
fn int_of(v: &Json) -> Option<i64> {
    let n = match v {
        Json::Num(n) => *n,
        Json::Str(s) if !s.trim().is_empty() => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    (n.fract() == 0.0 && n.abs() < 9.0e15).then_some(n as i64)
}

fn int(j: &Json, key: &str, what: &str) -> Res<i32> {
    let v = field(j, key, what)?;
    int_of(v).and_then(|n| i32::try_from(n).ok()).ok_or_else(|| format!("{what}: `{key}` isn't an integer: {v:?}"))
}

/// An integer field that defaults when missing or null.
fn int_or(j: &Json, key: &str, default: i32, what: &str) -> Res<i32> {
    match j.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(_) => int(j, key, what),
    }
}

/// "10 00 11 01" → address.
fn wire(hex: &str, what: &str) -> Res<Address> {
    let b: Vec<u8> = hex.split_whitespace().filter_map(|x| u8::from_str_radix(x, 16).ok()).collect();
    match b[..] {
        [a, b2, c, d] if b.iter().all(|&x| x < 0x80) && hex.split_whitespace().count() == 4 => {
            Ok(Address::from_wire([a, b2, c, d]))
        }
        _ => Err(format!("{what}: bad address {hex:?}")),
    }
}

fn wire_field(j: &Json, key: &str, what: &str) -> Res<Address> {
    wire(field(j, key, what)?.as_str().ok_or_else(|| format!("{what}: `{key}` isn't text"))?, what)
}

/// "000000000B" → bytes.
fn hex_bytes<const N: usize>(h: &str, what: &str) -> Res<[u8; N]> {
    let bad = || format!("{what}: expected {N} hex bytes, got {h:?}");
    if h.len() != 2 * N || !h.is_ascii() {
        return Err(bad());
    }
    let mut out = [0; N];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&h[2 * i..2 * i + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// The editor's default value: a number, or text (patch names).
fn init(j: &Json) -> Init {
    match j.get("init") {
        Some(v) if int_of(v).is_some_and(|n| i32::try_from(n).is_ok()) => Init::Num(int_of(v).unwrap_or(0) as i32),
        Some(Json::Str(s)) => Init::Text(leak_str(s)),
        _ => Init::Text(""),
    }
}

fn encoding(j: &Json, what: &str) -> Res<Encoding> {
    let e = field(j, "encoding", what)?.as_str().unwrap_or("");
    let bytes = || int(j, "bytes", what).and_then(|b| u16::try_from(b).map_err(|_| format!("{what}: bad size {b}")));
    if let Some(n) = e.strip_prefix("INTEGER1x").and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=7).contains(n)) {
        return Ok(Encoding::Int1(n));
    }
    Ok(match e {
        "INTEGER2x4" => Encoding::Int2x4,
        "INTEGER2x7" => Encoding::Int2x7,
        "INTEGER4x4" => Encoding::Int4x4,
        "BYTES" => Encoding::Bytes(bytes()?),
        "PADDING" => Encoding::Padding(bytes()?),
        _ => return Err(format!("{what}: unknown encoding {e:?}")),
    })
}

fn named(j: &Json, what: &str) -> Res<&'static [NamedAddress]> {
    let kv = j.as_object().ok_or_else(|| format!("{what} isn't an object"))?;
    let mut v = kv
        .iter()
        .map(|(k, a)| {
            let addr = wire(a.as_str().unwrap_or(""), k)?;
            Ok(NamedAddress { name: leak_str(k), addr })
        })
        .collect::<Res<Vec<_>>>()?;
    v.sort_by(|a, b| a.name.cmp(b.name));
    Ok(leak(v))
}

fn product(j: &Json) -> Res<ProductDef> {
    let w = "product";
    Ok(ProductDef {
        name: text(j, "name"),
        bts_version: text(j, "btsVersion"),
        bts_build: text(j, "btsBuild"),
        model_id: hex_bytes(text(j, "modelId"), "product modelId")?,
        device_id: u8::from_str_radix(text(j, "deviceId"), 16).map_err(|_| format!("{w}: bad deviceId"))?,
        communication_level: u8::try_from(int(j, "communicationLevel", w)?).map_err(|_| format!("{w}: bad level"))?,
        family_code: hex_bytes(text(j, "familyCode"), "product familyCode")?,
    })
}

fn param(j: &Json) -> Res<ParamDef> {
    let path = text(j, "path");
    Ok(ParamDef {
        path,
        desc: text(j, "desc"),
        addr: wire_field(j, "addr", path)?,
        encoding: encoding(j, path)?,
        min: int(j, "min", path)?,
        max: int(j, "max", path)?,
        init: init(j),
        ofs: int_or(j, "ofs", 0, path)?,
        factor: int_or(j, "factor", 1, path)?,
    })
}

/// Labels in stored-value order. Where the editor lists them in another order (`indexMachineValue`),
/// stored value v is labelled `values[indexMachineValue[v]]`.
fn values_by_machine(p: &Json, what: &str) -> Res<&'static [&'static str]> {
    let Some(values) = p.get("values").and_then(Json::as_array) else { return Ok(&[]) };
    let labels: Vec<&'static str> = values.iter().map(|v| leak_str(v.as_str().unwrap_or(""))).collect();
    let Some(imv) = p.get("indexMachineValue").and_then(Json::as_array) else { return Ok(leak(labels)) };
    let order: Vec<usize> = imv.iter().filter_map(|v| int_of(v).and_then(|n| usize::try_from(n).ok())).collect();
    let mut sorted = order.clone();
    sorted.sort_unstable();
    if order.len() != labels.len() || sorted.iter().enumerate().any(|(i, &x)| x != i) {
        return Err(format!("fx param {what}: indexMachineValue isn't a permutation of its labels"));
    }
    Ok(leak(order.iter().map(|&i| labels[i]).collect()))
}

fn fx_field(p: &Json, what: &str) -> Res<FxField> {
    if p.get("paramIndex").is_some() {
        return u8::try_from(int(p, "paramIndex", what)?).map(FxField::Param).map_err(|e| e.to_string());
    }
    match text(p, "field") {
        "Type" => Ok(FxField::Type),
        "Switch" => Ok(FxField::Switch),
        "DuplicationNumber" => Ok(FxField::DuplicationNumber),
        f => Err(format!("fx param {what}: no field ({f:?})")),
    }
}

/// `"#RRGGBB"`.
fn rgb(s: &str) -> Option<[u8; 3]> {
    let h = s.strip_prefix('#').filter(|h| h.len() == 6)?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    Some([byte(0)?, byte(2)?, byte(4)?])
}

fn fx_type(j: &Json, id: u8) -> Res<FxTypeDef> {
    let name = text(j, "name");
    let params = match j.get("parameters").and_then(Json::as_array) {
        None => &[][..],
        Some(ps) => leak(
            ps.iter()
                .map(|p| {
                    let what = format!("{name}.{}", text(p, "name"));
                    let format = p.get("format").and_then(Json::as_str).map(leak_str);
                    Ok(FxParamDef {
                        name: text(p, "name"),
                        unique_name: text(p, "uniqueName"),
                        field: fx_field(p, &what)?,
                        offset: wire_field(p, "offsetInFxItem", &what)?,
                        encoding: encoding(p, &what)?,
                        min: int(p, "min", &what)?,
                        max: int(p, "max", &what)?,
                        init: init(p),
                        ofs: int_or(p, "ofs", 0, &what)?,
                        factor: int_or(p, "factor", 1, &what)?,
                        values: values_by_machine(p, &what)?,
                        format,
                        template: text(p, "template"),
                    })
                })
                .collect::<Res<Vec<_>>>()?,
        ),
    };
    let color = j.get("color").and_then(Json::as_str).and_then(rgb);
    Ok(FxTypeDef { id, name, color, params })
}

fn param_doc(d: &Json) -> ParamDoc {
    let options = d.get("options").and_then(Json::as_array).unwrap_or(&[]);
    let options = options
        .iter()
        .filter_map(|o| match o.as_array()? {
            [l, t] => Some((leak_str(l.as_str()?), leak_str(t.as_str()?))),
            _ => None,
        })
        .collect();
    ParamDoc { key: text(d, "key"), text: text(d, "text"), options: leak(options) }
}

fn docs(j: &Json) -> (&'static [FxDoc], &'static [ParamDoc]) {
    let list = |k| j.get(k).and_then(Json::as_array).unwrap_or(&[]);
    let fx = list("fx")
        .iter()
        .map(|f| FxDoc {
            fx: text(f, "fx"),
            text: text(f, "text"),
            params: leak(f.get("params").and_then(Json::as_array).unwrap_or(&[]).iter().map(param_doc).collect()),
        })
        .collect();
    let mut params: Vec<ParamDoc> = list("params").iter().map(param_doc).collect();
    params.sort_by(|a, b| a.key.cmp(b.key));
    params.dedup_by(|a, b| a.key == b.key);
    (leak(fx), leak(params))
}

/// Builds the tables from the extractor's bundle and, optionally, the parameter guide's text.
pub fn from_json(bundle: &str, guide: Option<&str>) -> Res<Tables> {
    let b = Json::parse(bundle).map_err(|e| format!("tables: {e}"))?;
    match b.get("format").and_then(int_of) {
        Some(2) => {}
        f => return Err(format!("tables: unsupported format {f:?} (re-extract them)")),
    }
    let source = if b.get("fixture").and_then(Json::as_bool) == Some(true) {
        TablesSource::Fixture
    } else {
        TablesSource::Extracted
    };
    let commands = field(&b, "commands", "tables")?;

    let root = array(&b, "root", "tables")?
        .iter()
        .map(|r| {
            let name = text(r, "name");
            let size = wire_field(r, "size", name)?.linear();
            Ok(RootDef { name, block: text(r, "block"), addr: wire_field(r, "addr", name)?, size })
        })
        .collect::<Res<Vec<_>>>()?;

    let mut params = array(&b, "params", "tables")?.iter().map(param).collect::<Res<Vec<_>>>()?;
    params.sort_by(|a, b| a.path.cmp(b.path));

    let mut param_values = array(&b, "paramValues", "tables")?
        .iter()
        .map(|v| {
            let path = text(v, "path");
            let labels = array(v, "labels", path)?.iter().map(|l| leak_str(l.as_str().unwrap_or(""))).collect();
            Ok(ParamValues { path, first: int(v, "first", path)?, labels: leak(labels) })
        })
        .collect::<Res<Vec<_>>>()?;
    param_values.sort_by(|a, b| a.path.cmp(b.path));

    let mut fx_types = Vec::new();
    for (i, fx) in array(&b, "effects", "tables")?.iter().enumerate() {
        if int(fx, "typeId", "fx type")? as usize != i {
            return Err(format!("fx type {} out of order", text(fx, "name")));
        }
        fx_types.push(fx_type(fx, u8::try_from(i).map_err(|_| "too many fx types")?)?);
    }
    let master = match b.get("master") {
        Some(m) if !m.is_null() => fx_type(m, 99)?,
        _ => FxTypeDef { id: 99, name: "MASTER", color: None, params: &[] },
    };

    let assign = field(&b, "assign", "tables")?;
    let assign_categories = array(assign, "categories", "assign")?
        .iter()
        .map(|c| AssignCategoryDef {
            name: text(c, "name"),
            has_switch: c.get("hasSwitch").and_then(Json::as_bool).unwrap_or(false),
            block_name: text(c, "blockName"),
        })
        .collect();
    let assign_targets = array(assign, "targets", "assign")?
        .iter()
        .map(|t| {
            let name = text(t, "name");
            Ok(AssignTargetDef {
                name,
                category: i16::try_from(int(t, "category", name)?).map_err(|e| e.to_string())?,
                effect: t.get("effect").and_then(Json::as_bool).unwrap_or(true),
                unique_name: text(t, "uniqueName"),
            })
        })
        .collect::<Res<Vec<_>>>()?;

    let (fx_docs, param_docs) = match guide {
        Some(g) => docs(&Json::parse(g).map_err(|e| format!("docs: {e}"))?),
        None => (&[][..], &[][..]),
    };

    Ok(Tables {
        source,
        product: product(field(&b, "product", "tables")?)?,
        commands: named(field(commands, "commands", "commands")?, "commands")?,
        addresses: named(field(commands, "addresses", "commands")?, "addresses")?,
        root: leak(root),
        params: leak(params),
        param_values: leak(param_values),
        fx_types: leak(fx_types),
        master,
        assign_categories: leak(assign_categories),
        assign_targets: leak(assign_targets),
        fx_docs,
        param_docs,
    })
}
