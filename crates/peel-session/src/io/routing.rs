//! Hardware monitor/cue routes. These belong to the machine, not the project.

use peel_document::Id;
use peel_engine::output::{MAX_OUTPUT_ROUTES, OutputRouteRequest, OutputSource};
use serde::{Deserialize, Serialize};

use crate::Session;
pub use peel_engine::output::OutputChannels;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRoute {
    pub name: String,
    /// None taps the master mix. A track taps its post-insert, post-fader signal.
    pub track: Option<Id>,
    pub device: String,
    pub channels: OutputChannels,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default)]
    pub muted: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct OutputRouteStatus {
    pub name: String,
    pub device: String,
    pub sample_rate: u32,
    pub channels: OutputChannels,
    pub healthy: bool,
    pub underruns: u64,
    pub overruns: u64,
    pub buffered_frames: usize,
    /// Approximate FIFO plus output-device delay, not phase alignment.
    pub latency_ms: f32,
    pub buffered_latency_ms: f32,
    pub output_latency_ms: f32,
    pub clock_correction_ppm: f32,
}

#[derive(Default)]
pub(crate) struct RoutingState {
    routes: Vec<OutputRoute>,
    primary: Option<OutputChannels>,
}

impl Session {
    /// Replaces every secondary output together. No current route changes
    /// unless every route validates and every hardware stream opens.
    ///
    /// Separate devices are drift-corrected cue feeds, not phase-synchronous
    /// channels. Use an OS aggregate device for synchronous multichannel audio.
    pub fn configure_output_routes(&mut self, mut routes: Vec<OutputRoute>) -> Result<(), String> {
        if routes.len() > MAX_OUTPUT_ROUTES {
            return Err(format!(
                "At most {MAX_OUTPUT_ROUTES} secondary outputs are supported"
            ));
        }
        // Validate the whole request before opening a device or editing state.
        for (index, route) in routes.iter().enumerate() {
            if route.track.is_some_and(|id| {
                self.project().mixer.track(id).is_none()
                    && !self
                        .io
                        .output_routes
                        .routes
                        .iter()
                        .any(|old| old.track == Some(id))
            }) {
                return Err(format!(
                    "Output route {} references a missing mixer track",
                    index + 1
                ));
            }
            if route.device.trim().is_empty() {
                return Err("Choose a secondary output device".into());
            }
            if !route.gain_db.is_finite() || !(-90.0..=12.0).contains(&route.gain_db) {
                return Err("Output route gain must be between -90 and +12 dB".into());
            }
            route.channels.validate(u16::MAX)?;
            for other in &routes[..index] {
                if route.device == other.device && channels_overlap(route.channels, other.channels)
                {
                    return Err(
                        "Two secondary routes cannot use the same physical output channels".into(),
                    );
                }
            }
        }
        self.sync();
        let requests = self.output_route_requests(&routes, false)?;
        self.engine.set_output_routes(&requests)?;
        for (index, route) in routes.iter_mut().enumerate() {
            if route.name.trim().is_empty() {
                route.name = format!("Output {}", index + 1);
            }
        }
        self.io.output_routes.routes = routes;
        Ok(())
    }

    pub fn output_routes(&self) -> &[OutputRoute] {
        &self.io.output_routes.routes
    }

    pub fn output_route_status(&self) -> Vec<OutputRouteStatus> {
        let infos = self.engine.output_route_info();
        self.io
            .output_routes
            .routes
            .iter()
            .enumerate()
            .map(|(index, route)| {
                let info = infos.get(index);
                OutputRouteStatus {
                    name: route.name.clone(),
                    device: info.map_or_else(|| route.device.clone(), |i| i.device.clone()),
                    sample_rate: info.map_or(0, |i| i.sample_rate),
                    channels: route.channels,
                    healthy: info.is_some_and(|i| i.healthy)
                        && route.track.is_none_or(|id| {
                            self.project().mixer.track(id).is_some()
                                && self.meters.contains_key(&id)
                        }),
                    underruns: info.map_or(0, |i| i.underruns),
                    overruns: info.map_or(0, |i| i.overruns),
                    buffered_frames: info.map_or(0, |i| i.buffered_frames),
                    latency_ms: info.map_or(0.0, |i| i.latency_ms),
                    buffered_latency_ms: info.map_or(0.0, |i| i.buffered_latency_ms),
                    output_latency_ms: info.map_or(0.0, |i| i.output_latency_ms),
                    clock_correction_ppm: info.map_or(0.0, |i| i.clock_correction_ppm),
                }
            })
            .collect()
    }

    fn output_route_requests(
        &self,
        routes: &[OutputRoute],
        reopening: bool,
    ) -> Result<Vec<OutputRouteRequest>, String> {
        routes
            .iter()
            .map(|route| {
                let source = match route.track {
                    None => OutputSource::Master,
                    Some(id) if self.project().mixer.track(id).is_none() => OutputSource::Disabled,
                    Some(id) => match self.meters.get(&id).copied() {
                        Some(meter) => OutputSource::TrackMeter(meter),
                        None if reopening => OutputSource::Disabled,
                        None => {
                            return Err(
                                "Output source mixer track exceeds available meter capacity".into(),
                            );
                        }
                    },
                };
                Ok(OutputRouteRequest {
                    source,
                    device: route.device.clone(),
                    channels: route.channels,
                    gain_db: route.gain_db,
                    muted: route.muted,
                })
            })
            .collect()
    }

    pub fn primary_output_channels(&self) -> OutputChannels {
        self.engine.primary_output_channels()
    }
    pub fn primary_output_channel_count(&self) -> u16 {
        self.engine.primary_output_channel_count()
    }
    pub fn set_primary_output_channels(&mut self, channels: OutputChannels) -> Result<(), String> {
        self.engine.set_primary_output_channels(channels)?;
        self.io.output_routes.primary = Some(channels);
        Ok(())
    }

    /// Called after document sync. Deleted routes stay configured but silent,
    /// and regain their source when undo restores the same stable track ID.
    pub(crate) fn refresh_output_route_sources(&self) {
        for (index, route) in self.io.output_routes.routes.iter().enumerate() {
            let source = match route.track {
                None => Some(OutputSource::Master),
                Some(id) => self
                    .project()
                    .mixer
                    .track(id)
                    .and_then(|_| self.meters.get(&id))
                    .copied()
                    .map(OutputSource::TrackMeter),
            };
            self.engine.set_output_route_source(index, source);
        }
    }

    pub(crate) fn reopen_output_routes(&mut self) {
        if let Some(channels) = self.io.output_routes.primary
            && let Err(error) = self.engine.set_primary_output_channels(channels)
        {
            self.notify(format!("Primary output channels changed: {error}"), true);
        }
        if !self.io.output_routes.routes.is_empty() {
            let routes = self.io.output_routes.routes.clone();
            let result = self
                .output_route_requests(&routes, true)
                .and_then(|requests| self.engine.set_output_routes(&requests));
            if let Err(error) = result {
                self.notify(format!("Secondary outputs need attention: {error}"), true);
            }
            self.refresh_output_route_sources();
        }
    }
}

fn channels_overlap(a: OutputChannels, b: OutputChannels) -> bool {
    let pair = |c| match c {
        OutputChannels::Mono(a) => (a, a),
        OutputChannels::Stereo(a, b) => (a, b),
    };
    let (aa, ab) = pair(a);
    let (ba, bb) = pair(b);
    aa == ba || aa == bb || ab == ba || ab == bb
}

#[cfg(test)]
mod tests {
    use super::*;
    fn route() -> OutputRoute {
        OutputRoute {
            name: "Cue".into(),
            track: None,
            device: "Not a physical test device".into(),
            channels: OutputChannels::Stereo(0, 1),
            gain_db: 0.0,
            muted: false,
        }
    }
    #[test]
    fn invalid_routes_leave_existing_configuration_and_primary_channels_untouched() {
        let mut s = Session::new_silent(crate::template::empty());
        let mut missing = route();
        missing.track = Some(Id::new());
        assert!(
            s.configure_output_routes(vec![missing])
                .unwrap_err()
                .contains("missing mixer track")
        );
        assert!(s.output_routes().is_empty());
        let mut invalid = route();
        invalid.gain_db = f32::NAN;
        assert!(s.configure_output_routes(vec![invalid]).is_err());
        assert!(s.configure_output_routes(vec![route(); 5]).is_err());
        assert!(
            s.configure_output_routes(vec![route(), route()])
                .unwrap_err()
                .contains("same physical")
        );
        assert!(
            s.configure_output_routes(vec![route()])
                .unwrap_err()
                .contains("running primary audio output")
        );
        assert!(s.output_routes().is_empty());
        assert_eq!(s.primary_output_channels(), OutputChannels::Stereo(0, 1));
        assert!(
            s.set_primary_output_channels(OutputChannels::Mono(8))
                .is_err()
        );
        assert_eq!(s.primary_output_channels(), OutputChannels::Stereo(0, 1));
        s.set_primary_output_channels(OutputChannels::Mono(1))
            .unwrap();
        assert_eq!(s.primary_output_channels(), OutputChannels::Mono(1));
        s.restart_audio(None, Some(128)).unwrap();
        assert_eq!(s.primary_output_channels(), OutputChannels::Mono(1));
        s.configure_output_routes(vec![]).unwrap();
    }
    #[test]
    fn reopening_disables_only_deleted_sources_and_undo_recovers_them() {
        let mut s = Session::new_silent(crate::template::empty());
        let mut bus = s.project().mixer.tracks[0].clone();
        bus.id = Id::new();
        bus.name = "Cue bus".into();
        bus.output = Some(s.project().mixer.tracks[0].id);
        let bus_id = bus.id;
        s.doc.edit("Add bus", |p| p.mixer.tracks.push(bus));
        s.tick();
        let mut cue = route();
        cue.track = Some(bus_id);
        let mut master = route();
        master.device = "Other simulated device".into();
        s.io.output_routes.routes = vec![cue, master];
        let requests = s.output_route_requests(s.output_routes(), true).unwrap();
        assert!(matches!(requests[0].source, OutputSource::TrackMeter(_)));
        assert_eq!(requests[1].source, OutputSource::Master);
        s.doc
            .edit("Delete bus", |p| p.mixer.tracks.retain(|t| t.id != bus_id));
        s.tick();
        let requests = s.output_route_requests(s.output_routes(), true).unwrap();
        assert_eq!(requests[0].source, OutputSource::Disabled);
        assert_eq!(requests[1].source, OutputSource::Master);
        // A failed hardware reopen still leaves configured routes visible.
        s.reopen_output_routes();
        assert_eq!(s.output_route_status().len(), 2);
        assert!(!s.output_route_status()[0].healthy);
        s.doc.undo();
        s.tick();
        assert!(matches!(
            s.output_route_requests(s.output_routes(), true).unwrap()[0].source,
            OutputSource::TrackMeter(_)
        ));
        s.meters.remove(&bus_id);
        assert!(
            s.output_route_requests(s.output_routes(), false)
                .unwrap_err()
                .contains("meter capacity")
        );
        assert_eq!(
            s.output_route_requests(s.output_routes(), true).unwrap()[0].source,
            OutputSource::Disabled
        );
    }

    #[test]
    fn physical_channel_overlap_and_route_serialization() {
        assert!(channels_overlap(
            OutputChannels::Mono(1),
            OutputChannels::Stereo(0, 1)
        ));
        assert!(!channels_overlap(
            OutputChannels::Stereo(0, 1),
            OutputChannels::Stereo(2, 3)
        ));
        let data = serde_json::to_value(route()).unwrap();
        assert_eq!(data["channels"], serde_json::json!({"stereo":[0,1]}));
        assert_eq!(
            serde_json::from_value::<OutputRoute>(data).unwrap(),
            route()
        );
    }
}
