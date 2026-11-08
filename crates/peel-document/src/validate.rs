//! Reference and range validation. Untrusted projects are validated before
//! they reach the engine; transactions are validated in debug builds.

use std::collections::HashSet;

use crate::{ClipContent, FORMAT, Id, Project};

#[derive(Clone, Debug, PartialEq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Issue {
    pub severity: Severity,
    pub message: String,
}

impl Issue {
    fn error(message: impl Into<String>) -> Issue {
        Issue {
            severity: Severity::Error,
            message: message.into(),
        }
    }

    fn warning(message: impl Into<String>) -> Issue {
        Issue {
            severity: Severity::Warning,
            message: message.into(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Checks identity uniqueness, references and value ranges.
pub fn validate(p: &Project) -> Vec<Issue> {
    let mut issues = Vec::new();
    if p.format != FORMAT {
        issues.push(Issue::error(format!("unexpected format {:?}", p.format)));
    }
    let mut ids: HashSet<Id> = HashSet::new();
    let mut unique = |id: Id, what: &str, issues: &mut Vec<Issue>| {
        if id.is_nil() || !ids.insert(id) {
            issues.push(Issue::error(format!(
                "duplicate or nil id for {what}: {id}"
            )));
        }
    };

    if p.mixer.tracks.is_empty() {
        issues.push(Issue::error("mixer has no master track"));
    }
    let track_ids: HashSet<Id> = p.mixer.tracks.iter().map(|t| t.id).collect();
    for t in &p.mixer.tracks {
        unique(t.id, "mixer track", &mut issues);
        for d in &t.inserts {
            unique(d.id, "insert", &mut issues);
        }
        if let Some(out) = t.output {
            if !track_ids.contains(&out) {
                issues.push(Issue::error(format!(
                    "mixer track {} routes to a missing track",
                    t.name
                )));
            }
            if out == t.id {
                issues.push(Issue::error(format!(
                    "mixer track {} routes to itself",
                    t.name
                )));
            }
        }
    }
    if let Some(cycle) = routing_cycle(p) {
        issues.push(Issue::error(format!("mixer routing cycle through {cycle}")));
    }

    let channel_ids: HashSet<Id> = p.channels.iter().map(|c| c.id).collect();
    for c in &p.channels {
        unique(c.id, "channel", &mut issues);
        unique(c.device.id, "channel device", &mut issues);
        if !track_ids.contains(&c.track) {
            issues.push(Issue::error(format!(
                "channel {} routes to a missing mixer track",
                c.name
            )));
        }
        if let Some(asset) = c.device.sample
            && p.asset(asset).is_none()
        {
            issues.push(Issue::warning(format!(
                "channel {} references a missing sample",
                c.name
            )));
        }
    }

    let pattern_ids: HashSet<Id> = p.patterns.iter().map(|pt| pt.id).collect();
    for pt in &p.patterns {
        unique(pt.id, "pattern", &mut issues);
        if !pt.length.is_positive() {
            issues.push(Issue::error(format!(
                "pattern {} has a non-positive length",
                pt.name
            )));
        }
        for n in &pt.notes {
            unique(n.id, "note", &mut issues);
            if !channel_ids.contains(&n.channel) {
                issues.push(Issue::error(format!(
                    "pattern {} has a note for a missing channel",
                    pt.name
                )));
            }
            if !n.length.is_positive() {
                issues.push(Issue::error(format!(
                    "pattern {} has a note with non-positive length",
                    pt.name
                )));
            }
            if n.key > 127 || !(0.0..=1.0).contains(&n.velocity) {
                issues.push(Issue::error(format!(
                    "pattern {} has a note out of range",
                    pt.name
                )));
            }
        }
    }

    if p.arrangements.is_empty() {
        issues.push(Issue::error("project has no arrangement"));
    }
    for a in &p.arrangements {
        unique(a.id, "arrangement", &mut issues);
        let lanes: HashSet<Id> = a.tracks.iter().map(|t| t.id).collect();
        for t in &a.tracks {
            unique(t.id, "playlist track", &mut issues);
        }
        for c in &a.clips {
            unique(c.id, "clip", &mut issues);
            if !lanes.contains(&c.track) {
                issues.push(Issue::error("clip on a missing playlist track"));
            }
            if !c.length.is_positive() {
                issues.push(Issue::error("clip with non-positive length"));
            }
            match &c.content {
                ClipContent::Pattern { pattern } => {
                    if !pattern_ids.contains(pattern) {
                        issues.push(Issue::error("clip references a missing pattern"));
                    }
                }
                ClipContent::Audio { asset, track, .. } => {
                    if p.asset(*asset).is_none() {
                        issues.push(Issue::warning("audio clip references a missing asset"));
                    }
                    if !track_ids.contains(track) {
                        issues.push(Issue::error("audio clip routes to a missing mixer track"));
                    }
                }
            }
        }
    }
    if !p.arrangements.iter().any(|a| a.id == p.active_arrangement) {
        issues.push(Issue::warning("active arrangement is missing"));
    }
    for a in &p.assets {
        unique(a.id, "asset", &mut issues);
    }
    issues
}

/// Name of a track on a routing cycle, if any.
fn routing_cycle(p: &Project) -> Option<String> {
    let master = p.mixer.tracks.first()?.id;
    for start in &p.mixer.tracks {
        let mut seen = HashSet::new();
        let mut cur = start.id;
        while cur != master {
            if !seen.insert(cur) {
                return Some(start.name.clone());
            }
            let t = p.mixer.track(cur)?;
            cur = t.output.unwrap_or(master);
        }
    }
    None
}
