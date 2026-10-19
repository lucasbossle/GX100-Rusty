# CLAUDE.md — context for Claude Code sessions

## Goal
Rebuild BOSS TONE STUDIO for GX-100 as a Rust app (no JS at runtime), then extend it with features the official
editor lacks. Owner is investigating/learning; prefer small, verifiable steps.

## How we got here
The official app (`/Applications/BOSS/GX-100/BOSS TONE STUDIO for GX-100.app`) is a native macOS WKWebView shell around
plain HTML/JS in `Contents/Resources/html/js/`. The protocol was read from that JS (no sniffing, no decompiling):
- `common/midi_controller.js` — DT1/RQ1 framing + checksum, send pacing
- `config/product_setting.js` — model ID `000000000B`, device ID `10`, communication level 3
- `config/address_map.js` — Roland-generated parameter map (2,967 System/Temp params; 200 user patches)
- `businesslogic/address_const.js` — special command addresses (`7F …`)
- `config/effect_parameter.js` + `businesslogic/chain/chain_controller.js` — 83 FX types; meaning of `param(n)` per type
- `businesslogic/midi_connect_controller.js` — handshake: identity → read `EDITOR_COMMUNICATION_LEVEL` → write 1 to
  `EDITOR_COMMUNICATION_MODE`; app writes 0 there on close
`tools/extract/extract.mjs` runs those files in a Node `vm` sandbox and emits `extracted/*.json` + `extracted/tables.rs`.

## Layout
- `crates/gx100-protocol` — **dependency-free** lib: `address`, `sysex` (incl. `SysexAssembler`), `value`, `tables`
  (generated, `include!`d via build.rs), `session` (`Device<T: Transport>`), `patch` (FX slots), `sim` (in-memory pedal)
- `crates/gx100-cli` — `gx100` binary; `midi.rs` (midir) behind default feature `midi`
- `crates/gx100-protocol/fixtures/` — SYNTHETIC tables used when `extracted/tables.rs` is absent (CI/tests)

## Verification status
- 30 unit tests pass and clippy is clean against the fixture tables (verified in a cloud sandbox, rustc 1.97).
- The real `extracted/tables.rs` passed a line-shape lint but has **not yet been compiled** — run `cargo test` first.
- `crates/gx100-cli/src/midi.rs` has **never been compiled** (crates.io was unreachable when written; written against
  midir 0.10's API). Expect possible small API fixes.
- **Nothing has been tested on the real pedal yet.** First hardware run should be read-only: `ports`, `identify`, `hello`.

## Commands
```sh
node tools/extract/extract.mjs      # regenerate tables after a BTS update
cargo test
cargo run -p gx100-cli -- --sim hello
cargo run -p gx100-cli -- hello     # real pedal, read-only
```
The Mac this was set up on had no Rust toolchain yet (install via rustup).

## Rules
- Never commit `extracted/` or anything copied from Roland's app (gitignored). Fixture data must stay synthetic.
- Safety: `Device::write` refuses `7F xx xx xx`; `send_command` has an allowlist. Never add `FACTORY_RESET`,
  `PATCH_WRITE`/`INITIALIZE`/`INSERT`/`EXCHANGE`, `PRE_DEVICE_ID_CHANGE` to it without tracing the official app's exact
  sequence (see `librarian_services.js`, `patch_controller.js`) and asking the owner.
- CLI writes only touch the Temp patch unless `--allow-system`.
- rustfmt: `max_width = 120` (rustfmt.toml). Keep the protocol crate free of dependencies.

## Likely next steps
1. Build on the Mac, fix any midir API drift, run read-only commands against the pedal.
2. Full-patch read (RQ1 of the whole Temp block, ~7,219 bytes) and decode into a typed `Patch`.
3. Live-set files: `.tsl` (JSON) import/export.
4. Patch write/save flow (trace `PATCH_WRITE` usage first).
5. GUI (egui/iced/Slint) on top of `gx100-protocol`.
