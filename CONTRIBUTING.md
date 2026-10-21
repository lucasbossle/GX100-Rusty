# Contributing to GX100-Rusty

Thanks for helping! Bug reports, hardware test reports (especially Windows and Linux), and pull requests are all
welcome. For anything bigger than a small fix, please open an issue first so we can agree on the approach.

## Ground rules

### 1. Nothing from Roland in the repository

This project stays redistributable because it contains no Roland material. Never commit:

- anything under `extracted/` (generated from BOSS TONE STUDIO; gitignored);
- code, JSON, images, fonts or strings copied from BOSS TONE STUDIO;
- text from Roland's manuals or parameter guide (descriptions are downloaded on the user's machine by
  `tools/extract/manual.mjs`);
- screenshots that show those descriptions. Take screenshots with the simulator (`--sim`) or with descriptions off.

Facts needed for interoperability (addresses, message order, parameter names as the pedal shows them) are fine, written
in your own words. Test fixtures in `crates/gx100-protocol/fixtures/` must stay **synthetic** (made-up effects, made-up
text).

### 2. Pedal safety

People run this against their own pedal and their own patches.

- **Write exactly what BOSS TONE STUDIO writes, in the same order.** Find the sequence in its JavaScript (see
  "How we got here" in `CLAUDE.md`) and describe it in your PR.
- `Device::write` refuses the `7F …` command space, and `send_command` has an allowlist. A new store command gets
  its own traced, tested method. Never add factory reset, patch initialize/insert/exchange, temp clear or device-ID
  changes without discussing it first.
- Verify new writes on hardware as **no-ops** first (write back the current value, read back, compare); see
  `gx100 write-check`. Say in the PR what you tested on a real pedal.
- Make a backup before testing (Backups tab).

### 3. Code

- The protocol crate (`crates/gx100-protocol`) has **no dependencies**. Keep it that way.
- JavaScript only runs in `gx100-extract`, once, on BOSS TONE STUDIO's configuration files. The editor itself is
  plain Rust.
- Format with `cargo fmt` (`max_width = 120`), keep `cargo clippy --workspace --all-targets` clean.
- Match the style around you: small functions, comments that explain *why*.

## Development setup

You need Rust 1.95+ and BOSS TONE STUDIO for GX-100 installed (the app and CLI read their tables from it, see
`crates/gx100-extract`). Node.js 18+ is only needed for the development tools in `tools/extract/`.

```sh
cargo test                              # synthetic fixture tables (what CI would use)
node tools/extract/extract.mjs          # real tables → extracted/tables.json (+ intermediate JSON to inspect)
GX100_TABLES=$PWD/extracted/tables.json cargo test --workspace     # the same tests on the real tables
cargo clippy --workspace --all-targets
cargo run -p gx100-gui -- --sim         # the editor against the simulated pedal
cargo test --release -p gx100-extract -- --ignored   # boa's extraction == Node's (needs BOSS TONE STUDIO)
```

Run the tests on **both** table sets. A few GUI tests skip themselves on the fixture (they need the real tables).

If you change `tools/extract/core.js`, it runs in two engines (Node and boa, the latter at the app's first launch):
check both still agree with the ignored test above.

Useful while working on the protocol:

```sh
cargo run -p gx100-cli -- monitor 30    # what the pedal sends by itself
cargo run -p gx100-cli -- patch         # decode the live patch
```

Quit the GUI before using the CLI on the pedal (both toggle the pedal's editor mode).

`CLAUDE.md` holds the detailed development notes: what is known about the protocol, what has been verified on
hardware, and the open items. Please update it when you learn something new about the pedal.

## Pull requests

- One topic per PR, with a short description of what and why.
- Include tests where it makes sense (the simulator in `gx100-protocol::sim` covers most write paths).
- Mention what you verified on hardware, if anything.
- By contributing, you agree that your contribution is dual licensed under MIT OR Apache-2.0, like the project.
