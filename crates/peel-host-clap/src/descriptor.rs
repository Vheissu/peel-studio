//! `DeviceDescriptor`s for CLAP plugins.
//!
//! The `Device` trait wants a `&'static DeviceDescriptor`, but a CLAP
//! plugin's parameters are only known after it is created. The descriptor is
//! built once from the live plugin and leaked. A cache keeps the leak bounded:
//! loading the same plugin again returns the same descriptor as long as its
//! parameter list is unchanged, so memory grows by one descriptor per distinct
//! plugin layout rather than per instance.

use std::sync::{Mutex, PoisonError};

use clap_sys::ext::params::CLAP_PARAM_IS_AUTOMATABLE;
use peel_dsp::{DeviceDescriptor, DeviceKind, ParamDescriptor, Taper, Unit};

use crate::instance::{PluginShared, RawParam};
use crate::thread::RoleScope;

/// Stepped parameters with at most this many values become choices.
const MAX_CHOICES: usize = 64;

#[derive(Clone, Debug, PartialEq)]
struct ParamSpec {
    id: u32,
    name: String,
    group: String,
    min: f32,
    max: f32,
    default: f32,
    automatable: bool,
    choices: Vec<String>,
}

/// Everything a descriptor is built from, before any of it is leaked.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DescriptorSpec {
    type_id: String,
    name: String,
    description: String,
    kind: DeviceKind,
    params: Vec<ParamSpec>,
}

impl DescriptorSpec {
    /// Reads names, ranges and choice texts from a freshly opened plugin.
    /// Main thread only.
    pub(crate) fn from_plugin(
        shared: &PluginShared,
        plugin_id: &str,
        name: &str,
        description: &str,
        kind: DeviceKind,
    ) -> DescriptorSpec {
        // The plugin's text formatter is a main-thread function.
        let _main = RoleScope::main();
        DescriptorSpec {
            type_id: format!("clap:{plugin_id}"),
            name: name.to_string(),
            description: description.to_string(),
            kind,
            params: shared
                .exposed
                .iter()
                .map(|p| param_spec(shared, p))
                .collect(),
        }
    }

    fn matches(&self, d: &DeviceDescriptor) -> bool {
        self.type_id == d.type_id
            && self.name == d.name
            && self.description == d.description
            && self.kind == d.kind
            && self.params.len() == d.params.len()
            && self.params.iter().zip(d.params).all(|(s, p)| {
                s.id == p.id
                    && s.name == p.name
                    && s.group == p.group
                    && s.min == p.min
                    && s.max == p.max
                    && s.default == p.default
                    && s.automatable == p.automatable
                    && s.choices
                        .iter()
                        .map(String::as_str)
                        .eq(p.choices.iter().copied())
            })
    }

    fn leak(self) -> &'static DeviceDescriptor {
        fn leak_str(s: String) -> &'static str {
            Box::leak(s.into_boxed_str())
        }
        let params: Vec<ParamDescriptor> = self
            .params
            .into_iter()
            .map(|p| {
                let choices: Vec<&'static str> = p.choices.into_iter().map(leak_str).collect();
                ParamDescriptor {
                    id: p.id,
                    // The key is the CLAP id in decimal: stable across plugin
                    // versions, unlike display names.
                    key: leak_str(p.id.to_string()),
                    name: leak_str(p.name),
                    group: leak_str(p.group),
                    min: p.min,
                    max: p.max,
                    default: p.default,
                    unit: Unit::None,
                    taper: Taper::Linear,
                    choices: Box::leak(choices.into_boxed_slice()),
                    automatable: p.automatable,
                }
            })
            .collect();
        Box::leak(Box::new(DeviceDescriptor {
            type_id: leak_str(self.type_id),
            name: leak_str(self.name),
            description: leak_str(self.description),
            category: "CLAP",
            kind: self.kind,
            version: 1,
            params: Box::leak(params.into_boxed_slice()),
        }))
    }
}

static CACHE: Mutex<Vec<&'static DeviceDescriptor>> = Mutex::new(Vec::new());

/// The most recently loaded descriptor for a CLAP device type id
/// (`clap:<plugin id>`), if an instance of it has been loaded.
pub fn cached_descriptor(type_id: &str) -> Option<&'static DeviceDescriptor> {
    let cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache.iter().rev().find(|d| d.type_id == type_id).copied()
}

/// Returns the cached descriptor equal to `spec`, leaking a new one only if
/// the plugin's layout has not been seen before.
pub(crate) fn intern(spec: DescriptorSpec) -> &'static DeviceDescriptor {
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(found) = cache.iter().find(|d| spec.matches(d)) {
        return found;
    }
    let descriptor = spec.leak();
    cache.push(descriptor);
    descriptor
}

fn param_spec(shared: &PluginShared, p: &RawParam) -> ParamSpec {
    let group = if p.module.trim().is_empty() {
        "Parameters".to_string()
    } else {
        p.module.replace('/', " / ")
    };
    let choices = choice_labels(shared, p);
    let (min, max) = if choices.is_empty() {
        (p.min as f32, p.max as f32)
    } else {
        // Choices are addressed by index from zero.
        (0.0, (choices.len() - 1) as f32)
    };
    ParamSpec {
        id: p.id,
        name: if p.name.is_empty() {
            format!("Parameter {}", p.id)
        } else {
            p.name.clone()
        },
        group,
        min,
        max,
        default: (p.default as f32).clamp(min, max),
        automatable: p.flags & CLAP_PARAM_IS_AUTOMATABLE != 0,
        choices,
    }
}

/// Labels for a stepped parameter that counts from zero, taken from the
/// plugin's own `value_to_text`. Empty means "not a choice".
fn choice_labels(shared: &PluginShared, p: &RawParam) -> Vec<String> {
    if !p.is_stepped() || p.min != 0.0 || p.max < 1.0 || p.max.fract() != 0.0 {
        return Vec::new();
    }
    let count = p.max as usize + 1;
    if count > MAX_CHOICES {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            shared
                .param_text_unlocked(p.id, i as f64)
                .unwrap_or_else(|| match (count, i) {
                    (2, 0) => "Off".to_string(),
                    (2, _) => "On".to_string(),
                    _ => i.to_string(),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, choices: &[&str]) -> DescriptorSpec {
        DescriptorSpec {
            type_id: "clap:test.descriptor".into(),
            name: name.into(),
            description: "d".into(),
            kind: DeviceKind::Effect,
            params: vec![ParamSpec {
                id: 3,
                name: "Mode".into(),
                group: "Main".into(),
                min: 0.0,
                max: choices.len().saturating_sub(1) as f32,
                default: 0.0,
                automatable: true,
                choices: choices.iter().map(|s| s.to_string()).collect(),
            }],
        }
    }

    #[test]
    fn equal_layouts_share_one_leaked_descriptor() {
        let a = intern(spec("Cached", &["A", "B"]));
        let b = intern(spec("Cached", &["A", "B"]));
        assert!(std::ptr::eq(a, b));
        let c = intern(spec("Cached", &["A", "B", "C"]));
        assert!(!std::ptr::eq(a, c));
        assert_eq!(c.params[0].choices, ["A", "B", "C"]);
    }

    #[test]
    fn keys_are_the_decimal_clap_id() {
        let d = intern(spec("Keyed", &[]));
        assert_eq!(d.type_id, "clap:test.descriptor");
        assert_eq!(d.params[0].key, "3");
        assert_eq!(d.param_by_key("3").map(|p| p.id), Some(3));
    }
}
