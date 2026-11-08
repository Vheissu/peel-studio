//! Opening MIDI input ports and keeping them open as devices come and go.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::message::{MidiEvent, Parser};

/// Name of the MIDI client Peel Studio registers with the system.
const CLIENT_NAME: &str = "Peel Studio";
/// Longest wait before trying a port that failed to open again.
const MAX_RETRY_WAIT: Duration = Duration::from_secs(60);

/// Receives every parsed message, on the thread of the port it came from.
/// Several ports means several threads, so the handler must be thread-safe,
/// and it runs inside the system's MIDI callback: do not block or allocate
/// there.
pub type MidiHandler = dyn Fn(&MidiEvent) + Send + Sync + 'static;

/// A MIDI input port the system offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MidiPortInfo {
    /// Opaque identifier, stable while the device stays plugged in. Two
    /// identical keyboards have the same name and different ids.
    pub id: String,
    /// Display name. WinMM (Windows) truncates it to 31 characters and
    /// often shows a generic driver name such as "USB Audio Device".
    pub name: String,
}

/// Which ports [`MidiInput`] keeps open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum PortSelection {
    /// Every input port, including ones plugged in later.
    #[default]
    All,
    /// Only the port with this [`MidiPortInfo::id`]; reconnects if it
    /// disappears and comes back.
    Only(String),
    /// No ports.
    Nothing,
}

/// Lists the MIDI input ports without opening any. Returns an empty list
/// when the system has no MIDI support available (for example no ALSA
/// sequencer in a container).
pub fn list_inputs() -> Vec<MidiPortInfo> {
    match midir::MidiInput::new(CLIENT_NAME) {
        Ok(client) => scan(&client).into_iter().map(|(info, _)| info).collect(),
        Err(e) => {
            log::debug!("MIDI input unavailable: {e}");
            Vec::new()
        }
    }
}

fn scan(client: &midir::MidiInput) -> Vec<(MidiPortInfo, midir::MidiInputPort)> {
    client
        .ports()
        .into_iter()
        .filter_map(|port| {
            // A port that vanishes between listing and naming is skipped.
            let name = client.port_name(&port).ok()?;
            Some((
                MidiPortInfo {
                    id: port.id(),
                    name,
                },
                port,
            ))
        })
        .collect()
}

struct Connection {
    info: MidiPortInfo,
    conn: midir::MidiInputConnection<Parser>,
}

struct Failure {
    attempts: u32,
    retry_at: Instant,
}

/// Opens MIDI input ports and forwards what they receive to a handler.
///
/// Created with [`MidiInput::new`], which opens every port. Call
/// [`poll_ports`](MidiInput::poll_ports) every couple of seconds (it is
/// cheap) to pick up keyboards plugged in later and let go of unplugged
/// ones. Messages are parsed on each port's own callback thread and passed to
/// the handler there, so nothing waits for the UI.
pub struct MidiInput {
    handler: Arc<MidiHandler>,
    /// Used only to list ports; every connection gets a client of its own.
    scanner: Option<midir::MidiInput>,
    selection: PortSelection,
    available: Vec<MidiPortInfo>,
    open: Vec<Connection>,
    failures: HashMap<String, Failure>,
    events: Arc<AtomicU64>,
}

impl MidiInput {
    /// Opens every input port and sends each message to `handler`.
    pub fn new(handler: impl Fn(&MidiEvent) + Send + Sync + 'static) -> MidiInput {
        MidiInput::with_selection(handler, PortSelection::All)
    }

    pub fn with_selection(
        handler: impl Fn(&MidiEvent) + Send + Sync + 'static,
        selection: PortSelection,
    ) -> MidiInput {
        let mut input = MidiInput {
            handler: Arc::new(handler),
            scanner: None,
            selection,
            available: Vec::new(),
            open: Vec::new(),
            failures: HashMap::new(),
            events: Arc::new(AtomicU64::new(0)),
        };
        input.poll_ports();
        input
    }

    /// Re-scans the ports, closing connections whose device is gone (or no
    /// longer selected) and opening new ones. Returns true if the set of open
    /// ports changed. Cheap enough to call every couple of seconds; it does
    /// not block on a device that is slow to open.
    pub fn poll_ports(&mut self) -> bool {
        if self.scanner.is_none() {
            self.scanner = match midir::MidiInput::new(CLIENT_NAME) {
                Ok(client) => Some(client),
                Err(e) => {
                    log::debug!("MIDI input unavailable: {e}");
                    None
                }
            };
        }
        let found = self.scanner.as_ref().map(scan).unwrap_or_default();
        self.available = found.iter().map(|(info, _)| info.clone()).collect();
        let wanted: Vec<&(MidiPortInfo, midir::MidiInputPort)> = found
            .iter()
            .filter(|(info, _)| match &self.selection {
                PortSelection::All => true,
                PortSelection::Only(id) => *id == info.id,
                PortSelection::Nothing => false,
            })
            .collect();

        let mut changed = false;
        for c in std::mem::take(&mut self.open) {
            if wanted.iter().any(|(info, _)| info.id == c.info.id) {
                self.open.push(c);
            } else {
                log::info!("MIDI input closed: {}", c.info.name);
                c.conn.close();
                changed = true;
            }
        }
        // A port that vanished gets a fresh start when it returns.
        self.failures
            .retain(|id, _| self.available.iter().any(|a| a.id == *id));

        for (info, port) in wanted {
            if self.open.iter().any(|c| c.info.id == info.id) {
                continue;
            }
            if self
                .failures
                .get(&info.id)
                .is_some_and(|f| Instant::now() < f.retry_at)
            {
                continue;
            }
            match self.connect(info, port) {
                Ok(conn) => {
                    log::info!("MIDI input opened: {}", info.name);
                    self.failures.remove(&info.id);
                    self.open.push(Connection {
                        info: info.clone(),
                        conn,
                    });
                    changed = true;
                }
                Err(e) => {
                    let attempts = self.failures.get(&info.id).map_or(0, |f| f.attempts) + 1;
                    let wait = Duration::from_secs(2u64 << attempts.min(5)).min(MAX_RETRY_WAIT);
                    log::warn!("could not open MIDI input {}: {e}", info.name);
                    self.failures.insert(
                        info.id.clone(),
                        Failure {
                            attempts,
                            retry_at: Instant::now() + wait,
                        },
                    );
                }
            }
        }
        changed
    }

    fn connect(
        &self,
        info: &MidiPortInfo,
        port: &midir::MidiInputPort,
    ) -> Result<midir::MidiInputConnection<Parser>, String> {
        // `connect` consumes its client, so each port gets its own.
        let mut client = midir::MidiInput::new(CLIENT_NAME).map_err(|e| e.to_string())?;
        // System exclusive, MIDI clock and active sensing are not used, and
        // the clock alone is 24 callbacks per beat.
        client.ignore(midir::Ignore::All);
        let handler = self.handler.clone();
        let events = self.events.clone();
        client
            .connect(
                port,
                &format!("{CLIENT_NAME} input {}", info.name),
                move |_stamp, bytes, parser| {
                    parser.parse(bytes, |event| {
                        events.fetch_add(1, Ordering::Relaxed);
                        handler(&event);
                    });
                },
                Parser::new(),
            )
            .map_err(|e| e.to_string())
    }

    /// Chooses which ports to keep open and applies it now.
    pub fn select(&mut self, selection: PortSelection) -> bool {
        self.selection = selection;
        self.poll_ports()
    }

    pub fn selection(&self) -> &PortSelection {
        &self.selection
    }

    /// Every input port seen by the last scan.
    pub fn ports(&self) -> &[MidiPortInfo] {
        &self.available
    }

    /// The ports that are open right now.
    pub fn connected(&self) -> Vec<MidiPortInfo> {
        self.open.iter().map(|c| c.info.clone()).collect()
    }

    /// Messages received on all ports so far (for an activity light).
    pub fn event_count(&self) -> u64 {
        self.events.load(Ordering::Relaxed)
    }

    /// Closes every port. They reopen on the next
    /// [`poll_ports`](MidiInput::poll_ports) unless the selection is
    /// [`PortSelection::Nothing`].
    pub fn close_all(&mut self) {
        for c in std::mem::take(&mut self.open) {
            c.conn.close();
        }
        self.failures.clear();
    }
}
