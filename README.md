# gx100-editor

An independent editor for the BOSS GX-100, to be written in Rust (no JavaScript at runtime).

**Status:** protocol investigation. The SysEx tables are extracted from a locally installed
copy of *BOSS TONE STUDIO for GX-100*; nothing from Roland is committed to this repo.

## Extracting the protocol tables

Requires Node ≥ 18 and an installed copy of BOSS TONE STUDIO for GX-100 (tested with 2.0.1 build 118, macOS).

```sh
node tools/extract/extract.mjs                       # default: /Applications/BOSS/GX-100/BOSS TONE STUDIO for GX-100.app
node tools/extract/extract.mjs "/path/to/BOSS TONE STUDIO for GX-100.app" extracted
```

Output goes to `extracted/` (gitignored):

| File | Contents |
|---|---|
| `product.json` | Model ID, device ID, address/size lengths, communication level |
| `commands.json` | Identity request/reply, special command addresses (patch write, tuner, running mode…), fixed addresses, enums |
| `address_map.blocks.json` | The address map as Roland structures it: `root` (top-level instances) + reusable `blocks` with relative offsets |
| `address_map.flat.json` | Every parameter of System, PC map, Setup and the **Temp patch**, with absolute addresses (user patches omitted — same layout as Temp, at their own base) |
| `effects.json` | The 83 FX types (index = value of `fxItem(n).Type`) and, per type, what each generic `param(n)` slot means, with value labels |
| `resources.json` | Value-label lists referenced by `resourceId` |

## Protocol notes

Roland SysEx over USB MIDI. Device ID `10`, model ID `00 00 00 00 0B`, 4-byte addresses and sizes.

```
DT1 (write):  F0 41 10 00 00 00 00 0B 12 <addr:4> <data…>  <sum> F7
RQ1 (read):   F0 41 10 00 00 00 00 0B 11 <addr:4> <size:4> <sum> F7   → device replies with DT1
sum = (128 - (Σ addr+data bytes) % 128) & 0x7F
Identity:     F0 7E 7F 06 01 F7   → reply starts F0 7E 10 06 02 41 0B 04
```

- **Addresses** are four 7-bit bytes. To add offsets, convert to linear (`(a<<21)|(b<<14)|(c<<7)|d`), add, convert back.
- **Encodings:** `INTEGER1x7` = 1 byte; `INTEGER2x4` / `INTEGER4x4` = one nibble per byte, MSB first; `INTEGER2x7` = two 7-bit bytes.
  Wire value = `value + ofs` (FX params use `ofs = 32768`, so 0 → `08 00 00 00`).
- **Memory layout:** System `00 00 xx xx`, PC map `00 10 …`, Setup/temp `00 20 …`, **Temp (live) patch `10 00 00 00`**,
  user patches `20 00 00 00` + n·`00 06 00 00` (200 of them), patch names `50 …`, IR table `60 40 …`.
- **Patch** = common · LED · 20 assigns · chain (`efct` @ `00 00 0F 00`) · 20 FX slots (`fxItem(n)` @ `00 00 11 00` + (n-1)·`00 00 02 00`).
  Each slot: `Type` (+0), `Switch` (+1), `DuplicationNumber` (+2), `param(0..)` (+3, 4 bytes each). `effects.json` says what each `param(n)` is for the slot's Type.
- Before editing, the official app sets `EDITOR_COMMUNICATION_MODE` (`7F 00 00 01`) to 1.

Example — turn FX slot 1 on in the live patch: `F0 41 10 00 00 00 00 0B 12 10 00 11 01 01 5D F7`

⚠️ `FACTORY_RESET` lives at `7F 02 00 7C`. Never write to `7F …` addresses you haven't traced in the original app.

## Legal

This is a clean-room-style interoperability project. Roland's code, assets and the extracted tables are not
redistributed; users regenerate the tables from their own licensed install.
