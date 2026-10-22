//! Setlists and backups on disk: JSON files under the app's data folder
//! (macOS: `~/Library/Application Support/OpenGX100 Editor`). Patch data is stored as hex.
//!
//! Backups made by BOSS TONE STUDIO (`.alb` files from its "backup all") are read too: same data
//! (system areas + 200 patches), stored per block as arrays of hex strings.
//!
//! Every write goes to a temporary file first and is then renamed over the old one,
//! so a crash mid-save never leaves a half-written setlist or backup.

use opengx100_protocol::backup::SYSTEM_AREAS;
use opengx100_protocol::patch::{patch_blocks, patch_span, PatchData};
use opengx100_protocol::tables;
use opengx100_protocol::Address;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const SETLIST_FORMAT: &str = "OpenGX100 Editor setlist";
const BACKUP_FORMAT: &str = "OpenGX100 Editor backup";
/// What files written before the project was renamed say; still read.
const OLD_SETLIST_FORMAT: &str = "gx100-editor setlist";
const OLD_BACKUP_FORMAT: &str = "gx100-editor backup";

/// Seconds since the Unix epoch.
pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `2026-10-19 22:14`, local time.
pub fn fmt_time(t: i64) -> String {
    chrono::DateTime::from_timestamp(t, 0)
        .map(|d| d.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "?".into())
}

// ------------------------------------------------------------------ in memory

/// An ordered list of patches to write into a range of the pedal. Each entry keeps a copy of
/// its patch, so the setlist still means the same thing after the pedal's patches change.
#[derive(Debug, Clone, PartialEq)]
pub struct Setlist {
    pub id: String,
    pub name: String,
    pub created: i64,
    pub modified: i64,
    /// First user patch of the range to write into.
    pub target_start: u16,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The user patch it was taken from (1-based).
    pub source: u16,
    pub name: String,
    /// The patch as it was when added; `None` until it has been read from the pedal.
    pub data: Option<PatchData>,
}

impl Setlist {
    pub fn new(name: String) -> Self {
        let t = now();
        let id = format!("setlist-{}", chrono::Utc::now().timestamp_millis());
        Setlist { id, name, created: t, modified: t, target_start: 1, entries: Vec::new() }
    }
}

/// Everything a full backup holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub origin: Origin,
    pub created: i64,
    /// The BOSS TONE STUDIO version the parameter tables came from.
    pub bts_version: String,
    pub system: Vec<Area>,
    /// (patch number, name, data), patches 1 to 200.
    pub patches: Vec<(u16, String, PatchData)>,
}

/// Which program made a backup file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// This editor (`.json`).
    Editor,
    /// BOSS TONE STUDIO's "backup all" (`.alb`).
    BossToneStudio,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Area {
    pub name: String,
    pub addr: Address,
    pub data: Vec<u8>,
}

/// A backup file, without loading it.
#[derive(Debug, Clone, PartialEq)]
pub struct BackupInfo {
    pub path: PathBuf,
    pub created: i64,
    pub bytes: u64,
}

// ------------------------------------------------------------------ on disk

#[derive(Serialize, Deserialize)]
struct SetlistFile {
    format: String,
    version: u32,
    id: String,
    name: String,
    created: i64,
    modified: i64,
    target_start: u16,
    entries: Vec<EntryFile>,
}

#[derive(Serialize, Deserialize)]
struct EntryFile {
    source: u16,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct BackupFile {
    format: String,
    version: u32,
    created: i64,
    bts_version: String,
    system: Vec<AreaFile>,
    patches: Vec<PatchFile>,
}

#[derive(Serialize, Deserialize)]
struct AreaFile {
    name: String,
    addr: String,
    data: String,
}

#[derive(Serialize, Deserialize)]
struct PatchFile {
    number: u16,
    name: String,
    data: String,
}

/// BOSS TONE STUDIO's backup file (`.alb`): `{"system": {area: ["0B", …]}, "userPatch": [{"User_patch%common": […], …}],
/// "formatRev": "0000", "device": "GX-100"}`.
#[derive(Deserialize)]
struct AlbFile {
    device: String,
    system: HashMap<String, Vec<String>>,
    #[serde(rename = "userPatch")]
    user_patch: Vec<HashMap<String, Vec<String>>>,
}

fn alb_bytes(what: &str, hex: &[String]) -> io::Result<Vec<u8>> {
    hex.iter().map(|h| u8::from_str_radix(h, 16).map_err(|_| invalid(format!("{what}: bad byte {h:?}")))).collect()
}

/// Converts a BOSS TONE STUDIO backup. Strict: an unknown area or block, a missing block or a size that
/// doesn't fit is an error, so nothing is ever misplaced.
fn backup_from_alb(f: AlbFile, created: i64) -> io::Result<Backup> {
    if f.device != "GX-100" {
        return Err(invalid(format!("this backup is for a {}, not a GX-100", f.device)));
    }
    if let Some(unknown) = f.system.keys().find(|k| !SYSTEM_AREAS.contains(&k.as_str())) {
        return Err(invalid(format!("unknown system area {unknown:?}")));
    }
    let mut system = Vec::new();
    for name in SYSTEM_AREAS {
        let Some(hex) = f.system.get(*name) else { continue };
        let root = tables::root(name).ok_or_else(|| invalid(format!("system area {name} isn't in the tables")))?;
        let data = alb_bytes(name, hex)?;
        if data.len() != root.size as usize {
            return Err(invalid(format!("system area {name} has {} bytes, expected {}", data.len(), root.size)));
        }
        system.push(Area { name: name.to_string(), addr: root.addr, data });
    }
    let mut patches = Vec::new();
    for (i, blocks) in f.user_patch.into_iter().enumerate() {
        let n = i as u16 + 1;
        let mut bytes = vec![0u8; patch_span() as usize];
        for (key, hex) in &blocks {
            let name =
                key.strip_prefix("User_patch%").ok_or_else(|| invalid(format!("patch {n}: odd block {key:?}")))?;
            let b = patch_blocks()
                .iter()
                .find(|b| b.name == name)
                .ok_or_else(|| invalid(format!("patch {n}: unknown block {key:?}")))?;
            let data = alb_bytes(key, hex)?;
            if data.len() > b.size as usize {
                return Err(invalid(format!("patch {n} {key}: {} bytes, the block holds {}", data.len(), b.size)));
            }
            bytes[b.offset as usize..b.offset as usize + data.len()].copy_from_slice(&data);
        }
        if let Some(missing) = patch_blocks().iter().find(|b| !blocks.contains_key(&format!("User_patch%{}", b.name))) {
            return Err(invalid(format!("patch {n} has no {} block", missing.name)));
        }
        let data = PatchData::from_bytes(bytes);
        patches.push((n, data.name(), data));
    }
    Ok(Backup { origin: Origin::BossToneStudio, created, bts_version: "BOSS TONE STUDIO".into(), system, patches })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> io::Result<Vec<u8>> {
    let bad = || io::Error::new(io::ErrorKind::InvalidData, "bad hex data");
    if !s.len().is_multiple_of(2) {
        return Err(bad());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2).ok_or_else(bad)?, 16).map_err(|_| bad()))
        .collect()
}

fn patch_from_hex(s: &str) -> io::Result<PatchData> {
    let bytes = unhex(s)?;
    if bytes.len() != patch_span() as usize {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("patch data is {} bytes, this version expects {}", bytes.len(), patch_span()),
        ));
    }
    Ok(PatchData::from_bytes(bytes))
}

fn invalid(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e.to_string())
}

/// Writes `contents` to `path` without ever leaving a partial file behind.
fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    /// The per-user data folder (created if needed).
    pub fn open_default() -> io::Result<Store> {
        Store::at(opengx100_extract::data_dir().ok_or_else(|| io::Error::other("no data folder on this system"))?)
    }

    pub fn at(root: PathBuf) -> io::Result<Store> {
        let store = Store { root };
        fs::create_dir_all(store.setlists_dir())?;
        fs::create_dir_all(store.backups_dir())?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn setlists_dir(&self) -> PathBuf {
        self.root.join("setlists")
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    fn files_with(dir: &Path, extensions: &[&str]) -> Vec<PathBuf> {
        let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
        rd.filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|x| x.to_str()).is_some_and(|x| extensions.contains(&x)))
            .collect()
    }

    // -------------------------------------------------------------- setlists

    /// Every saved setlist, most recently changed first, plus a message per file that couldn't be read.
    pub fn load_setlists(&self) -> (Vec<Setlist>, Vec<String>) {
        let mut lists = Vec::new();
        let mut errors = Vec::new();
        for path in Self::files_with(&self.setlists_dir(), &["json"]) {
            match Self::read_setlist(&path) {
                Ok(s) => lists.push(s),
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        }
        lists.sort_by_key(|s: &Setlist| std::cmp::Reverse(s.modified));
        (lists, errors)
    }

    fn read_setlist(path: &Path) -> io::Result<Setlist> {
        let f: SetlistFile = serde_json::from_slice(&fs::read(path)?).map_err(invalid)?;
        if f.format != SETLIST_FORMAT && f.format != OLD_SETLIST_FORMAT {
            return Err(invalid("not a setlist file"));
        }
        let entries = f
            .entries
            .into_iter()
            .map(|e| {
                Ok(Entry { source: e.source, name: e.name, data: e.data.as_deref().map(patch_from_hex).transpose()? })
            })
            .collect::<io::Result<_>>()?;
        Ok(Setlist {
            id: f.id,
            name: f.name,
            created: f.created,
            modified: f.modified,
            target_start: f.target_start,
            entries,
        })
    }

    pub fn save_setlist(&self, s: &Setlist) -> io::Result<()> {
        write_atomic(&self.setlists_dir().join(format!("{}.json", s.id)), &Self::setlist_json(s)?)
    }

    /// Writes a setlist to any file (same format as the saved ones), e.g. to share or keep elsewhere.
    pub fn export_setlist(s: &Setlist, path: &Path) -> io::Result<()> {
        fs::write(path, Self::setlist_json(s)?)
    }

    /// Reads a setlist file made by [`Self::export_setlist`]. It gets a new id, so importing the same
    /// file twice, or a copy of a setlist that's already here, never overwrites anything.
    pub fn import_setlist(path: &Path) -> io::Result<Setlist> {
        let mut s = Self::read_setlist(path)?;
        let fresh = Setlist::new(s.name.clone());
        s.id = fresh.id;
        s.modified = fresh.modified;
        Ok(s)
    }

    fn setlist_json(s: &Setlist) -> io::Result<Vec<u8>> {
        let f = SetlistFile {
            format: SETLIST_FORMAT.into(),
            version: 1,
            id: s.id.clone(),
            name: s.name.clone(),
            created: s.created,
            modified: s.modified,
            target_start: s.target_start,
            entries: s
                .entries
                .iter()
                .map(|e| EntryFile {
                    source: e.source,
                    name: e.name.clone(),
                    data: e.data.as_ref().map(|d| hex(d.bytes())),
                })
                .collect(),
        };
        Ok(serde_json::to_vec_pretty(&f)?)
    }

    pub fn delete_setlist(&self, id: &str) -> io::Result<()> {
        fs::remove_file(self.setlists_dir().join(format!("{id}.json")))
    }

    // -------------------------------------------------------------- backups

    /// Backup files (ours and BOSS TONE STUDIO's `.alb`), newest first.
    pub fn list_backups(&self) -> Vec<BackupInfo> {
        let mut out: Vec<BackupInfo> = Self::files_with(&self.backups_dir(), &["json", "alb"])
            .into_iter()
            .filter_map(|path| {
                let meta = fs::metadata(&path).ok()?;
                let created = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .and_then(|s| s.strip_prefix("backup-"))
                    .and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y%m%d-%H%M%S").ok())
                    .and_then(|d| d.and_local_timezone(chrono::Local).single())
                    .map(|d| d.timestamp())
                    .or_else(|| {
                        let m = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
                        Some(m.as_secs() as i64)
                    })?;
                Some(BackupInfo { path, created, bytes: meta.len() })
            })
            .collect();
        out.sort_by_key(|b| std::cmp::Reverse(b.created));
        out
    }

    pub fn save_backup(&self, b: &Backup) -> io::Result<PathBuf> {
        let f = BackupFile {
            format: BACKUP_FORMAT.into(),
            version: 1,
            created: b.created,
            bts_version: b.bts_version.clone(),
            system: b
                .system
                .iter()
                .map(|a| AreaFile { name: a.name.clone(), addr: a.addr.to_string(), data: hex(&a.data) })
                .collect(),
            patches: b
                .patches
                .iter()
                .map(|(n, name, d)| PatchFile { number: *n, name: name.clone(), data: hex(d.bytes()) })
                .collect(),
        };
        let stamp = chrono::DateTime::from_timestamp(b.created, 0)
            .map(|d| d.with_timezone(&chrono::Local).format("%Y%m%d-%H%M%S").to_string())
            .unwrap_or_else(|| b.created.to_string());
        let mut path = self.backups_dir().join(format!("backup-{stamp}.json"));
        for i in 2.. {
            if !path.exists() {
                break;
            }
            path = self.backups_dir().join(format!("backup-{stamp}-{i}.json"));
        }
        write_atomic(&path, &serde_json::to_vec(&f)?)?;
        Ok(path)
    }

    /// Reads a backup made by this editor or by BOSS TONE STUDIO.
    pub fn load_backup(&self, path: &Path) -> io::Result<Backup> {
        let raw = fs::read(path)?;
        if path.extension().is_some_and(|x| x == "alb") {
            let created = fs::metadata(path)?
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            return backup_from_alb(serde_json::from_slice(&raw).map_err(invalid)?, created);
        }
        let f: BackupFile = serde_json::from_slice(&raw).map_err(invalid)?;
        if f.format != BACKUP_FORMAT && f.format != OLD_BACKUP_FORMAT {
            return Err(invalid("not a backup file"));
        }
        let system = f
            .system
            .into_iter()
            .map(|a| {
                let addr = Address::parse(&a.addr).ok_or_else(|| invalid(format!("bad address {}", a.addr)))?;
                Ok(Area { name: a.name, addr, data: unhex(&a.data)? })
            })
            .collect::<io::Result<_>>()?;
        let patches = f
            .patches
            .into_iter()
            .map(|p| Ok((p.number, p.name, patch_from_hex(&p.data)?)))
            .collect::<io::Result<_>>()?;
        Ok(Backup { origin: Origin::Editor, created: f.created, bts_version: f.bts_version, system, patches })
    }

    pub fn delete_backup(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(fill: u8) -> PatchData {
        PatchData::from_bytes(vec![fill; patch_span() as usize])
    }

    #[test]
    fn setlists_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf()).unwrap();
        let mut s = Setlist::new("Gig".into());
        s.target_start = 10;
        s.entries.push(Entry { source: 45, name: "TOM SAWYER".into(), data: Some(patch(7)) });
        s.entries.push(Entry { source: 3, name: "2112".into(), data: None });
        store.save_setlist(&s).unwrap();
        store.save_setlist(&s).unwrap(); // overwrite in place

        let (lists, errors) = store.load_setlists();
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(lists, vec![s.clone()]);

        store.delete_setlist(&s.id).unwrap();
        assert!(store.load_setlists().0.is_empty());
    }

    #[test]
    fn export_import_makes_an_independent_copy() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().join("data")).unwrap();
        let mut s = Setlist::new("Gig".into());
        s.entries.push(Entry { source: 4, name: "LEAD".into(), data: Some(patch(9)) });
        store.save_setlist(&s).unwrap();
        let file = dir.path().join("gig.json");
        Store::export_setlist(&s, &file).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2)); // ids are millisecond-based
        let imported = Store::import_setlist(&file).unwrap();
        assert_ne!(imported.id, s.id);
        assert_eq!((imported.name.as_str(), &imported.entries), ("Gig", &s.entries));
        store.save_setlist(&imported).unwrap();
        assert_eq!(store.load_setlists().0.len(), 2, "the original is untouched");
        fs::write(&file, b"{}").unwrap();
        assert!(Store::import_setlist(&file).is_err());
    }

    #[test]
    fn bad_files_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf()).unwrap();
        fs::write(dir.path().join("setlists/broken.json"), b"{ nope").unwrap();
        store.save_setlist(&Setlist::new("ok".into())).unwrap();
        let (lists, errors) = store.load_setlists();
        assert_eq!(lists.len(), 1);
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn backups_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf()).unwrap();
        let b = Backup {
            origin: Origin::Editor,
            created: now(),
            bts_version: "1.0".into(),
            system: vec![Area {
                name: "System_common".into(),
                addr: Address::from_wire([0, 0, 0, 0]),
                data: vec![1, 2],
            }],
            patches: vec![(1, "A".into(), patch(1)), (2, "B".into(), patch(2))],
        };
        let p1 = store.save_backup(&b).unwrap();
        let p2 = store.save_backup(&b).unwrap();
        assert_ne!(p1, p2, "same second doesn't overwrite");
        assert_eq!(store.load_backup(&p1).unwrap(), b);
        let list = store.list_backups();
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|i| (i.created - b.created).abs() <= 1 && i.bytes > 0));
    }

    /// An `.alb` shaped like BOSS TONE STUDIO's, from a backup.
    fn to_alb(b: &Backup) -> serde_json::Value {
        let hexes = |d: &[u8]| d.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>();
        let system: serde_json::Map<_, _> =
            b.system.iter().map(|a| (a.name.clone(), serde_json::json!(hexes(&a.data)))).collect();
        let patches: Vec<serde_json::Value> = b
            .patches
            .iter()
            .map(|(_, _, d)| {
                let m: serde_json::Map<_, _> = patch_blocks()
                    .iter()
                    .map(|bl| {
                        let bytes = &d.bytes()[bl.offset as usize..(bl.offset + bl.size) as usize];
                        (format!("User_patch%{}", bl.name), serde_json::json!(hexes(bytes)))
                    })
                    .collect();
                serde_json::Value::Object(m)
            })
            .collect();
        serde_json::json!({ "system": system, "userPatch": patches, "formatRev": "0000", "device": "GX-100" })
    }

    #[test]
    fn reads_boss_tone_studio_backups() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf()).unwrap();
        let common = tables::root("System_common").unwrap();
        let mut named = vec![0u8; patch_span() as usize];
        named[..16].copy_from_slice(b"CRUNCH          ");
        let b = Backup {
            origin: Origin::BossToneStudio,
            created: 0,
            bts_version: "BOSS TONE STUDIO".into(),
            system: vec![Area { name: "System_common".into(), addr: common.addr, data: vec![3; common.size as usize] }],
            patches: vec![(1, "CRUNCH".into(), PatchData::from_bytes(named)), (2, patch(0).name(), patch(0))],
        };
        let path = dir.path().join("backups/GX-100.alb");
        fs::write(&path, serde_json::to_vec(&to_alb(&b)).unwrap()).unwrap();

        let list = store.list_backups();
        assert_eq!(list.len(), 1, "the .alb is listed");
        let read = store.load_backup(&path).unwrap();
        assert_eq!(Backup { created: read.created, ..b.clone() }, read);

        // a block we don't know, a short system area, another device: refused
        let mut bad = to_alb(&b);
        bad["userPatch"][0]["User_patch%mystery"] = serde_json::json!(["00"]);
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(store.load_backup(&path).is_err());
        let mut bad = to_alb(&b);
        bad["system"]["System_common"] = serde_json::json!(["00"]);
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(store.load_backup(&path).is_err());
        let mut bad = to_alb(&b);
        bad["device"] = serde_json::json!("GT-1000");
        fs::write(&path, serde_json::to_vec(&bad).unwrap()).unwrap();
        assert!(store.load_backup(&path).unwrap_err().to_string().contains("GT-1000"));
    }

    /// Compares a real BOSS TONE STUDIO backup with one of ours taken from the same pedal:
    /// `OPENGX100_ALB=… OPENGX100_OURS=… cargo test -p opengx100-gui -- --ignored same_pedal`
    #[test]
    #[ignore]
    fn same_pedal_boss_and_ours_match() {
        let (Ok(alb), Ok(ours)) = (std::env::var("OPENGX100_ALB"), std::env::var("OPENGX100_OURS")) else {
            panic!("set OPENGX100_ALB and OPENGX100_OURS");
        };
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path().to_path_buf()).unwrap();
        let boss = store.load_backup(Path::new(&alb)).unwrap();
        let ours = store.load_backup(Path::new(&ours)).unwrap();
        assert_eq!(boss.system, ours.system);
        assert_eq!(boss.patches.len(), ours.patches.len());
        for (a, b) in boss.patches.iter().zip(&ours.patches) {
            assert_eq!(a, b, "patch {}", a.0);
        }
    }

    #[test]
    fn hex_roundtrip_and_errors() {
        assert_eq!(unhex(&hex(&[0, 0x7f, 0xff])).unwrap(), vec![0, 0x7f, 0xff]);
        assert!(unhex("abc").is_err());
        assert!(unhex("zz").is_err());
        assert!(patch_from_hex("00").is_err(), "wrong length");
    }
}
