//! Gets the GX-100 parameter tables from the user's own BOSS TONE STUDIO, so that nothing of Roland's
//! ships with the editor. At first launch the official editor's configuration scripts are run once in
//! an embedded JavaScript engine (boa) by `tools/extract/core.js`, the same code the development tool
//! (`tools/extract/extract.mjs`) runs under Node; the result is cached and re-extracted when BOSS TONE
//! STUDIO changes version.

use boa_engine::{js_string, Context, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction, Source};
use gx100_protocol::tables::{self, Tables};
use std::fmt;
use std::path::{Path, PathBuf};

/// Where the official installer puts it on macOS.
pub const DEFAULT_APP: &str = "/Applications/BOSS/GX-100/BOSS TONE STUDIO for GX-100.app";
const APP_NAME: &str = "BOSS TONE STUDIO for GX-100.app";

/// The extraction script, shared with the Node tool.
const CORE_JS: &str = include_str!("../../../tools/extract/core.js");

/// The official editor's files `core.js` reads, relative to `Contents/Resources/html`.
const FILES: &[&str] = &[
    "js/utilities/constant.js",
    "js/utilities/converter.js",
    "js/config/product_setting.js",
    "js/config/address_map.js",
    "js/businesslogic/address_const.js",
    "js/config/resource.js",
    "bg777-pc.elf",
    "js/businesslogic/chain/chain_controller.js",
    "js/config/effect_parameter.js",
    "js/businesslogic/numpad_const.js",
    "js/businesslogic/assign_const.js",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// BOSS TONE STUDIO for GX-100 isn't installed where we looked (and nothing is cached).
    AppNotFound,
    /// The path given isn't a BOSS TONE STUDIO for GX-100 app.
    NotTheApp(PathBuf),
    /// Reading its files or running its scripts failed (e.g. an update changed them).
    Extract(String),
    /// The tables loaded but are unusable, or the cache couldn't be written.
    Tables(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::AppNotFound => write!(f, "BOSS TONE STUDIO for GX-100 isn't installed (looked in /Applications)"),
            Error::NotTheApp(p) => write!(f, "{} isn't BOSS TONE STUDIO for GX-100", p.display()),
            Error::Extract(e) => write!(f, "reading BOSS TONE STUDIO's tables failed: {e}"),
            Error::Tables(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

fn html(app: &Path) -> PathBuf {
    app.join("Contents/Resources/html")
}

/// Whether `app` looks like BOSS TONE STUDIO for GX-100 (has the files we read).
pub fn is_app(app: &Path) -> bool {
    FILES.iter().all(|f| html(app).join(f).is_file())
}

/// The installed BOSS TONE STUDIO for GX-100: the default location, else anywhere a couple of levels
/// below /Applications or ~/Applications.
pub fn find_app() -> Option<PathBuf> {
    let default = PathBuf::from(DEFAULT_APP);
    if is_app(&default) {
        return Some(default);
    }
    let mut dirs: Vec<PathBuf> = vec!["/Applications".into()];
    dirs.extend(dirs::home_dir().map(|h| h.join("Applications")));
    for depth in 0..3 {
        let mut next = Vec::new();
        for d in &dirs {
            let Ok(entries) = std::fs::read_dir(d) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.file_name().is_some_and(|n| n == APP_NAME) && is_app(&p) {
                    return Some(p);
                }
                // Don't descend into other apps' bundles.
                if depth < 2 && p.is_dir() && p.extension().is_none_or(|x| x != "app") {
                    next.push(p);
                }
            }
        }
        dirs = next;
    }
    None
}

/// `key: 'value'` from the official editor's product settings (without running them).
fn pick(src: &str, key: &str) -> Option<String> {
    let mut rest = src;
    while let Some(i) = rest.find(key) {
        let before = rest[..i].chars().next_back();
        let after = rest[i + key.len()..].trim_start();
        rest = &rest[i + key.len()..];
        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let Some(v) = after.strip_prefix(':') else { continue };
        let v = v.trim_start().trim_start_matches('\'');
        let end = v.find(|c: char| c == '\'' || c == ',' || c.is_whitespace()).unwrap_or(v.len());
        if end > 0 {
            return Some(v[..end].to_string());
        }
    }
    None
}

/// BOSS TONE STUDIO's version and build ("2.0.1", "118"), read from its product settings.
pub fn app_version(app: &Path) -> Option<(String, String)> {
    let src = std::fs::read_to_string(html(app).join("js/config/product_setting.js")).ok()?;
    Some((pick(&src, "version")?, pick(&src, "build")?))
}

fn js_err(msg: String) -> boa_engine::JsError {
    JsNativeError::error().with_message(msg).into()
}

/// Runs the official editor's configuration scripts and returns the tables as JSON (the format
/// [`tables::from_json`] reads). Takes a few seconds.
pub fn extract(app: &Path) -> Result<String, Error> {
    if !is_app(app) {
        return Err(Error::NotTheApp(app.to_path_buf()));
    }
    let mut ctx = Context::default();
    let fail = |e: boa_engine::JsError| Error::Extract(e.to_string());

    // The files, as one object of strings: core.js asks for them by name.
    let files = JsObject::with_null_proto();
    for f in FILES {
        let text = std::fs::read_to_string(html(app).join(f)).map_err(|e| Error::Extract(format!("{f}: {e}")))?;
        files.set(JsString::from(*f), JsString::from(text.as_str()), false, &mut ctx).map_err(fail)?;
    }
    ctx.register_global_property(js_string!("__files"), files, boa_engine::property::Attribute::all()).map_err(fail)?;
    // Runs one of the official editor's scripts in the shared global scope.
    let run = NativeFunction::from_copy_closure(|_, args, ctx| -> JsResult<JsValue> {
        let src = args.first().cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped();
        ctx.eval(Source::from_bytes(src.as_bytes()))
    });
    ctx.register_global_builtin_callable(js_string!("__run"), 2, run).map_err(fail)?;

    let script = format!(
        "{CORE_JS}\n;(() => {{\n\
           globalThis.console = {{ log() {{}}, warn() {{}}, error() {{}}, info() {{}}, debug() {{}} }};\n\
           const read = (rel) => {{\n\
             if (!(rel in __files)) throw new Error('core.js asked for an unlisted file: ' + rel);\n\
             return __files[rel];\n\
           }};\n\
           const t = extractTables({{ read, run: __run, global: globalThis, log() {{}} }});\n\
           return JSON.stringify(tablesBundle(t));\n\
         }})()"
    );
    let out = ctx.eval(Source::from_bytes(script.as_bytes())).map_err(fail)?;
    let json = out.as_string().ok_or_else(|| js_err("no result".into())).map_err(fail)?;
    Ok(json.to_std_string_escaped())
}

/// Where extracted tables are kept: `<data dir>/gx100-editor/tables/` (on macOS
/// `~/Library/Application Support/gx100-editor/tables/`). A `docs.json` (parameter guide text, from
/// `tools/extract/manual.mjs`) placed there is loaded too.
pub fn cache_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("gx100-editor").join("tables"))
}

fn cache_file() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("tables.json"))
}

/// Where the tables came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// `$GX100_TABLES`.
    Env(PathBuf),
    /// The cache, still matching the installed BOSS TONE STUDIO (or it's no longer installed).
    Cache(PathBuf),
    /// Freshly extracted from this app (and cached).
    Extracted(PathBuf),
}

/// The tables to use: `$GX100_TABLES` if set; else the cache, if it matches the installed BOSS TONE
/// STUDIO (`app`, or [`find_app`]); else extracted from it now and cached. Doesn't install them.
pub fn prepare(app: Option<&Path>) -> Result<(Tables, Origin), Error> {
    if let Some(p) = std::env::var_os("GX100_TABLES").map(PathBuf::from) {
        return tables::load_file(&p).map(|t| (t, Origin::Env(p))).map_err(Error::Tables);
    }
    let app = app.map(Path::to_path_buf).or_else(find_app);
    let cached = cache_file().filter(|f| f.is_file()).and_then(|f| tables::load_file(&f).ok().map(|t| (t, f)));
    match (app, cached) {
        (None, Some((t, f))) => Ok((t, Origin::Cache(f))),
        (None, None) => Err(Error::AppNotFound),
        (Some(app), cached) => {
            let version = app_version(&app);
            if let Some((t, f)) = cached {
                let same =
                    version.as_ref().is_some_and(|(v, b)| t.product.bts_version == v && t.product.bts_build == b);
                if same {
                    return Ok((t, Origin::Cache(f)));
                }
            }
            let json = extract(&app)?;
            let t = tables::from_json(&json, None).map_err(Error::Tables)?;
            let docs = cache_dir().map(|d| d.join("docs.json")).and_then(|d| std::fs::read_to_string(d).ok());
            let t = match docs {
                Some(d) => tables::from_json(&json, Some(&d)).unwrap_or(t),
                None => t,
            };
            if let Some(f) = cache_file() {
                write_atomic(&f, &json).map_err(|e| Error::Tables(format!("saving {}: {e}", f.display())))?;
            }
            Ok((t, Origin::Extracted(app)))
        }
    }
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_product_settings() {
        let src = "var ProductSetting = {\n  name: 'GX-100',\n  version: '2.0.1', build: '118',\n  subversion: 'x' };";
        assert_eq!(pick(src, "version").as_deref(), Some("2.0.1"));
        assert_eq!(pick(src, "build").as_deref(), Some("118"));
        assert_eq!(pick(src, "name").as_deref(), Some("GX-100"));
        assert_eq!(pick(src, "missing"), None);
    }

    /// Runs the real extraction when BOSS TONE STUDIO is installed and compares it with the Node
    /// tool's output (`node tools/extract/extract.mjs` → extracted/tables.json), when present.
    #[test]
    #[ignore = "needs BOSS TONE STUDIO installed; slow"]
    fn matches_the_node_tool() {
        let app = find_app().expect("BOSS TONE STUDIO installed");
        let t0 = std::time::Instant::now();
        let json = extract(&app).expect("extract");
        eprintln!("extracted in {:?}", t0.elapsed());
        let t = tables::from_json(&json, None).expect("tables");
        assert!(t.params.len() > 2000 && t.fx_types.len() > 50);
        let node = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../extracted/tables.json");
        if let Ok(n) = std::fs::read_to_string(node) {
            let parse = |s: &str| gx100_protocol::json::Json::parse(s).unwrap();
            assert!(parse(&json) == parse(&n), "boa and Node disagree");
            eprintln!("identical to the Node tool's output");
        }
    }
}
