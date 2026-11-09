use peel_compose::{ArpOrder, StrumDirection, Tool};
use peel_document::{Id, Note, Project};
use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NoteTool {
    Legato {
        gap: String,
    },
    Gate {
        amount: f32,
    },
    Chop {
        division: String,
        gate: f32,
    },
    Arpeggiate {
        step: String,
        order: Order,
        octaves: u8,
        gate: f32,
        seed: u64,
    },
    Strum {
        spread: String,
        direction: Direction,
        velocity: f32,
    },
    Flam {
        count: u8,
        spacing: String,
        velocity: f32,
    },
    Humanize {
        timing: String,
        velocity: f32,
        length: f32,
        seed: u64,
    },
    Levels {
        multiply: f32,
        offset: f32,
    },
    Flip {
        time: bool,
        pitch: bool,
    },
    Tidy,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Order {
    Up,
    Down,
    UpDown,
    Random,
}
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Up,
    Down,
    Alternate,
}

pub fn transform(
    p: &mut Project,
    pattern: Id,
    selection: Option<Vec<String>>,
    input: NoteTool,
) -> Result<&'static str, String> {
    let pattern = p.pattern_mut(pattern).ok_or("Pattern does not exist")?;
    let selection = selection
        .map(|ids| {
            ids.into_iter()
                .map(|s| super::control::id(&s))
                .collect::<Result<std::collections::HashSet<_>, _>>()
        })
        .transpose()?;
    if selection.as_ref().is_some_and(|ids| {
        ids.iter()
            .any(|id| !pattern.notes.iter().any(|n| n.id == *id))
    }) {
        return Err("Selected note does not exist in this pattern".into());
    }
    let notes: Vec<Note> = pattern
        .notes
        .iter()
        .filter(|n| selection.as_ref().is_none_or(|ids| ids.contains(&n.id)))
        .copied()
        .collect();
    let unit = |v: f32| {
        if (0.0..=1.0).contains(&v) {
            Ok(v)
        } else {
            Err("Tool value must be 0..=1".to_string())
        }
    };
    let nonnegative = |s: &str| -> Result<peel_document::Time, String> {
        let t = super::control::time(s)?;
        if t.is_negative() {
            Err("Tool time must be nonnegative".into())
        } else {
            Ok(t)
        }
    };
    let tool = match input {
        NoteTool::Legato { gap } => Tool::Legato {
            gap: nonnegative(&gap)?,
        },
        NoteTool::Gate { amount } => {
            if !(0.0..=8.0).contains(&amount) {
                return Err("amount must be 0..=8".into());
            }
            Tool::Gate { amount }
        }
        NoteTool::Chop { division, gate } => Tool::Chop {
            division: nonnegative(&division)?,
            gate: unit(gate)?,
        },
        NoteTool::Arpeggiate {
            step,
            order,
            octaves,
            gate,
            seed,
        } => {
            if !(1..=8).contains(&octaves) {
                return Err("octaves must be 1..=8".into());
            }
            Tool::Arpeggiate {
                step: nonnegative(&step)?,
                order: match order {
                    Order::Up => ArpOrder::Up,
                    Order::Down => ArpOrder::Down,
                    Order::UpDown => ArpOrder::UpDown,
                    Order::Random => ArpOrder::Random,
                },
                octaves,
                gate: unit(gate)?,
                seed,
            }
        }
        NoteTool::Strum {
            spread,
            direction,
            velocity,
        } => Tool::Strum {
            spread: nonnegative(&spread)?,
            direction: match direction {
                Direction::Up => StrumDirection::Up,
                Direction::Down => StrumDirection::Down,
                Direction::Alternate => StrumDirection::Alternate,
            },
            velocity: unit(velocity)?,
        },
        NoteTool::Flam {
            count,
            spacing,
            velocity,
        } => Tool::Flam {
            count,
            spacing: nonnegative(&spacing)?,
            velocity: unit(velocity)?,
        },
        NoteTool::Humanize {
            timing,
            velocity,
            length,
            seed,
        } => Tool::Humanize {
            timing: nonnegative(&timing)?,
            velocity: unit(velocity)?,
            length: unit(length)?,
            seed,
        },
        NoteTool::Levels { multiply, offset } => {
            if !(0.0..=8.0).contains(&multiply) || !(-1.0..=1.0).contains(&offset) {
                return Err("multiply must be 0..=8 and offset -1..=1".into());
            }
            Tool::Levels { multiply, offset }
        }
        NoteTool::Flip { time, pitch } => Tool::Flip { time, pitch },
        NoteTool::Tidy => Tool::Tidy,
    };
    let bound = match &tool {
        Tool::Chop { division, .. } | Tool::Arpeggiate { step: division, .. } => notes
            .iter()
            .map(|n| (n.length.as_f64() / division.max(&peel_compose::MIN_LENGTH).as_f64()).ceil())
            .sum::<f64>(),
        Tool::Flam { count, .. } => notes.len() as f64 * (f64::from(*count) + 1.0),
        _ => notes.len() as f64,
    };
    if bound > 100_000.0 {
        return Err(
            "Tool would produce more than 100000 notes; select fewer notes or a larger division"
                .into(),
        );
    }
    let transformed = peel_compose::apply(&notes, &tool);
    let ids: std::collections::HashSet<_> = notes.iter().map(|n| n.id).collect();
    pattern.notes.retain(|n| !ids.contains(&n.id));
    pattern.notes.extend(transformed);
    pattern.sort_notes();
    Ok(tool.name())
}
