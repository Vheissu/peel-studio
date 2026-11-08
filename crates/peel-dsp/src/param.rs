/// Stable parameter identity within a device type. Never derived from the
/// order of controls on screen.
pub type ParamId = u32;

/// Display unit for a parameter's plain value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    None,
    Decibels,
    Hertz,
    Seconds,
    Milliseconds,
    Percent,
    Semitones,
    Cents,
    /// -1 (left) to 1 (right).
    Pan,
    Ratio,
    /// Quarter notes (tempo-synced times).
    Beats,
    Octaves,
}

/// Mapping between normalized control position (0..=1) and plain value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Taper {
    Linear,
    /// Exponential between `min` and `max` (both must be > 0). Use for
    /// frequency and time ranges.
    Log,
    /// `plain = min + (max - min) * norm^exp`. Exponents above 1 give finer
    /// control near `min`.
    Power(f32),
}

/// Describes one automatable control.
#[derive(Clone, Copy, Debug)]
pub struct ParamDescriptor {
    pub id: ParamId,
    /// Stable string key stored in project files, e.g. `filter.cutoff`.
    pub key: &'static str,
    /// Display name, e.g. `Cutoff`.
    pub name: &'static str,
    /// Section label, e.g. `Filter`.
    pub group: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: Unit,
    pub taper: Taper,
    /// Non-empty for enumerated parameters; the plain value is the index.
    pub choices: &'static [&'static str],
    pub automatable: bool,
}

impl ParamDescriptor {
    /// A continuous linear parameter.
    pub const fn linear(
        id: ParamId,
        key: &'static str,
        name: &'static str,
        group: &'static str,
        min: f32,
        max: f32,
        default: f32,
        unit: Unit,
    ) -> ParamDescriptor {
        ParamDescriptor {
            id,
            key,
            name,
            group,
            min,
            max,
            default,
            unit,
            taper: Taper::Linear,
            choices: &[],
            automatable: true,
        }
    }

    /// A continuous exponential parameter (frequency, time).
    pub const fn log(
        id: ParamId,
        key: &'static str,
        name: &'static str,
        group: &'static str,
        min: f32,
        max: f32,
        default: f32,
        unit: Unit,
    ) -> ParamDescriptor {
        ParamDescriptor {
            id,
            key,
            name,
            group,
            min,
            max,
            default,
            unit,
            taper: Taper::Log,
            choices: &[],
            automatable: true,
        }
    }

    /// A parameter with a power-curve taper.
    pub const fn power(
        id: ParamId,
        key: &'static str,
        name: &'static str,
        group: &'static str,
        min: f32,
        max: f32,
        default: f32,
        unit: Unit,
        exp: f32,
    ) -> ParamDescriptor {
        ParamDescriptor {
            id,
            key,
            name,
            group,
            min,
            max,
            default,
            unit,
            taper: Taper::Power(exp),
            choices: &[],
            automatable: true,
        }
    }

    /// An enumerated parameter; `default` is the default index.
    pub const fn choice(
        id: ParamId,
        key: &'static str,
        name: &'static str,
        group: &'static str,
        choices: &'static [&'static str],
        default: u32,
    ) -> ParamDescriptor {
        ParamDescriptor {
            id,
            key,
            name,
            group,
            min: 0.0,
            max: (choices.len() as f32) - 1.0,
            default: default as f32,
            unit: Unit::None,
            taper: Taper::Linear,
            choices,
            automatable: true,
        }
    }

    /// An on/off switch (0 or 1).
    pub const fn toggle(
        id: ParamId,
        key: &'static str,
        name: &'static str,
        group: &'static str,
        default: bool,
    ) -> ParamDescriptor {
        ParamDescriptor {
            id,
            key,
            name,
            group,
            min: 0.0,
            max: 1.0,
            default: if default { 1.0 } else { 0.0 },
            unit: Unit::None,
            taper: Taper::Linear,
            choices: &["Off", "On"],
            automatable: true,
        }
    }

    pub fn is_choice(&self) -> bool {
        !self.choices.is_empty()
    }

    pub fn clamp(&self, plain: f32) -> f32 {
        let v = if plain.is_finite() {
            plain
        } else {
            self.default
        };
        let v = v.clamp(self.min.min(self.max), self.max.max(self.min));
        if self.is_choice() { v.round() } else { v }
    }

    pub fn to_normalized(&self, plain: f32) -> f32 {
        let plain = self.clamp(plain);
        if self.max == self.min {
            return 0.0;
        }
        let n = match self.taper {
            Taper::Linear => (plain - self.min) / (self.max - self.min),
            Taper::Log => {
                let (lo, hi) = (self.min.max(1e-9), self.max.max(1e-9));
                (plain.max(1e-9) / lo).ln() / (hi / lo).ln()
            }
            Taper::Power(exp) => ((plain - self.min) / (self.max - self.min))
                .max(0.0)
                .powf(1.0 / exp),
        };
        n.clamp(0.0, 1.0)
    }

    pub fn from_normalized(&self, norm: f32) -> f32 {
        let n = norm.clamp(0.0, 1.0);
        let v = match self.taper {
            Taper::Linear => self.min + (self.max - self.min) * n,
            Taper::Log => {
                let (lo, hi) = (self.min.max(1e-9), self.max.max(1e-9));
                lo * (hi / lo).powf(n)
            }
            Taper::Power(exp) => self.min + (self.max - self.min) * n.powf(exp),
        };
        self.clamp(v)
    }

    /// Human-readable value with unit, using tabular-friendly precision.
    pub fn format(&self, plain: f32) -> String {
        let v = self.clamp(plain);
        if self.is_choice() {
            return self
                .choices
                .get(v as usize)
                .copied()
                .unwrap_or("?")
                .to_string();
        }
        match self.unit {
            Unit::None => format!("{v:.2}"),
            Unit::Decibels => {
                if v <= -96.0 {
                    "-inf dB".to_string()
                } else {
                    format!("{v:+.1} dB")
                }
            }
            Unit::Hertz => {
                if v >= 1000.0 {
                    format!("{:.2} kHz", v / 1000.0)
                } else if v >= 100.0 {
                    format!("{v:.0} Hz")
                } else {
                    format!("{v:.1} Hz")
                }
            }
            Unit::Seconds => {
                if v < 1.0 {
                    format!("{:.0} ms", v * 1000.0)
                } else {
                    format!("{v:.2} s")
                }
            }
            Unit::Milliseconds => format!("{v:.1} ms"),
            Unit::Percent => format!("{:.0}%", v * 100.0),
            Unit::Semitones => format!("{v:+.0} st"),
            Unit::Cents => format!("{v:+.0} ct"),
            Unit::Pan => {
                let p = (v * 100.0).round() as i32;
                match p {
                    0 => "C".to_string(),
                    p if p < 0 => format!("{}L", -p),
                    p => format!("{p}R"),
                }
            }
            Unit::Ratio => format!("{v:.1}:1"),
            Unit::Beats => format_beats(v),
            Unit::Octaves => format!("{v:+.1} oct"),
        }
    }
}

fn format_beats(v: f32) -> String {
    // Common note values as fractions of a whole note.
    const TABLE: &[(f32, &str)] = &[
        (0.125, "1/32"),
        (1.0 / 6.0, "1/16T"),
        (0.1875, "1/32D"),
        (0.25, "1/16"),
        (1.0 / 3.0, "1/8T"),
        (0.375, "1/16D"),
        (0.5, "1/8"),
        (2.0 / 3.0, "1/4T"),
        (0.75, "1/8D"),
        (1.0, "1/4"),
        (1.5, "1/4D"),
        (2.0, "1/2"),
        (3.0, "1/2D"),
        (4.0, "1 bar"),
    ];
    for (b, label) in TABLE {
        if (v - b).abs() < 1e-3 {
            return (*label).to_string();
        }
    }
    format!("{v:.2} beats")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_taper_round_trips() {
        let p = ParamDescriptor::log(
            1,
            "cutoff",
            "Cutoff",
            "Filter",
            20.0,
            20_000.0,
            1000.0,
            Unit::Hertz,
        );
        for v in [20.0, 100.0, 1000.0, 19_999.0] {
            let n = p.to_normalized(v);
            assert!((p.from_normalized(n) - v).abs() / v < 1e-3);
        }
        assert!((p.to_normalized(632.456) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn choices_clamp_and_format() {
        let p = ParamDescriptor::choice(2, "wave", "Wave", "Osc", &["Saw", "Square", "Sine"], 0);
        assert_eq!(p.clamp(1.6), 2.0);
        assert_eq!(p.format(1.0), "Square");
        assert_eq!(p.max, 2.0);
    }

    #[test]
    fn formats_units() {
        let pan = ParamDescriptor::linear(3, "pan", "Pan", "Mix", -1.0, 1.0, 0.0, Unit::Pan);
        assert_eq!(pan.format(0.0), "C");
        assert_eq!(pan.format(-0.5), "50L");
        let db =
            ParamDescriptor::linear(4, "gain", "Gain", "Mix", -60.0, 12.0, 0.0, Unit::Decibels);
        assert_eq!(db.format(-6.0), "-6.0 dB");
        assert_eq!(db.format(f32::NAN), "+0.0 dB");
    }
}
