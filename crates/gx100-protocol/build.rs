//! Picks the parameter tables to compile in:
//! 1. `$GX100_TABLES` (path to a generated tables.rs), else
//! 2. `<repo>/extracted/tables.rs` (produced by `node tools/extract/extract.mjs`), else
//! 3. the synthetic fixture in `fixtures/tables.rs` (tests still run; a warning is printed).

use std::{env, fs, path::PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let extracted = manifest.join("../../extracted/tables.rs");
    let fixture = manifest.join("fixtures/tables.rs");

    println!("cargo:rerun-if-env-changed=GX100_TABLES");
    println!("cargo:rerun-if-changed={}", extracted.display());
    println!("cargo:rerun-if-changed={}", fixture.display());

    let src = match env::var_os("GX100_TABLES").map(PathBuf::from) {
        Some(p) => {
            println!("cargo:rerun-if-changed={}", p.display());
            p
        }
        None if extracted.exists() => extracted,
        None => {
            println!(
                "cargo:warning=gx100-protocol: no extracted tables found, using the synthetic test fixture. \
                 Run `node tools/extract/extract.mjs` to generate the real ones."
            );
            fixture
        }
    };

    let out = PathBuf::from(env::var("OUT_DIR").unwrap()).join("tables.rs");
    fs::copy(&src, &out).unwrap_or_else(|e| panic!("copying {}: {e}", src.display()));
}
