//! Deterministic, non-realtime rendering through the same [`Engine`].

use std::sync::Arc;

use rtrb::{Consumer, RingBuffer};

use crate::EngineConfig;
use crate::command::{EngineCommand, Retired};
use crate::engine::Engine;
use crate::live::{LiveInput, LiveTiming, RecordedReceiver};
use crate::telemetry::Telemetry;

/// Renders audio faster (or slower) than realtime for export and tests.
///
/// There is no command queue: [`send`](OfflineEngine::send) applies a command
/// immediately, between `render` calls. Rendering uses exactly the code the
/// audio callback uses. The metronome and preview voice are silent unless
/// [`set_monitoring`](OfflineEngine::set_monitoring) turns them on, so an
/// export never contains them.
pub struct OfflineEngine {
    engine: Engine,
    retired: Consumer<Retired>,
    telemetry: Arc<Telemetry>,
}

impl OfflineEngine {
    pub fn new(sample_rate: f32, config: &EngineConfig) -> OfflineEngine {
        let telemetry = Arc::new(Telemetry::new());
        let (tx, retired) = RingBuffer::new(config.retire_capacity.max(16));
        let mut engine = Engine::new(sample_rate, config, telemetry.clone(), tx, None);
        engine.set_monitoring(false);
        engine.set_cpu_metering(false);
        OfflineEngine {
            engine,
            retired,
            telemetry,
        }
    }

    /// Applies a command now.
    pub fn send(&mut self, cmd: EngineCommand) {
        self.engine.handle_command(cmd);
    }

    /// Renders `left.len().min(right.len())` frames.
    pub fn render(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.engine.render(left, right);
    }

    /// Takes everything the engine has retired since the last call. Dropping
    /// the result frees it; call this regularly during long renders.
    pub fn drain_retired(&mut self) -> Vec<Retired> {
        let mut out = Vec::new();
        while let Ok(item) = self.retired.pop() {
            out.push(item);
        }
        out
    }

    /// Takes the producer end of the live input queue (once). Events are
    /// applied at the start of the next `render`, at sample offset zero (an
    /// offline engine has no device callbacks to time them against).
    pub fn take_live_input(&mut self) -> Option<LiveInput> {
        self.engine.take_live_input()
    }

    /// Takes the ring of recorded live notes (once).
    pub fn take_recorded(&mut self) -> Option<RecordedReceiver> {
        self.engine.take_recorded()
    }

    pub fn set_live_timing(&mut self, timing: LiveTiming) {
        self.engine.set_live_timing(timing);
    }

    /// Includes the metronome and preview voice in the output (off by
    /// default).
    pub fn set_monitoring(&mut self, on: bool) {
        self.engine.set_monitoring(on);
    }

    pub fn telemetry(&self) -> &Arc<Telemetry> {
        &self.telemetry
    }

    pub fn sample_rate(&self) -> f32 {
        self.engine.sample_rate()
    }

    pub fn max_block(&self) -> usize {
        self.engine.max_block()
    }

    pub fn is_playing(&self) -> bool {
        self.engine.is_playing()
    }

    /// Content position in quarter notes.
    pub fn position_beats(&self) -> f64 {
        self.engine.position_beats()
    }

    /// The wrapped engine, for inspection.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }
}
