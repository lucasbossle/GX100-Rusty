//! The BOSS GX-100 editor protocol: Roland SysEx (DT1/RQ1) over USB MIDI.
//!
//! - [`address`]: 4-byte 7-bit addresses
//! - [`sysex`]: building/parsing messages, reassembling split SysEx
//! - [`value`]: how values are packed into data bytes
//! - [`tables`]: every parameter, command and FX type (generated)
//! - [`session`]: a [`session::Device`] on top of any [`session::Transport`]
//! - [`patch`]: patch and FX-slot helpers
//! - [`sim`]: an in-memory pedal for tests and offline use

pub mod address;
pub mod patch;
pub mod session;
pub mod sim;
pub mod sysex;
pub mod tables;
pub mod value;

pub use address::Address;
pub use session::{Device, Error, ParamValue, Transport};
