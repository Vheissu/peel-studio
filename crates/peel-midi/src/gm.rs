//! General MIDI names and the default drum-to-device mapping.
//!
//! Names are the standard GM Level 1 program and percussion lists. The
//! device ids returned by [`default_drum_device`] are plain strings; this
//! crate does not depend on the device registry, so the caller can replace
//! the mapping through `ImportOptions::drum_device_for_key`.

/// GM Level 1 program names, indexed by program number 0-127.
pub const PROGRAM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano",
    "Bright Acoustic Piano",
    "Electric Grand Piano",
    "Honky-tonk Piano",
    "Electric Piano 1",
    "Electric Piano 2",
    "Harpsichord",
    "Clavinet",
    "Celesta",
    "Glockenspiel",
    "Music Box",
    "Vibraphone",
    "Marimba",
    "Xylophone",
    "Tubular Bells",
    "Dulcimer",
    "Drawbar Organ",
    "Percussive Organ",
    "Rock Organ",
    "Church Organ",
    "Reed Organ",
    "Accordion",
    "Harmonica",
    "Tango Accordion",
    "Acoustic Guitar (nylon)",
    "Acoustic Guitar (steel)",
    "Electric Guitar (jazz)",
    "Electric Guitar (clean)",
    "Electric Guitar (muted)",
    "Overdriven Guitar",
    "Distortion Guitar",
    "Guitar Harmonics",
    "Acoustic Bass",
    "Electric Bass (finger)",
    "Electric Bass (pick)",
    "Fretless Bass",
    "Slap Bass 1",
    "Slap Bass 2",
    "Synth Bass 1",
    "Synth Bass 2",
    "Violin",
    "Viola",
    "Cello",
    "Contrabass",
    "Tremolo Strings",
    "Pizzicato Strings",
    "Orchestral Harp",
    "Timpani",
    "String Ensemble 1",
    "String Ensemble 2",
    "Synth Strings 1",
    "Synth Strings 2",
    "Choir Aahs",
    "Voice Oohs",
    "Synth Voice",
    "Orchestra Hit",
    "Trumpet",
    "Trombone",
    "Tuba",
    "Muted Trumpet",
    "French Horn",
    "Brass Section",
    "Synth Brass 1",
    "Synth Brass 2",
    "Soprano Sax",
    "Alto Sax",
    "Tenor Sax",
    "Baritone Sax",
    "Oboe",
    "English Horn",
    "Bassoon",
    "Clarinet",
    "Piccolo",
    "Flute",
    "Recorder",
    "Pan Flute",
    "Blown Bottle",
    "Shakuhachi",
    "Whistle",
    "Ocarina",
    "Lead 1 (square)",
    "Lead 2 (sawtooth)",
    "Lead 3 (calliope)",
    "Lead 4 (chiff)",
    "Lead 5 (charang)",
    "Lead 6 (voice)",
    "Lead 7 (fifths)",
    "Lead 8 (bass + lead)",
    "Pad 1 (new age)",
    "Pad 2 (warm)",
    "Pad 3 (polysynth)",
    "Pad 4 (choir)",
    "Pad 5 (bowed)",
    "Pad 6 (metallic)",
    "Pad 7 (halo)",
    "Pad 8 (sweep)",
    "FX 1 (rain)",
    "FX 2 (soundtrack)",
    "FX 3 (crystal)",
    "FX 4 (atmosphere)",
    "FX 5 (brightness)",
    "FX 6 (goblins)",
    "FX 7 (echoes)",
    "FX 8 (sci-fi)",
    "Sitar",
    "Banjo",
    "Shamisen",
    "Koto",
    "Kalimba",
    "Bag pipe",
    "Fiddle",
    "Shanai",
    "Tinkle Bell",
    "Agogo",
    "Steel Drums",
    "Woodblock",
    "Taiko Drum",
    "Melodic Tom",
    "Synth Drum",
    "Reverse Cymbal",
    "Guitar Fret Noise",
    "Breath Noise",
    "Seashore",
    "Bird Tweet",
    "Telephone Ring",
    "Helicopter",
    "Applause",
    "Gunshot",
];

/// Name of a GM program, or `"Program N"` out of range.
pub fn program_name(program: u8) -> &'static str {
    PROGRAM_NAMES
        .get(usize::from(program))
        .copied()
        .unwrap_or("Unknown program")
}

/// GM percussion names for keys 35-81.
const DRUM_NAMES: [&str; 47] = [
    "Acoustic Bass Drum",
    "Bass Drum 1",
    "Side Stick",
    "Acoustic Snare",
    "Hand Clap",
    "Electric Snare",
    "Low Floor Tom",
    "Closed Hi-Hat",
    "High Floor Tom",
    "Pedal Hi-Hat",
    "Low Tom",
    "Open Hi-Hat",
    "Low-Mid Tom",
    "Hi-Mid Tom",
    "Crash Cymbal 1",
    "High Tom",
    "Ride Cymbal 1",
    "Chinese Cymbal",
    "Ride Bell",
    "Tambourine",
    "Splash Cymbal",
    "Cowbell",
    "Crash Cymbal 2",
    "Vibraslap",
    "Ride Cymbal 2",
    "Hi Bongo",
    "Low Bongo",
    "Mute Hi Conga",
    "Open Hi Conga",
    "Low Conga",
    "High Timbale",
    "Low Timbale",
    "High Agogo",
    "Low Agogo",
    "Cabasa",
    "Maracas",
    "Short Whistle",
    "Long Whistle",
    "Short Guiro",
    "Long Guiro",
    "Claves",
    "Hi Wood Block",
    "Low Wood Block",
    "Mute Cuica",
    "Open Cuica",
    "Mute Triangle",
    "Open Triangle",
];

/// GM percussion name for a key, or `"Percussion N"` outside 35-81.
pub fn drum_name(key: u8) -> String {
    match key
        .checked_sub(35)
        .and_then(|i| DRUM_NAMES.get(usize::from(i)))
    {
        Some(name) => (*name).to_string(),
        None => format!("Percussion {key}"),
    }
}

/// Default mapping from a GM drum key to a native device family.
///
/// The ids follow the native drum instruments in the roadmap (kick, snare,
/// hat, clap, tom). Anything without a close match uses the hat, the most
/// neutral short percussive sound.
pub fn default_drum_device(key: u8) -> (String, u32, String) {
    let family = match key {
        35 | 36 => "kick",
        37 | 38 | 40 => "snare",
        39 => "clap",
        41 | 43 | 45 | 47 | 48 | 50 | 60 | 61 | 64 | 65 | 66 => "tom",
        _ => "hat",
    };
    (format!("peel.drum.{family}"), 1, drum_name(key))
}

/// Default melodic device for imported tracks.
pub fn default_melodic_device() -> (String, u32) {
    ("peel.synth.poly".to_string(), 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_are_complete_and_ordered() {
        assert_eq!(program_name(0), "Acoustic Grand Piano");
        assert_eq!(program_name(24), "Acoustic Guitar (nylon)");
        assert_eq!(program_name(40), "Violin");
        assert_eq!(program_name(127), "Gunshot");
        assert_eq!(drum_name(35), "Acoustic Bass Drum");
        assert_eq!(drum_name(42), "Closed Hi-Hat");
        assert_eq!(drum_name(81), "Open Triangle");
        assert_eq!(drum_name(82), "Percussion 82");
        assert_eq!(drum_name(10), "Percussion 10");
    }

    #[test]
    fn drum_families() {
        assert_eq!(default_drum_device(36).0, "peel.drum.kick");
        assert_eq!(default_drum_device(38).0, "peel.drum.snare");
        assert_eq!(default_drum_device(42).0, "peel.drum.hat");
        assert_eq!(default_drum_device(39).0, "peel.drum.clap");
        assert_eq!(default_drum_device(45).0, "peel.drum.tom");
        assert_eq!(default_drum_device(99).0, "peel.drum.hat");
    }
}
