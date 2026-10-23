//! The pedal's tuner, as the official editor drives it (`tuner_controller.js`, `running_mode_controller.js`,
//! `midi_observe_controller.js`).
//!
//! - **On**: `System_common.TunerType` = 1 MONO / 2 POLY / 3 TT; for MONO and TT also `System_common.TunerMode` = 0;
//!   then the `RUNNING_MODE` command = 1 (MONO and TT) or 2 (POLY). Changing the kind repeats this.
//! - **Off**: `RUNNING_MODE` = 0 (play).
//! - While it runs (editor mode on), the pedal sends 48 bytes at the `TUNER` command address: 8 readings of
//!   6 bytes, the first for MONO/TT, then strings 1–7 for POLY ([`Readings`]). In TT mode it also sends the
//!   target string, note and cent offset (`Setup_temp.TTTargetStringNum`/`TTTargetName`/`TTCentOffset`).
//! - The pedal sends `RUNNING_MODE` itself when its tuner is switched on or off with the footswitch.
//!
//! Settings shown with the tuner: `System_pitch.ReferencePitch`, `TunerOutput` (THRU only for MONO/TT; the
//! official editor won't change the kind while it's THRU), `PolyTunerType`, `PolyTunerOffset`, and for TT
//! `Setup_temp.TTMode` (AUTO, or a string).

use crate::session::{Device, Error, ParamValue, Result, Transport};
use crate::tables;

/// The tuner's centre value: a reading of 5000 is in tune; 0 and 10000 are the ends of the scale.
pub const CENTRE: u16 = 5000;
pub const FULL_SCALE: u16 = 10000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Mono,
    Poly,
    /// True Temperament: like MONO, aimed at one string (or AUTO) with the fret correction of BOSS's TT necks.
    TrueTemperament,
}

impl Kind {
    /// The value of `System_common.TunerType`. The official editor treats 0 as POLY.
    pub fn from_type(v: i32) -> Kind {
        match v {
            1 => Kind::Mono,
            3 => Kind::TrueTemperament,
            _ => Kind::Poly,
        }
    }

    pub fn type_value(self) -> i32 {
        match self {
            Kind::Mono => 1,
            Kind::Poly => 2,
            Kind::TrueTemperament => 3,
        }
    }

    /// The `RUNNING_MODE` that shows this tuner.
    pub fn running_mode(self) -> u8 {
        match self {
            Kind::Poly => 2,
            Kind::Mono | Kind::TrueTemperament => 1,
        }
    }
}

/// One reading: `note` is a MIDI note number (0: no signal), `value` runs from 0 (flat) through [`CENTRE`]
/// (in tune) to [`FULL_SCALE`] (sharp); `in_tune` is the pedal's own verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reading {
    pub note: u8,
    pub value: u16,
    pub in_tune: bool,
}

impl Reading {
    pub fn has_signal(&self) -> bool {
        self.note != 0
    }

    /// -1.0 (flat end of the scale) … 0.0 (in tune) … 1.0 (sharp end).
    pub fn deviation(&self) -> f32 {
        (self.value as f32 - CENTRE as f32) / CENTRE as f32
    }
}

/// What the pedal sends while its tuner runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Readings {
    /// MONO and TT.
    pub mono: Reading,
    /// POLY, strings 1 (highest) to 7.
    pub strings: [Reading; 7],
}

impl Readings {
    /// Decodes the 48 bytes sent at the `TUNER` address: per reading, the note, the value as 4 nibbles,
    /// and the in-tune flag.
    pub fn parse(data: &[u8]) -> Option<Readings> {
        if data.len() < 48 {
            return None;
        }
        let reading = |c: &[u8]| Reading {
            note: c[0],
            value: c[1..5].iter().fold(0u16, |v, &n| v << 4 | (n & 0x0F) as u16),
            in_tune: c[5] == 1,
        };
        let mut chunks = data.chunks(6).map(reading);
        let mono = chunks.next()?;
        let mut strings = [Reading::default(); 7];
        for s in &mut strings {
            *s = chunks.next()?;
        }
        Some(Readings { mono, strings })
    }

    /// The 48 bytes the pedal would send for these readings.
    pub fn encode(&self) -> Vec<u8> {
        std::iter::once(&self.mono)
            .chain(&self.strings)
            .flat_map(|r| {
                let v = r.value;
                [
                    r.note,
                    (v >> 12) as u8 & 0xF,
                    (v >> 8) as u8 & 0xF,
                    (v >> 4) as u8 & 0xF,
                    v as u8 & 0xF,
                    r.in_tune as u8,
                ]
            })
            .collect()
    }
}

const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// The name of a pitch class (0 = C … 11 = B), or of a MIDI note number's pitch class.
pub fn note_name(n: u8) -> &'static str {
    NOTES[(n % 12) as usize]
}

/// The octave of a MIDI note number (60 = C4).
pub fn octave(note: u8) -> i32 {
    note as i32 / 12 - 1
}

/// The open strings of `System_pitch.PolyTunerType` (0 6-string, 1 6-string drop D, 2 7-string, 3 7-string drop A,
/// 4 4-string bass, 5 5-string bass), string 1 first, as pitch classes; `None` for a string the instrument doesn't
/// have. `offset` is `System_pitch.PolyTunerOffset` (0–5 = 5 semitones down … none).
pub fn open_strings(poly_type: i32, offset: i32) -> Vec<Option<u8>> {
    const E: i8 = 4;
    const B: i8 = 11;
    const G: i8 = 7;
    const D: i8 = 2;
    const A: i8 = 9;
    let strings: &[i8] = match poly_type {
        1 => &[E, B, G, D, A, D],
        2 => &[E, B, G, D, A, E, B],
        3 => &[E, B, G, D, A, E, A],
        4 => &[-1, -1, G, D, A, E],
        5 => &[-1, -1, G, D, A, E, B],
        _ => &[E, B, G, D, A, E],
    };
    let shift = offset - 5;
    strings.iter().map(|&n| (n >= 0).then(|| (n as i32 + 12 + shift).rem_euclid(12) as u8)).collect()
}

/// How many strings the TT tuner can aim at for this `PolyTunerType` (it picks from AUTO and these).
pub fn tt_strings(poly_type: i32) -> u8 {
    match poly_type {
        2 | 3 => 7,
        4 => 4,
        5 => 5,
        _ => 6,
    }
}

/// `Setup_temp.TTCentOffset` as the pedal sends it (one 7-bit byte, negative values from 0x7F down).
pub fn cent_offset(raw: u8) -> i32 {
    if raw >= 0x40 {
        raw as i32 - 0x80
    } else {
        raw as i32
    }
}

impl<T: Transport> Device<T> {
    /// Shows the pedal's tuner of `kind`, as the official editor's TUNER button does.
    pub fn tuner_on(&mut self, kind: Kind) -> Result<()> {
        let param = |path: &str| tables::param(path).ok_or_else(|| Error::Unknown(path.to_string()));
        self.write_param(param("System_common.TunerType")?, &ParamValue::Num(kind.type_value()))?;
        if kind != Kind::Poly {
            self.write_param(param("System_common.TunerMode")?, &ParamValue::Num(0))?;
        }
        self.send_command("RUNNING_MODE", &[kind.running_mode()])
    }

    /// Back to playing.
    pub fn tuner_off(&mut self) -> Result<()> {
        self.send_command("RUNNING_MODE", &[0])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::SimulatedGx100;
    use std::time::Duration;

    #[test]
    fn readings_roundtrip() {
        let mut r = Readings { mono: Reading { note: 45, value: 4870, in_tune: false }, ..Default::default() };
        r.strings[0] = Reading { note: 64, value: 5000, in_tune: true };
        let bytes = r.encode();
        assert_eq!(bytes.len(), 48);
        assert_eq!(&bytes[..6], &[45, 0x1, 0x3, 0x0, 0x6, 0]);
        assert_eq!(Readings::parse(&bytes), Some(r));
        assert!(Readings::parse(&bytes[..47]).is_none());
        assert!(r.mono.deviation() < 0.0 && r.strings[0].deviation() == 0.0);
        assert!(!r.strings[1].has_signal());
    }

    #[test]
    fn notes_and_strings() {
        assert_eq!((note_name(45), octave(45)), ("A", 2));
        assert_eq!((note_name(60), octave(60)), ("C", 4));
        let six: Vec<_> = open_strings(0, 5).iter().map(|n| n.map(note_name)).collect();
        assert_eq!(six, ["E", "B", "G", "D", "A", "E"].map(Some));
        // one semitone down
        assert_eq!(open_strings(0, 4)[0].map(note_name), Some("D#"));
        // 4-string bass: strings 1 and 2 don't exist
        assert_eq!(open_strings(4, 5)[..3], [None, None, Some(7)]);
        assert_eq!(open_strings(2, 5).len(), 7);
        assert_eq!((tt_strings(0), tt_strings(3), tt_strings(4), tt_strings(5)), (6, 7, 4, 5));
        assert_eq!((cent_offset(0x7E), cent_offset(0x7F), cent_offset(3)), (-2, -1, 3));
        assert_eq!(
            (Kind::from_type(0), Kind::from_type(1), Kind::from_type(3)),
            (Kind::Poly, Kind::Mono, Kind::TrueTemperament)
        );
    }

    #[test]
    fn on_and_off_send_what_the_official_editor_sends() {
        if tables::command("RUNNING_MODE").is_none() || tables::param("System_common.TunerType").is_none() {
            return; // fixture tables
        }
        let mut d = Device::new(SimulatedGx100::new()).without_pacing().with_timeout(Duration::from_millis(50));
        d.tuner_on(Kind::TrueTemperament).unwrap();
        d.tuner_on(Kind::Poly).unwrap();
        d.tuner_off().unwrap();
        let sent: Vec<(String, Vec<u8>)> = d
            .transport()
            .sent()
            .iter()
            .map(|m| {
                let addr = crate::Address::from_wire([m[9], m[10], m[11], m[12]]);
                (addr.to_string(), m[13..m.len() - 2].to_vec())
            })
            .collect();
        let want = [
            ("00 00 00 07", 3), // TunerType TT
            ("00 00 00 06", 0), // TunerMode
            ("7F 00 00 02", 1), // RUNNING_MODE
            ("00 00 00 07", 2), // POLY: no TunerMode
            ("7F 00 00 02", 2),
            ("7F 00 00 02", 0), // off
        ];
        let want: Vec<(String, Vec<u8>)> = want.iter().map(|&(a, v)| (a.to_string(), vec![v])).collect();
        assert_eq!(sent, want);
    }
}
