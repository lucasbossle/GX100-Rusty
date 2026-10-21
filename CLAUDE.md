# CLAUDE.md — context for Claude Code sessions

## Goal
Rebuild BOSS TONE STUDIO (BTS) for GX-100 as a Rust app, then extend it with features the official
editor lacks. Published as **GX100-Rusty** (public, https://github.com/lucasbossle/GX100-Rusty, branch `master`, MIT OR
Apache-2.0, macOS release zips on GitHub). The only JS that ever runs is BTS's own configuration scripts, once, to
read the tables (owner approved the embedded engine on 2026-10-20). The owner is investigating/learning: prefer small, verifiable steps, explain findings, and verify on the
real pedal (read-only first, then no-op writes) before calling something done.

## Next session: start here
State on 2026-10-20 (third session): repo public, v0.1.0 released for macOS. 55 protocol + 1 extract (+1 ignored) + 10 GUI
tests pass on both table sets (`cargo test` = fixture; `GX100_TABLES=$PWD/extracted/tables.json cargo test
--workspace` = real), clippy clean. Ask the owner before committing/pushing.

Done in the third session:
- **Tables at run time** (so the release contains nothing of Roland's): `tools/extract/core.js` (engine-neutral
  extraction, shared) → JSON bundle → `tables::from_json` → `tables::install`. `gx100-extract` runs core.js in boa at
  first launch (1.3 s release / ~8 s debug), caches `~/Library/Application Support/gx100-editor/tables/tables.json`
  (+ optional `docs.json` beside it), re-extracts when BTS's version/build changes. Verified identical to the Node
  output (ignored test `matches_the_node_tool`) and to the old generated tables (one-off field-by-field comparison).
  GUI shows a first-launch screen (`setup.rs`: spinner / "BOSS TONE STUDIO is needed" + Locate…). CLI: `--bts`.
- **Release**: `tools/macos/bundle.sh` → universal `dist/GX100-Rusty.app` (ad-hoc signed, macOS 11+) + zip. Binary
  checked: no guide text, no table strings beyond the parameter paths our code names.
- Assign writes verified on the pedal; live "now" column on the Assigns page; README/CONTRIBUTING for open source.

Open items, in order:
1. **Try the Assigns page on the pedal**: the "now" column (and "Now" row in the editor) shows each FX/MASTER
   target's live value, amber while the pedal changes it, with a bar between MIN and MAX. Unknown whether the pedal
   pushes changes made by EXP/INT PDL/WAVE assigns (footswitch toggles are pushed). Change a target on a scratch
   patch and check MIN/MAX show what the pedal reset them to.
2. Look at the GUI (Claude can't): Assigns layout, tooltips, option notes under list params.
3. Smaller: hide FX params that don't apply in the current mode (e.g. CHORUS `rate-1/-2` only in STEREO modes; see
   how `effect_parameter.js` / chain UI decide visibility); change FX type / chain order (needs tracing the official
   chain-edit sequence: `CHAIN_EDIT_TRIGGER`, `FX_SELECTABLE`, `FX_OVERWRITE` in `chain_controller.js`); decode
   `led.OnOffState`; Restore from backup (writes; same block order as the librarian); `.tsl` live-set import/export;
   optionally save backups as `.alb` so BTS can restore them; HARMONIST "canShift" target names (BTS renames some
   harmonist targets by key); KNOB SETTINGS (not traced yet).
4. Release follow-ups: descriptions in the release (in-app download of the parameter guide: needs an HTTP client and
   running manual.mjs's parsing in boa); app icon; CI (GitHub Actions: fmt, clippy, tests on the fixture); Windows/
   Linux (find BTS there; bundle); Developer ID signing/notarization if the owner gets one.

## How we got here
BTS (`/Applications/BOSS/GX-100/BOSS TONE STUDIO for GX-100.app`) is a macOS WKWebView shell around plain HTML/JS in
`Contents/Resources/html/`. The protocol was read from that JS (no sniffing, no decompiling):
- `js/common/midi_controller.js` — DT1/RQ1 framing + checksum, send pacing (3125 B/s)
- `js/config/product_setting.js` — model ID `000000000B`, device ID `10`, communication level 3
- `js/config/address_map.js` — Roland-generated parameter map (2,967 System/Temp params; 200 user patches)
- `js/businesslogic/address_const.js` — special command addresses (`7F …`)
- `js/config/effect_parameter.js` + `js/businesslogic/chain/chain_controller.js` — 83 FX types; `param(n)` per type
- `js/businesslogic/midi_connect_controller.js` — handshake: identity → read `EDITOR_COMMUNICATION_LEVEL` → write 1 to
  `EDITOR_COMMUNICATION_MODE`; app writes 0 there on close
- `js/businesslogic/midi_observe_controller.js` — what the app does with DT1s the pedal pushes
- `js/businesslogic/patch_controller.js` — select patch, WRITE, CLEAR; `js/common/librarian.js` +
  `js/config/librarian_setting.js` — librarian read/write/temporaryWrite; `js/businesslogic/backup/` — backup all
- `bg777-pc.elf` (JSON) — the GX-100 UI definition: pages of widgets bound to parameters (`parameter-id`
  "System_control%29" = block + decimal offset) with label lists (`resource-id` → `js/config/resource.js`).
  `bg709-pc.elf` is another model (differs e.g. in AutoOff range).
`tools/extract/core.js` runs those files (`extractTables(host)` + `tablesBundle`); hosts: `extract.mjs` (Node `vm`
sandbox, writes `extracted/*.json` incl. `tables.json`) and `gx100-extract` (boa). Node was installed with Homebrew on
2026-10-19 (owner approved); it's only needed for development tools now.

## Layout
- `crates/gx100-protocol` — **dependency-free** lib:
  - `address`, `sysex` (incl. `SysexAssembler`), `value` (encodings), `json` (small reader), `tables` (runtime:
    `from_json`/`load_file`/`install`/`get`; without install: `$GX100_TABLES` else the fixture. `PARAMS` etc. are
    `Table<T>` deref views (`PARAMS.iter()`, `*SOURCE`); `PARAMS`, `FX_TYPES`, `ROOT`, `COMMANDS`, `ADDRESSES`, `PARAM_VALUES` + `ParamDef::display/labels`,
    `MASTER` (patch master settings as an FX-like type, for assign ranges), `ASSIGN_CATEGORIES/TARGETS`,
    `FX_DOCS`/`PARAM_DOCS` + `fx_doc`, `param_doc`)
  - `session` — `Device<T: Transport>`: read/write, `identify`, editor mode, notification queue
    (`next_notification`; unsolicited DT1s that arrive during a read are kept), `rq1_accepted`, `wait_until_ready`,
    `save_temp_patch` (PATCH_WRITE), `save_input_setting`; blanket `Transport for Box<T>`
  - `patch` — `PatchData` (a whole patch, linear bytes; `name`, `value`, `chain`, `chain_layout` with DIVIDER/
    SPLITTER/MIXER branch, `fx_slot`, `fx_param`, `apply`), `patch_blocks()` (derived from PARAMS),
    `Device::read_patch/read_patch_name/select_patch/write_user_patch/load_into_temp`
  - `assign` — the 20 assigns: `target` (→ `Target::Fx/Master/Tuner/MidiCc/MidiPc`), `category_choices`,
    `targets_for`, `applies` (what BTS greys out), and edits that return BTS's messages (`set_field`, `set_target`,
    `set_category`, `block_message`)
  - `librarian` — `plan_moves`, `write_messages` (user patch), `temp_write_messages` (efct first), `wire_cost`
  - `backup` — `SYSTEM_AREAS` (24, BTS's `SystemSetting`), `Device::read_system_areas`
  - `sim` — in-memory pedal: answers RQ1/identity/IS_RQ1_ACCEPT, handles PATCH_WRITE, CurrentPatchNum (loads the
    patch into Temp), SETTING_WRITE_CMD; `push_unsolicited` fakes pedal-initiated DT1s
- `crates/gx100-midi` — midir transport (`open`, `open_with_waker`, `port_names`), shared by CLI and GUI
- `crates/gx100-cli` — `gx100` binary (MIDI behind default feature `midi`): `ports identify hello get set slots patch
  monitor write-check fx-get fx-set find fx-types commands`
- `crates/gx100-gui` — `gx100-editor` binary, eframe/egui **0.37** (new API: `App::ui` + `App::logic`,
  `egui::Panel::top/left`, `Id::unique`, `ScrollArea::scroll_source`). Deps allowed here: serde, serde_json, dirs,
  chrono, rfd.
  - `worker.rs` — owns the `Device` on its own thread (opens MIDI there; midir handles needn't be `Send`). UI sends
    `Cmd`, gets `Event`. **No polling:** editor mode on at connect (off on `Shutdown`, sent from `EditorApp::drop`); the
    midir callback sends `Cmd::MidiArrived`, the worker drains notifications → `TempChanged`, `SystemChanged`,
    `CurrentPatch` (then re-reads Temp), pedal-side WRITE (re-reads that patch). Background jobs, one step per loop
    pass: name scan, `BackupJob`, `SetlistJob` (write + read back + compare). `writable()` only lets `WriteBytes`
    reach the Temp patch and system areas.
  - `app.rs` — window, patch list, live editor (Patch tab), chain diagram (split/rejoin wiring; active branch solid
    blue, inactive dashed/dimmed; DIVIDER caption "A only/B only/A + B"), FX param editor, events.
  - `app/editing.rs` — value widgets (ComboBox for labelled values, checkbox for 2-value, slider otherwise), writes
    (`set_temp_param`, `set_fx_param`, `set_fx_switch`, `set_sys_param`), unsaved-changes check (`patch_differs`:
    ignores pedal-position and the led block), CTL/EXP, IN/OUT, System menus, dialogs (Write, unsaved switch,
    save input setting, confirm setlist write).
  - `app/assigns.rs` — CTL/EXP → Assigns page.
  - `app/setlists.rs` — Setlists tab; `app/backups.rs` — Backups tab; `store.rs` — persistence.
- `crates/gx100-protocol/fixtures/{tables,docs}.json` — SYNTHETIC tables (made-up FX), embedded as the fallback
- `crates/gx100-extract` — boa + dirs: `find_app`, `app_version`, `extract` (core.js in boa), `prepare` (env → cache
  → extract), `cache_dir`. Used by CLI and GUI.
- `tools/macos/bundle.sh` — universal .app + zip in `dist/` (gitignored)

## The GUI today (what each tab does)
- **Patch** (live editor): always shows the patch active on the pedal ("005 BARCHETTA · active on the pedal"),
  follows patch changes made on the pedal, flashes values the pedal changes (footswitches, expression pedal: live
  amber bar on pedal FX). Clicking a patch in the list switches the pedal to it (asks first if there are unsaved
  edits). Every patch setting, FX on/off and FX parameter is editable and written immediately. "● edited" = live patch
  differs from the stored copy. **Write…** = PATCH_WRITE (name + target slot, warns when replacing another patch).
  **Revert** = load the stored copy into the live patch.
- **CTL/EXP**: per control (NUM1-4, CURNUM, BANK▼▲, CTL1-4, EXP1 SW, EXP1 PEDAL, EXP2, MANUAL1-4): PREFERENCE
  (MEMORY = patch's own setting / SYSTEM = system-wide; `System_control.<x>Preference`), patch function/mode
  (`Temp_patch.common.<x>Function/Mode`), system function/mode (`System_control.<x>Function/Mode`); the one in use
  is highlighted. Second page **Assigns**: the live patch's 20 assigns (see "Next session").
- **IN/OUT**: input setting preset (`System_control.InputSettingNum`; choosing one makes the pedal load it, so the
  app re-reads system), "Save current settings here…" (SETTING_WRITE), input sens/type (`Setup_efct`), output
  select/level (`System_inout`), Global EQ (`System_control.GlobalEqSw` + `Setup_efct.IOOutGlobalEq*`).
  Unknown whether the pedal persists `Setup_efct` EQ edits by itself; we write exactly what BTS writes.
- **System**: hardware, play options, MIDI, MIDI CC numbers, USB audio — param lists taken from the BTS menu pages.
- **Setlists** (name chosen by the owner): saved ordered lists of patches. Tick patches in the list, drag rows to
  reorder (custom drag; scroll area has drag-scroll off), pick the target start slot, preview (slot / now / would
  become; red = patches not in the setlist that get overwritten, amber = duplicates), DT1 list per patch.
  Auto-saved 0.5 s after each change and on quit; switch/new/duplicate/rename/delete; **Export…/Import…** (rfd file
  dialogs; import always gets a new id). Entries keep a **snapshot** of their patch; rows show ✓ / "⚠ changed" (pedal
  differs; "use pedal's") / "saved copy". **Write to pedal…** → confirm dialog (lists patches lost, current patch
  rewritten, latest backup) → background job with progress/Stop, each patch read back and compared; if the current
  patch was rewritten, its new data is loaded into the live patch.
- **Backups**: "Back up now" (system areas + 200 patches, ~4 min, background, cancel; also fills the patch cache),
  list (ours `.json` and BTS `.alb`), open to browse (names that differ from the pedal in amber), Show in Finder,
  delete (confirm). **Restore is disabled** (it would write).
- Storage: `~/Library/Application Support/gx100-editor/{setlists/<id>.json, backups/backup-YYYYMMDD-HHMMSS.json}`,
  hex patch data, atomic writes (tmp + rename), unreadable files reported but not fatal.

## Verification status (2026-10-20, second session)
- Tests: 55 protocol (fixture by default; real with `GX100_TABLES=$PWD/extracted/tables.json`), 10 GUI (3 skip on
  the fixture)
  (store round-trips incl. `.alb`, export/import, worker events, backup job, setlist job, write guard).
  Ignored test `same_pedal_boss_and_ours_match` compares a real `.alb` with our backup:
  `GX100_ALB=… GX100_OURS=… cargo test -p gx100-gui -- --ignored same_pedal`.
- Real pedal: identify, hello, get, patch, monitor, backups (byte-identical to BTS's `.alb` of the same pedal).
  Writes verified as no-ops with read-back (`write-check`): setlist write (user-patch DT1s), system param write.
  Select patch and PATCH_WRITE verified on the pedal (2026-10-20, `write-check` on patch 118). Assign writes
  verified too (second run; the pedal resets TARGET MIN/MAX when TARGET is written, see Protocol facts). Assign
  decoding verified by reading patch "ANALOG KID" (MAN 2/3 assigns switching BOOSTER/DELAY+/SOLO, CTL 1 → PITCH
  SHIFTER).
- GUI runs against the pedal; Claude has no screen-capture permission, so nothing visual has been checked by Claude
  (branched chain diagram only seen with `--sim` demo patches, which have a DIVIDER/SPLITTER/MIXER split).

## Protocol facts (learned from BTS's JS and the pedal)
- `System_common.CurrentPatchNum` (`00 00 00 00`, 4 nibbles) is **0-based** (131 = `User_patch(132)`).
- Chain = linked list in `efct`: `ChainTopItem` = first slot, `ChainNextItem<i>` = slot after slot i (0-based, stored
  +1, -1 ends). Slots not in the list are unused but keep stale data. `[pre] DIVIDER [A…] SPLITTER [B…] MIXER [post]`,
  at most one each, all or none, in that order, else broken (`convert2ChainBlockIdOrder`). SPLITTER is only a marker.
  Active path: DIVIDER `mode` DUAL(1) → both; SINGLE → `ch-select` A(0)/B(1) (`ChainDivider.js` `turnChildLine`).
- Patches are read/written block by block: common, led, assign(1..20), efct, fxItem(1..20) (43 blocks, 4,699 real
  bytes in a 7,219-byte span). Full patch read ≈ 44 RQ1s ≈ 1.2 s; a name is one RQ1 (~10 ms). Reads work without
  editor mode.
- **Editor mode on** (`7F 00 00 01` = 1) → the pedal pushes DT1s: expression pedal → `pedal-position` of the pedal
  FX slot (many/s); footswitches → the toggled param + `Temp_patch.led.OnOffState` (8 bytes); patch change →
  `CurrentPatchNum` then `Temp_patch.common.Name` only (re-read Temp after `IS_RQ1_ACCEPT` says ready); WRITE on the
  pedal → DT1 at PATCH_WRITE; also RUNNING_MODE/TUNER, `System_control.ControlMode`, sometimes whole fxItem/efct blocks.
- `IS_RQ1_ACCEPT` (`7F 00 07 03`): send DT1 0, pedal answers 00 busy / 01 ready. On the allowlist (status only).
- Writes (traced): select patch = DT1 CurrentPatchNum, then wait until ready. WRITE = Temp name DT1, then PATCH_WRITE
  `[i>>7, i&7F]` (0-based); pedal confirms with a DT1 at PATCH_WRITE, then is busy (BTS actually sends it twice;
  we send once). Librarian write = plain DT1s into `User_patch(n)` in block order, waits 0, no command. Load into Temp
  (`temporaryWrite`) = efct block first, then the rest. Input setting save = SETTING_WRITE_NAME (16 chars) +
  SETTING_WRITE_CMD 0 → slot `InputSettingNum`.
- Backup = `SystemSetting` blockSet (24 areas incl. Pcmap 512 B) + 200 user patches. BTS `.alb` = JSON
  `{"system": {area: ["0B",…]}, "userPatch": [200 × {"User_patch%<block>": [hex…]}], "formatRev": "0000",
  "device": "GX-100"}` — byte-identical content to ours (only encoding differs: ours 2.9 MB hex, theirs 4.9 MB).
- Value labels: `extract.mjs` maps `bg777-pc.elf` widgets to params → `PARAM_VALUES` (338 params; a list is kept
  only if it exactly covers the param's range; dials get labels from their `<stem>-stringer` widget; assign(1)'s
  labels are copied to assign(2..20), since BTS shows every assign on one page bound to assign(1)).
- FX list labels: a few lists (`indexMachineValue` in `effect_parameter.js`) aren't in stored-value order: stored value
  v is labelled `values[indexMachineValue[v]]` (BTS's own `templateValue` agrees). gen_rust emits them in value order.
- Assigns (`assign_page_controller.js`, `disable_dial_bg.js`): TARGET indexes `ASSIGN_TARGETS` (741); effect targets
  name a parameter (`paramUniqueName`) of the FX in slot `TARGET_FX_ITEM` (0-based), so they follow the slot's FX type.
  Category list = "----", chain blocks (no SPLITTER), MASTER, TUNER, MIDI; picking one sets `TARGET_FX_ITEM` (effects
  only) and TARGET = ON/OFF (if the category has a switch) or its first target, then sends 5 bytes from
  `TARGET_FX_ITEM`. Picking a target: DT1 TARGET (4 bytes), then the same 5 bytes. Any other field: DT1 of the field,
  then the whole 45-byte block. ACT RANGE LO/HI keep LO < HI (±128); BANK MSB OFF clears LSB. MIN/MAX use the target
  parameter's value domain. **The pedal resets TARGET MIN/MAX itself when TARGET is written** (assign 1, PREAMP SOLO
  SW, MIN 1→0, MAX stayed 1, presumably the target's full range); BTS updates from the pushed DT1 / re-reads the
  block (`requestUpdateTopTargetMinMaxForBlockIndex`); our GUI re-reads the assign block (`Cmd::ReadTemp`) after a
  target/category change. The simulator doesn't model the reset. Lamps (`led.OnOffState`) can change too; TUNER ON/OFF is 0/1; MIDI targets use MIDI_CC_VALUE_MIN/MAX. SOURCE 16/17 (EXP1/2),
  18 (INT PDL), 19 (WAVE PDL), 20 (INPUT) decide which fields apply. Two targets have no matching parameter in BTS's
  own tables (AIRD BASS PREAMP `mic-dist`, DIVIDER `signal-mix`); BTS shows no range for them, neither do we.

## Commands
```sh
node tools/extract/extract.mjs        # tables via Node → extracted/ (tables.json + intermediate JSON)
node tools/extract/manual.mjs         # fetch the parameter guide → extracted/docs.json (--refresh, --dump);
                                      # copy it to ~/Library/Application Support/gx100-editor/tables/ for the app
cargo test                            # fixture; GX100_TABLES=$PWD/extracted/tables.json cargo test --workspace = real
cargo test --release -p gx100-extract -- --ignored   # boa extraction == Node output
./tools/macos/bundle.sh               # release build: dist/GX100-Rusty.app + zip
cargo clippy --workspace --all-targets
cargo run -p gx100-gui                # the editor (binary gx100-rusty) (add `-- --sim` for the simulator)
cargo run -p gx100-cli -- hello       # real pedal, read-only
cargo run -p gx100-cli -- patch 132   # dump a stored patch incl. its assigns (no number: the live patch)
cargo run -p gx100-cli -- monitor 30  # print what the pedal pushes (editor mode on, then off)
cargo run -p gx100-cli -- write-check # no-op write verification (refuses if the live patch has unsaved edits)
```
Quit the GUI (⌘Q) before using the CLI on the pedal: two programs toggling editor mode interfere, and killing the GUI
skips its exit hook (the pedal then stays in editor mode until the next connect).

## Rules
- Never commit `extracted/` or anything copied from Roland's app or manuals (gitignored), and never ship it in a
  release: the app reads the tables from the user's BTS. Commit as the GitHub no-reply identity (repo-local config). Fixture data must stay
  synthetic (including the fixture's doc strings). `manual_map.mjs` holds only guide row names → our paths. Menu contents in our code are lists of parameter paths (our design); labels come from the extractor.
- Safety: `Device::write` refuses `7F xx xx xx`; `send_command` has an allowlist (`EDITOR_COMMUNICATION_MODE`,
  `RUNNING_MODE`, `EDITOR_PAGE`, `TUNER`, `IS_RQ1_ACCEPT`). Store commands get a dedicated, traced, tested method
  instead: `save_temp_patch` (PATCH_WRITE) and `save_input_setting` (SETTING_WRITE_NAME/CMD), enabled by the owner on
  2026-10-19. Never add `FACTORY_RESET`, `PATCH_INITIALIZE`/`INSERT`/`EXCHANGE`/`TEMP_CLEAR`, `PRE_DEVICE_ID_CHANGE`
  without tracing BTS's exact sequence and asking the owner.
- Write exactly what BTS writes, in BTS's order; when unsure what the pedal does with something, mirror BTS.
- Hardware tests of writes must be no-ops (write back what's there, verify by reading back) unless the owner says
  otherwise; keep a recent backup. Ask before installing tools.
- CLI writes only touch the Temp patch unless `--allow-system`.
- rustfmt `max_width = 120`. Keep the protocol crate free of dependencies. Run tests on both table sets.
