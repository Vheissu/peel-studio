//! Scales and chord shapes for the Piano Roll.

pub const NOTE_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

pub const SCALES: &[(&str, &[u8])] = &[
    ("Major", &[0, 2, 4, 5, 7, 9, 11]),
    ("Minor", &[0, 2, 3, 5, 7, 8, 10]),
    ("Dorian", &[0, 2, 3, 5, 7, 9, 10]),
    ("Phrygian", &[0, 1, 3, 5, 7, 8, 10]),
    ("Lydian", &[0, 2, 4, 6, 7, 9, 11]),
    ("Mixolydian", &[0, 2, 4, 5, 7, 9, 10]),
    ("Harmonic minor", &[0, 2, 3, 5, 7, 8, 11]),
    ("Major pentatonic", &[0, 2, 4, 7, 9]),
    ("Minor pentatonic", &[0, 3, 5, 7, 10]),
    ("Blues", &[0, 3, 5, 6, 7, 10]),
];

/// Chord shapes. Diatonic shapes stack scale degrees when a scale is set
/// and fall back to the fixed intervals otherwise.
pub enum Shape {
    Fixed(&'static [i32]),
    Diatonic(usize),
}

pub const CHORDS: &[(&str, Shape)] = &[
    ("Single note", Shape::Fixed(&[0])),
    ("Triad in scale", Shape::Diatonic(3)),
    ("Seventh in scale", Shape::Diatonic(4)),
    ("Major", Shape::Fixed(&[0, 4, 7])),
    ("Minor", Shape::Fixed(&[0, 3, 7])),
    ("Dominant 7", Shape::Fixed(&[0, 4, 7, 10])),
    ("Major 7", Shape::Fixed(&[0, 4, 7, 11])),
    ("Minor 7", Shape::Fixed(&[0, 3, 7, 10])),
    ("Sus2", Shape::Fixed(&[0, 2, 7])),
    ("Sus4", Shape::Fixed(&[0, 5, 7])),
    ("Power", Shape::Fixed(&[0, 7, 12])),
    ("Octave", Shape::Fixed(&[0, 12])),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scale {
    pub root: u8,
    pub kind: usize,
}

impl Scale {
    pub fn steps(&self) -> &'static [u8] {
        SCALES[self.kind.min(SCALES.len() - 1)].1
    }

    pub fn name(&self) -> String {
        format!(
            "{} {}",
            NOTE_NAMES[(self.root % 12) as usize],
            SCALES[self.kind.min(SCALES.len() - 1)].0
        )
    }

    pub fn contains(&self, key: u8) -> bool {
        let pc = (key as i32 - self.root as i32).rem_euclid(12) as u8;
        self.steps().contains(&pc)
    }

    /// Nearest key in the scale (ties go down).
    pub fn snap(&self, key: i32) -> i32 {
        for d in 0..12 {
            for k in [key - d, key + d] {
                if (0..=127).contains(&k) && self.contains(k as u8) {
                    return k;
                }
            }
        }
        key
    }

    /// The scale degree `n` steps above `key` (which should be in scale).
    pub fn step_up(&self, key: i32, n: usize) -> i32 {
        let mut k = key;
        let mut count = 0;
        while count < n && k < 127 {
            k += 1;
            if self.contains(k as u8) {
                count += 1;
            }
        }
        k
    }
}

/// Keys for a chord rooted at `key`.
pub fn chord_keys(shape: usize, key: i32, scale: Option<Scale>) -> Vec<u8> {
    let keys: Vec<i32> = match (&CHORDS[shape.min(CHORDS.len() - 1)].1, scale) {
        (Shape::Diatonic(n), Some(sc)) => {
            let root = sc.snap(key);
            (0..*n).map(|i| sc.step_up(root, i * 2)).collect()
        }
        (Shape::Diatonic(n), None) => [0, 4, 7, 10][..*n].iter().map(|i| key + i).collect(),
        (Shape::Fixed(iv), _) => iv.iter().map(|i| key + i).collect(),
    };
    keys.into_iter()
        .filter(|k| (0..=127).contains(k))
        .map(|k| k as u8)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_membership_and_snap() {
        let a_minor = Scale { root: 9, kind: 1 };
        assert!(a_minor.contains(57)); // A3
        assert!(a_minor.contains(60)); // C4
        assert!(!a_minor.contains(61)); // C#4
        assert_eq!(a_minor.snap(61), 60);
        assert_eq!(a_minor.snap(63), 62);
    }

    #[test]
    fn diatonic_triads_follow_the_scale() {
        let c_major = Scale { root: 0, kind: 0 };
        assert_eq!(chord_keys(1, 60, Some(c_major)), vec![60, 64, 67]); // C E G
        assert_eq!(chord_keys(1, 62, Some(c_major)), vec![62, 65, 69]); // D F A
        assert_eq!(chord_keys(2, 67, Some(c_major)), vec![67, 71, 74, 77]); // G7
        assert_eq!(chord_keys(4, 57, None), vec![57, 60, 64]); // A minor
    }
}
