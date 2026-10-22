//! Parameter tables read from the official editor's own data (see `tools/extract`), loaded at run
//! time: the extractor's JSON is [`install`]ed at startup. Without it, lookups fall back to
//! `$OPENGX100_TABLES` or to a tiny synthetic fixture.

use crate::address::Address;
use crate::value::{Encoding, ValueSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TablesSource {
    /// Generated from an installed BOSS TONE STUDIO.
    Extracted,
    /// Synthetic test data: only a handful of parameters.
    Fixture,
}

#[derive(Debug)]
pub struct ProductDef {
    pub name: &'static str,
    pub bts_version: &'static str,
    pub bts_build: &'static str,
    pub model_id: [u8; 5],
    pub device_id: u8,
    pub communication_level: u8,
    pub family_code: [u8; 2],
}

#[derive(Debug, Clone, Copy)]
pub struct NamedAddress {
    pub name: &'static str,
    pub addr: Address,
}

/// A top-level memory area (System, Temp patch, User patch n, …).
#[derive(Debug, Clone, Copy)]
pub struct RootDef {
    pub name: &'static str,
    pub block: &'static str,
    pub addr: Address,
    /// Size in bytes.
    pub size: u32,
}

/// Default value as given by the editor (patch names default to text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Init {
    Num(i32),
    Text(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct ParamDef {
    /// e.g. `Temp_patch.fxItem(1).Switch`
    pub path: &'static str,
    pub desc: &'static str,
    pub addr: Address,
    pub encoding: Encoding,
    pub min: i32,
    pub max: i32,
    pub init: Init,
    pub ofs: i32,
    pub factor: i32,
}

impl ParamDef {
    pub fn spec(&self) -> ValueSpec {
        ValueSpec { encoding: self.encoding, min: self.min, max: self.max, ofs: self.ofs }
    }

    /// Value labels, when the official editor shows this parameter as a list.
    pub fn labels(&self) -> Option<&'static ParamValues> {
        param_values(self.path)
    }

    /// Human-readable value: its label when the parameter is a list.
    pub fn display(&self, value: i32) -> String {
        if let Some(l) = self.labels().and_then(|v| v.label(value)) {
            return l.to_string();
        }
        if self.factor > 1 {
            let v = value as f64 / self.factor as f64;
            return if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
        }
        value.to_string()
    }
}

/// The labels of a list parameter: `labels[i]` is the label of value `first + i`.
#[derive(Debug, Clone, Copy)]
pub struct ParamValues {
    pub path: &'static str,
    pub first: i32,
    pub labels: &'static [&'static str],
}

impl ParamValues {
    pub fn label(&self, value: i32) -> Option<&'static str> {
        usize::try_from(value - self.first).ok().and_then(|i| self.labels.get(i)).copied()
    }

    /// (value, label) pairs, for building a picker.
    pub fn options(&self) -> impl Iterator<Item = (i32, &'static str)> + '_ {
        self.labels.iter().enumerate().map(|(i, l)| (self.first + i as i32, *l))
    }
}

/// Which field of an FX slot an [`FxParamDef`] lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FxField {
    Type,
    Switch,
    DuplicationNumber,
    /// The generic `param(n)` slot, reinterpreted per FX type.
    Param(u8),
}

#[derive(Debug, Clone, Copy)]
pub struct FxParamDef {
    pub name: &'static str,
    /// Stable identifier, e.g. `drive`.
    pub unique_name: &'static str,
    pub field: FxField,
    /// Offset inside the FX slot.
    pub offset: Address,
    pub encoding: Encoding,
    pub min: i32,
    pub max: i32,
    pub init: Init,
    pub ofs: i32,
    /// Display divisor (e.g. BPM is stored ×10).
    pub factor: i32,
    /// Value labels, when the parameter is a list.
    pub values: &'static [&'static str],
    /// The editor's JS display expression, kept for reference.
    pub format: Option<&'static str>,
    pub template: &'static str,
}

impl FxParamDef {
    pub fn spec(&self) -> ValueSpec {
        ValueSpec { encoding: self.encoding, min: self.min, max: self.max, ofs: self.ofs }
    }

    /// Human-readable value: its label when the parameter is a list.
    pub fn display(&self, value: i32) -> String {
        if !self.values.is_empty() {
            let spans_range = self.values.len() as i64 == (self.max as i64 - self.min as i64 + 1);
            let idx = if spans_range { value - self.min } else { value };
            if let Some(label) = usize::try_from(idx).ok().and_then(|i| self.values.get(i)) {
                return (*label).to_string();
            }
        }
        if self.factor > 1 {
            let v = value as f64 / self.factor as f64;
            return if v.fract() == 0.0 { format!("{v:.0}") } else { format!("{v}") };
        }
        value.to_string()
    }

    /// Parses a value typed by a user: a label (case-insensitive) or a number.
    pub fn parse_value(&self, s: &str) -> Option<i32> {
        if let Some(i) = self.values.iter().position(|v| v.eq_ignore_ascii_case(s.trim())) {
            let spans_range = self.values.len() as i64 == (self.max as i64 - self.min as i64 + 1);
            return Some(if spans_range { self.min + i as i32 } else { i as i32 });
        }
        s.trim().parse().ok()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct FxTypeDef {
    /// The value written to `fxItem(n).Type`.
    pub id: u8,
    pub name: &'static str,
    pub params: &'static [FxParamDef],
}

impl FxTypeDef {
    pub fn param(&self, unique_name: &str) -> Option<&'static FxParamDef> {
        self.params.iter().find(|p| p.unique_name.eq_ignore_ascii_case(unique_name))
    }
}

/// A group of assign targets: one per FX type (matched by `block_name`), plus MASTER, TUNER, MIDI and "----".
#[derive(Debug, Clone, Copy)]
pub struct AssignCategoryDef {
    pub name: &'static str,
    /// Its targets start with ON/OFF (the effect's switch).
    pub has_switch: bool,
    /// The FX type (or MASTER/TUNER/MIDI) the category belongs to.
    pub block_name: &'static str,
}

/// What an assign can move. The value of `assign(n).TARGET` indexes [`ASSIGN_TARGETS`].
#[derive(Debug, Clone, Copy)]
pub struct AssignTargetDef {
    pub name: &'static str,
    /// Index in [`ASSIGN_CATEGORIES`]; -1 for ON/OFF, which every effect with a switch shares.
    pub category: i16,
    /// A parameter of the FX in slot `TARGET_FX_ITEM`; otherwise of MASTER, TUNER or MIDI.
    pub effect: bool,
    /// The [`FxParamDef::unique_name`] it moves.
    pub unique_name: &'static str,
}

/// Text from the GX-100 parameter guide about one setting: what it does, and per list option.
/// Fetched by `tools/extract/manual.mjs` into the (local-only) generated tables; empty without it.
#[derive(Debug, Clone, Copy)]
pub struct ParamDoc {
    /// FX docs: the parameter's `unique_name`. [`PARAM_DOCS`]: the parameter path, with numbers in
    /// parentheses replaced by `n` (`Temp_patch.assign(n).SOURCE`).
    pub key: &'static str,
    pub text: &'static str,
    /// (option label as the pedal shows it, explanation)
    pub options: &'static [(&'static str, &'static str)],
}

impl ParamDoc {
    pub fn option(&self, label: &str) -> Option<&'static str> {
        let norm = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_uppercase();
        let l = norm(label);
        self.options.iter().find(|(o, _)| norm(o) == l).map(|(_, t)| *t)
    }
}

/// The guide's text about an FX type (what it does, what it models) and its parameters.
#[derive(Debug, Clone, Copy)]
pub struct FxDoc {
    pub fx: &'static str,
    pub text: &'static str,
    pub params: &'static [ParamDoc],
}

impl FxDoc {
    pub fn param(&self, unique_name: &str) -> Option<&'static ParamDoc> {
        self.params.iter().find(|p| p.key == unique_name)
    }
}

mod load;

pub use load::from_json;

/// Every table, as loaded from the extractor's output (see [`install`]).
#[derive(Debug)]
pub struct Tables {
    pub source: TablesSource,
    pub product: ProductDef,
    /// Special command addresses (sorted by name).
    pub commands: &'static [NamedAddress],
    /// Fixed addresses the editor uses directly (sorted by name).
    pub addresses: &'static [NamedAddress],
    /// Top-level memory areas, in address order.
    pub root: &'static [RootDef],
    /// Every System / PC-map / Setup / Temp-patch parameter, sorted by path.
    pub params: &'static [ParamDef],
    /// Value labels of list parameters (sorted by path).
    pub param_values: &'static [ParamValues],
    /// FX types, indexed by the value of `fxItem(n).Type`.
    pub fx_types: &'static [FxTypeDef],
    /// The patch's MASTER settings (level, BPM, key…) as assign targets see them. Offsets are meaningless.
    pub master: FxTypeDef,
    /// Assign target categories, indexed by [`AssignTargetDef::category`].
    pub assign_categories: &'static [AssignCategoryDef],
    /// Assign targets, indexed by the value of `assign(n).TARGET`.
    pub assign_targets: &'static [AssignTargetDef],
    /// The parameter guide's text about each FX type and its parameters (empty without it).
    pub fx_docs: &'static [FxDoc],
    /// The guide's text about patch and system settings, by generalized path (sorted).
    pub param_docs: &'static [ParamDoc],
}

/// The synthetic tables in `fixtures/` (made-up effects and text), for tests and as a last resort.
pub fn fixture() -> Tables {
    from_json(include_str!("../fixtures/tables.json"), Some(include_str!("../fixtures/docs.json")))
        .expect("fixture tables")
}

static TABLES: std::sync::OnceLock<&'static Tables> = std::sync::OnceLock::new();

/// Makes `t` the tables every lookup uses. Call it once, before anything reads a table; afterwards
/// (or when a lookup already fell back to the defaults) it returns `t` back.
pub fn install(t: Tables) -> Result<(), Box<Tables>> {
    let mut t = Some(Box::new(t));
    TABLES.get_or_init(|| Box::leak(t.take().expect("first")));
    t.map_or(Ok(()), Err)
}

/// Whether tables have been installed (or a lookup already fell back to the defaults).
pub fn installed() -> bool {
    TABLES.get().is_some()
}

/// The tables in use. Without [`install`]: the file named by `$OPENGX100_TABLES` (a `tables.json`, with
/// an optional `docs.json` beside it), else the synthetic fixture.
pub fn get() -> &'static Tables {
    TABLES.get_or_init(|| Box::leak(Box::new(default_tables())))
}

fn default_tables() -> Tables {
    let Some(path) = std::env::var_os("OPENGX100_TABLES") else { return fixture() };
    load_file(std::path::Path::new(&path)).unwrap_or_else(|e| panic!("OPENGX100_TABLES: {e}"))
}

/// Loads a `tables.json` written by the extractor, plus `docs.json` from the same folder if present.
pub fn load_file(path: &std::path::Path) -> Result<Tables, String> {
    let bundle = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let docs = path.parent().map(|d| d.join("docs.json")).and_then(|d| std::fs::read_to_string(d).ok());
    from_json(&bundle, docs.as_deref())
}

/// A view into the installed tables that reads like the table itself (`PARAMS.iter()`, `MASTER.param(…)`).
pub struct Table<T: ?Sized + 'static>(fn(&'static Tables) -> &'static T);

impl<T: ?Sized + 'static> Clone for Table<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized + 'static> Copy for Table<T> {}

impl<T: 'static> IntoIterator for Table<[T]> {
    type Item = &'static T;
    type IntoIter = std::slice::Iter<'static, T>;
    fn into_iter(self) -> Self::IntoIter {
        (self.0)(get()).iter()
    }
}

impl<T: ?Sized + 'static> std::ops::Deref for Table<T> {
    type Target = T;
    fn deref(&self) -> &'static T {
        (self.0)(get())
    }
}

pub static SOURCE: Table<TablesSource> = Table(|t| &t.source);
pub static PRODUCT: Table<ProductDef> = Table(|t| &t.product);
pub static COMMANDS: Table<[NamedAddress]> = Table(|t| t.commands);
pub static ADDRESSES: Table<[NamedAddress]> = Table(|t| t.addresses);
pub static ROOT: Table<[RootDef]> = Table(|t| t.root);
pub static PARAMS: Table<[ParamDef]> = Table(|t| t.params);
pub static PARAM_VALUES: Table<[ParamValues]> = Table(|t| t.param_values);
pub static FX_TYPES: Table<[FxTypeDef]> = Table(|t| t.fx_types);
pub static MASTER: Table<FxTypeDef> = Table(|t| &t.master);
pub static ASSIGN_CATEGORIES: Table<[AssignCategoryDef]> = Table(|t| t.assign_categories);
pub static ASSIGN_TARGETS: Table<[AssignTargetDef]> = Table(|t| t.assign_targets);
pub static FX_DOCS: Table<[FxDoc]> = Table(|t| t.fx_docs);
pub static PARAM_DOCS: Table<[ParamDoc]> = Table(|t| t.param_docs);

/// Looks up a parameter by its full path (e.g. `Temp_patch.common.Name`).
pub fn param(path: &str) -> Option<&'static ParamDef> {
    PARAMS.binary_search_by(|p| p.path.cmp(path)).ok().map(|i| &PARAMS[i])
}

/// Value labels of the parameter at `path`, if it is a list.
pub fn param_values(path: &str) -> Option<&'static ParamValues> {
    PARAM_VALUES.binary_search_by(|v| v.path.cmp(path)).ok().map(|i| &PARAM_VALUES[i])
}

/// The parameter starting exactly at `addr`, if any.
pub fn param_at(addr: Address) -> Option<&'static ParamDef> {
    PARAMS.iter().find(|p| p.addr == addr)
}

/// Case-insensitive search over parameter paths and descriptions.
pub fn search_params(query: &str) -> impl Iterator<Item = &'static ParamDef> + '_ {
    let q = query.to_ascii_lowercase();
    PARAMS.iter().filter(move |p| p.path.to_ascii_lowercase().contains(&q) || p.desc.to_ascii_lowercase().contains(&q))
}

fn named(list: &'static [NamedAddress], name: &str) -> Option<Address> {
    list.binary_search_by(|n| n.name.cmp(name)).ok().map(|i| list[i].addr)
}

/// A special command address (e.g. `PATCH_WRITE`).
pub fn command(name: &str) -> Option<Address> {
    named(&COMMANDS, name)
}

/// A fixed address the editor uses directly (e.g. `CHAIN`).
pub fn address(name: &str) -> Option<Address> {
    named(&ADDRESSES, name)
}

pub fn root(name: &str) -> Option<&'static RootDef> {
    ROOT.iter().find(|r| r.name == name)
}

pub fn fx_type(id: u8) -> Option<&'static FxTypeDef> {
    FX_TYPES.get(id as usize)
}

pub fn fx_type_by_name(name: &str) -> Option<&'static FxTypeDef> {
    FX_TYPES.iter().find(|t| t.name.eq_ignore_ascii_case(name))
}

/// The guide's text about an FX type (`MASTER` for the patch's master settings).
pub fn fx_doc(fx_name: &str) -> Option<&'static FxDoc> {
    FX_DOCS.iter().find(|d| d.fx.eq_ignore_ascii_case(fx_name))
}

/// The guide's text about a parameter, by path (`Temp_patch.assign(3).SOURCE`).
pub fn param_doc(path: &str) -> Option<&'static ParamDoc> {
    let key = doc_key(path);
    PARAM_DOCS.binary_search_by(|d| d.key.cmp(&key)).ok().map(|i| &PARAM_DOCS[i])
}

/// `Temp_patch.assign(3).SOURCE` → `Temp_patch.assign(n).SOURCE`.
pub fn doc_key(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while let Some(i) = rest.find('(') {
        let (head, tail) = rest.split_at(i + 1);
        out.push_str(head);
        match tail.find(')') {
            Some(j) if j > 0 && tail[..j].bytes().all(|b| b.is_ascii_digit()) => {
                out.push('n');
                rest = &tail[j..];
            }
            _ => rest = tail,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_sorted_for_binary_search() {
        assert!(PARAMS.windows(2).all(|w| w[0].path < w[1].path));
        assert!(COMMANDS.windows(2).all(|w| w[0].name < w[1].name));
        assert!(ADDRESSES.windows(2).all(|w| w[0].name < w[1].name));
        assert!(PARAM_VALUES.windows(2).all(|w| w[0].path < w[1].path));
        assert!(PARAM_DOCS.windows(2).all(|w| w[0].key < w[1].key));
    }

    #[test]
    fn fx_type_ids_match_positions() {
        assert!(FX_TYPES.iter().enumerate().all(|(i, t)| t.id as usize == i));
    }

    #[test]
    fn core_lookups() {
        let sw = param("Temp_patch.fxItem(1).Switch").expect("fx slot switch");
        assert_eq!(sw.addr.to_string(), "10 00 11 01");
        assert_eq!(param_at(sw.addr).map(|p| p.path), Some(sw.path));
        assert_eq!(command("EDITOR_COMMUNICATION_MODE").unwrap().to_string(), "7F 00 00 01");
        assert_eq!(root("Temp_patch").unwrap().addr.to_string(), "10 00 00 00");
        assert_eq!(PRODUCT.model_id, crate::sysex::MODEL_ID);
        assert!(param("nope").is_none());
    }

    #[test]
    fn value_labels_match_their_parameters() {
        for v in PARAM_VALUES {
            let p = param(v.path).unwrap_or_else(|| panic!("labels for unknown parameter {}", v.path));
            assert_eq!(v.first, p.min, "{}", v.path);
            assert_eq!(v.labels.len() as i32, p.max - p.min + 1, "{}", v.path);
            assert_eq!(p.display(p.min), v.labels[0]);
        }
        let level = param("Temp_patch.common.Name").unwrap();
        assert!(level.labels().is_none());
    }

    #[test]
    fn doc_keys_generalize_instance_numbers() {
        assert_eq!(doc_key("Temp_patch.assign(12).SOURCE"), "Temp_patch.assign(n).SOURCE");
        assert_eq!(doc_key("System_inputSetting(3)._Name(1)"), "System_inputSetting(n)._Name(n)");
        assert_eq!(doc_key("System_common.AutoOff"), "System_common.AutoOff");
        assert_eq!(doc_key("odd(x).y("), "odd(x).y(");
        for d in PARAM_DOCS {
            assert_eq!(doc_key(d.key), d.key, "doc keys are already generalized");
        }
    }

    #[test]
    fn assign_targets_point_at_categories() {
        for t in ASSIGN_TARGETS {
            assert!(t.category == -1 || (t.category as usize) < ASSIGN_CATEGORIES.len(), "{}", t.name);
        }
        let named = |n: &str| ASSIGN_CATEGORIES.iter().any(|c| c.block_name == n);
        assert!(named("MASTER") && named("TUNER") && named("MIDI") && named("----"));
    }

    #[test]
    fn fixture_specific() {
        if *SOURCE != TablesSource::Fixture {
            return;
        }
        let alpha = fx_type_by_name("fx alpha").unwrap();
        let mode = alpha.param("MODE").unwrap();
        assert_eq!(mode.field, FxField::Param(0));
        assert_eq!(mode.display(2), "HARD");
        assert_eq!(mode.parse_value("mid"), Some(1));
        let rate = fx_type(1).unwrap().param("rate").unwrap();
        assert_eq!(rate.display(1200), "120");
        assert_eq!(rate.display(1205), "120.5");
        assert!(fx_type(2).unwrap().params.is_empty());
        assert_eq!(param("Temp_patch.common.Name").unwrap().init, Init::Text("INIT MEMORY     "));
        assert_eq!(param("Temp_patch.common.Level").unwrap().init, Init::Num(100));
    }
}
