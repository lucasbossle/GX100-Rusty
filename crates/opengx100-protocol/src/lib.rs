//! The BOSS GX-100 editor protocol: Roland SysEx (DT1/RQ1) over USB MIDI.
//!
//! - [`address`]: 4-byte 7-bit addresses
//! - [`sysex`]: building/parsing messages, reassembling split SysEx
//! - [`value`]: how values are packed into data bytes
//! - [`tables`]: every parameter, command and FX type (loaded from the extractor's JSON)
//! - [`json`]: the small JSON reader they need
//! - [`session`]: a [`session::Device`] on top of any [`session::Transport`]
//! - [`patch`]: patch and FX-slot helpers
//! - [`chain`]: editing the signal chain (insert, remove, move, replace effects)
//! - [`assign`]: the 20 assigns of a patch: targets, and the messages an edit sends
//! - [`backup`]: what a full backup holds, reading the system areas
//! - [`librarian`]: planning a reorder/rewrite of user patches (no writes yet)
//! - [`tuner`]: the pedal's tuner: switching it on/off, decoding what it sends
//! - [`sim`]: an in-memory pedal for tests and offline use

pub mod address;
pub mod assign;
pub mod backup;
pub mod chain;
pub mod json;
pub mod librarian;
pub mod patch;
pub mod session;
pub mod sim;
pub mod sysex;
pub mod tables;
pub mod tuner;
pub mod value;

pub use address::Address;
pub use session::{Device, Error, ParamValue, Transport};
