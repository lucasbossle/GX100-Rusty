# gx100-editor

An independent editor for the BOSS GX-100, written in Rust (no JavaScript at runtime).

**Status:** protocol layer + command-line tool. The SysEx tables are extracted from a locally installed
copy of *BOSS TONE STUDIO for GX-100*; nothing from Roland is committed to this repo.

## Layout

| Path | What |
|---|---|
| `tools/extract/extract.mjs` | Reads the official editor's JS and writes the protocol tables to `extracted/` (JSON + `tables.rs`) |
| `tools/extract/gen_rust.mjs` | JSON → Rust tables (called by `extract.mjs`) |
| `crates/gx100-protocol` | Dependency-free library: addresses, SysEx DT1/RQ1, value encodings, generated tables, request/response `Device`, FX-slot helpers, and an in-memory simulated pedal |
| `crates/gx100-cli` | `gx100` command-line tool (MIDI via `midir`) |

## Getting started

Requires Node ≥ 18, Rust (install with [rustup](https://rustup.rs)), and an installed BOSS TONE STUDIO for GX-100
(tested with 2.0.1 build 118, macOS).

```sh
node tools/extract/extract.mjs        # → extracted/*.json + extracted/tables.rs (gitignored)
cargo test                            # 30 tests, no hardware needed
cargo run -p gx100-cli -- --sim hello # try everything against the simulated pedal
cargo run -p gx100-cli -- ports       # is the pedal visible?
cargo run -p gx100-cli -- hello       # read-only handshake with the real pedal
```

Without `extracted/tables.rs` the crate builds against a tiny synthetic fixture (and says so), so CI and tests work
without Roland's files. Point `GX100_TABLES=/path/to/tables.rs` at another generated file to override.

### CLI

```
gx100 find <text>                     search parameters (offline)
gx100 fx-types [<type>]               list FX types / one type's parameters (offline)
gx100 identify | hello | slots        talk to the pedal (read-only)
gx100 get <path|address> [size]       read a parameter or raw bytes
gx100 set <path> <value>              write a live-patch parameter (--allow-system for system settings)
gx100 fx-get <slot>                   show an FX slot's parameters
gx100 fx-set <slot> <param> <value>   e.g. `gx100 fx-set 3 drive 80`, `gx100 fx-set 3 type "T-SCREAM"`
```

Options: `--port <name>` picks the MIDI port (default: the one named like "GX-100"), `--sim` uses the simulator.

Writes only touch the **live (Temp) patch** — nothing is saved to a memory slot. The library refuses raw writes to the
`7F …` command space; only a small allowlist of commands (editor mode, running mode, tuner, editor page) can be sent,
never patch write/initialize or factory reset.

## Protocol notes

Roland SysEx over USB MIDI. Device ID `10` (the pedal reports its own in the identity reply), model ID `00 00 00 00 0B`,
4-byte addresses and sizes.

```
DT1 (write):  F0 41 10 00 00 00 00 0B 12 <addr:4> <data…>  <sum> F7
RQ1 (read):   F0 41 10 00 00 00 00 0B 11 <addr:4> <size:4> <sum> F7   → device replies with DT1(s)
sum = (128 - (Σ addr+data bytes) % 128) & 0x7F
Identity:     F0 7E 7F 06 01 F7   → reply F0 7E <dev> 06 02 41 0B 04 <num:2> <rev:4> F7
```

- **Addresses** are four 7-bit bytes. To add offsets, convert to linear (`(a<<21)|(b<<14)|(c<<7)|d`), add, convert back.
- **Encodings:** `INTEGER1x7` = 1 byte; `INTEGER2x4` / `INTEGER4x4` = one nibble per byte, MSB first;
  `INTEGER2x7` = two 7-bit bytes; `BYTES` = raw ASCII (patch names, 16 chars).
  Wire value = `value + ofs` (FX params use `ofs = 32768`, so 0 → `08 00 00 00`).
- **Memory layout:** System `00 00 xx xx`, PC map `00 10 …`, Setup/temp `00 20 …`, **Temp (live) patch `10 00 00 00`**,
  user patches `20 00 00 00` + n·`00 06 00 00` (200 of them), patch names `50 …`, IR table `60 40 …`.
- **Patch** = common · LED · 20 assigns · chain (`efct` @ `00 00 0F 00`) · 20 FX slots (`fxItem(n)` @ `00 00 11 00` + (n-1)·`00 00 02 00`).
  Each slot: `Type` (+0), `Switch` (+1), `DuplicationNumber` (+2), `param(0..)` (+3, 4 bytes each).
  `effects.json` says what each `param(n)` means for the slot's Type.
- **Session:** the official app sends the identity request, adopts the reported device ID, reads
  `EDITOR_COMMUNICATION_LEVEL` (`7F 00 00 00`, expects 3), then writes 1 to `EDITOR_COMMUNICATION_MODE` (`7F 00 00 01`);
  it writes 0 there when closing. Output is paced to MIDI speed (3125 bytes/s).

Example — turn FX slot 1 on in the live patch: `F0 41 10 00 00 00 00 0B 12 10 00 11 01 01 5D F7`

⚠️ `FACTORY_RESET` lives at `7F 02 00 7C`. Never write to `7F …` addresses you haven't traced in the original app.

## Legal

An interoperability project. Roland's code, assets and the extracted tables are not redistributed; users regenerate
the tables from their own licensed install. The fixture in `crates/gx100-protocol/fixtures` is synthetic.
