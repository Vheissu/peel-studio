//! Scripts: factory actions and `.js`, `.lua` and `.py` files installed in
//! the user's Scripts folder, shown in the right column in place of the
//! Inspector (F8 or the right rail). A script only runs when Run is
//! pressed; one runs at a time, in the background, and its result stays
//! until the next run. The column is not modal: editing goes on beside it.

use std::path::{Path, PathBuf};

use peel_extensions::{Script, ScriptContext, ScriptJob};
use peel_session::Session;
use peel_ui::{Align, FocusKind, Id as W, Rect, Role, Semantics, Ui, vec2};

use crate::studio::Studio;
use crate::workspace::Workspace;

pub const PANEL: W = W(0x5c21_9700_0f08);

/// File types the folder scan and Add file accept.
const EXTENSIONS: [&str; 3] = ["js", "lua", "py"];
/// Log lines kept on screen; earlier ones are summarized.
const MAX_LOG_LINES: usize = 200;

const ROW_H: f32 = 28.0;
const HEAD_H: f32 = 24.0;

/// Which script is selected: a factory index (names can repeat across
/// languages), or a local file so a rescan keeps the selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Factory(usize),
    Local(PathBuf),
}

pub struct Running {
    pub(crate) job: ScriptJob,
    pub name: String,
    pub cancelling: bool,
}

#[derive(Clone, Debug)]
pub struct Outcome {
    pub name: String,
    /// `Ok` carries the summary ("Applied", "Finished").
    pub result: Result<String, String>,
    pub logs: Vec<String>,
}

pub struct Scripts {
    pub dir: PathBuf,
    pub python: Option<PathBuf>,
    pub factory: Vec<Script>,
    pub local: Vec<(PathBuf, Script)>,
    /// Files in the folder that did not load: (file name, error).
    pub broken: Vec<(String, String)>,
    pub selected: Option<Pick>,
    pub job: Option<Running>,
    pub(crate) control: peel_mcp::Control,
    pub last: Option<Outcome>,
    /// Show the column on the next frame (`PEEL_SCRIPTS`).
    pub open_requested: bool,
    /// Whether the Inspector showed before Scripts replaced it, so closing
    /// Scripts puts it back.
    pub return_inspector: bool,
    scanned: bool,
}

impl Default for Scripts {
    fn default() -> Self {
        Scripts::with_dir(default_dir())
    }
}

/// `<app data>/Peel Studio/Scripts`; tests use a private temp folder.
pub fn default_dir() -> PathBuf {
    if cfg!(test) {
        std::env::temp_dir().join(format!("peel-app-scripts-{}", std::process::id()))
    } else {
        crate::prefs::app_dir().join("Scripts")
    }
}

fn is_script_file(path: &Path) -> bool {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .is_some_and(|e| EXTENSIONS.contains(&e.as_str()))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

impl Scripts {
    pub fn with_dir(dir: PathBuf) -> Scripts {
        Scripts {
            dir,
            python: None,
            factory: Vec::new(),
            local: Vec::new(),
            broken: Vec::new(),
            selected: None,
            job: None,
            control: Default::default(),
            last: None,
            open_requested: false,
            return_inspector: true,
            scanned: false,
        }
    }

    /// Loads the factory list and scans the folder the first time the
    /// panel opens.
    pub fn ensure_scanned(&mut self) {
        if !self.scanned {
            self.refresh();
        }
    }

    /// Reloads factory scripts and rescans the folder.
    pub fn refresh(&mut self) {
        self.scanned = true;
        self.factory = peel_extensions::builtin_scripts();
        self.local.clear();
        self.broken.clear();
        let mut files: Vec<PathBuf> = std::fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.is_file() && is_script_file(p))
                    .collect()
            })
            .unwrap_or_default();
        files.sort_by_key(|p| file_name(p).to_lowercase());
        for path in files {
            match Script::from_path(&path) {
                Ok(s) => self.local.push((path, s)),
                Err(e) => self.broken.push((file_name(&path), e)),
            }
        }
        if !self
            .selected
            .as_ref()
            .is_some_and(|p| self.script(p).is_some())
        {
            self.selected = (!self.factory.is_empty()).then_some(Pick::Factory(0));
        }
    }

    pub fn script(&self, pick: &Pick) -> Option<&Script> {
        match pick {
            Pick::Factory(i) => self.factory.get(*i),
            Pick::Local(path) => self.local.iter().find(|(p, _)| p == path).map(|(_, s)| s),
        }
    }

    pub fn selected_script(&self) -> Option<&Script> {
        self.selected.as_ref().and_then(|p| self.script(p))
    }

    /// Copies `src` into the Scripts folder. Never replaces an existing
    /// file; the copy must load as a script.
    pub fn add_file(&mut self, src: &Path) -> Result<String, String> {
        if !is_script_file(src) {
            return Err("Scripts must be .js, .lua or .py files".into());
        }
        let name = file_name(src);
        if name.is_empty() {
            return Err("Not a file".into());
        }
        let dest = self.dir.join(&name);
        if dest.exists() {
            return Err(format!("{name} is already installed"));
        }
        let script = Script::from_path(src).map_err(|e| format!("{name}: {e}"))?;
        let bytes = script.source.as_bytes();
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("Could not create the Scripts folder: {e}"))?;
        // `create_new` refuses to overwrite even if the file appears now.
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&dest)?;
            f.write_all(bytes)?;
            f.sync_all()
        };
        if let Err(e) = write() {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                return Err(format!("{name} is already installed"));
            }
            let _ = std::fs::remove_file(&dest);
            return Err(format!("Could not copy {name}: {e}"));
        }
        self.refresh();
        self.selected = Some(Pick::Local(dest));
        Ok(name)
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    /// Starts the selected script on a copy of the project. Refuses while
    /// another script runs or audio or notes are being recorded.
    pub fn run_selected(
        &mut self,
        session: &Session,
        ws: &Workspace,
        recording: bool,
    ) -> Result<(), String> {
        if self.job.is_some() {
            return Err("A script is already running".into());
        }
        if recording {
            return Err("Stop recording to run a script".into());
        }
        let script = self
            .selected_script()
            .cloned()
            .ok_or_else(|| "Choose a script".to_string())?;
        let name = script.name.clone();
        let project = session.project().clone();
        let start = if cfg!(test) {
            ScriptJob::start_with_python
        } else {
            ScriptJob::start_isolated
        };
        let job = start(script, project, context(session, ws), self.python.clone());
        self.job = Some(Running {
            job,
            name,
            cancelling: false,
        });
        Ok(())
    }

    pub fn cancel(&mut self) {
        if let Some(run) = &mut self.job {
            run.job.cancel();
            run.cancelling = true;
        }
    }

    /// Checks the running job once; applies a finished result. Returns the
    /// outcome when the job ended this call.
    pub fn poll(&mut self, session: &mut Session, recording: bool) -> Option<Outcome> {
        let run = self.job.as_mut()?;
        let result = run.job.poll()?;
        let run = self.job.take()?;
        let outcome = match result {
            Ok(out) => {
                let name = if out.name.is_empty() {
                    run.name.clone()
                } else {
                    out.name.clone()
                };
                let logs = out.logs.clone();
                // `apply` refuses cancelled runs and results computed
                // against an older revision of the project.
                let result = if recording && out.has_project_changes() {
                    Err("Not applied: recording started".to_string())
                } else {
                    out.apply(session)
                        .map(|changed| if changed { "Applied" } else { "Finished" }.to_string())
                };
                Outcome { name, result, logs }
            }
            Err(e) => Outcome {
                name: run.name,
                result: Err(e),
                logs: Vec::new(),
            },
        };
        match &outcome.result {
            Ok(msg) => session.notify(format!("{}: {msg}", outcome.name), false),
            Err(e) => session.notify(format!("{}: {e}", outcome.name), true),
        }
        self.last = Some(outcome.clone());
        Some(outcome)
    }
}

/// What the script sees as the current selection.
pub fn context(session: &Session, ws: &Workspace) -> ScriptContext {
    let mut notes: Vec<_> = ws.roll.selection.iter().copied().collect();
    notes.sort();
    let mut clips: Vec<_> = ws.playlist.selection.iter().copied().collect();
    clips.sort();
    ScriptContext {
        pattern: Some(session.selected_pattern()),
        channel: ws.selected_channel,
        notes,
        clips,
    }
}

/// Audio or note recording is in progress.
pub fn recording(studio: &Studio) -> bool {
    studio.session.is_recording() || (studio.recorder.armed && studio.session.is_playing())
}

/// Polls the running job once per frame and keeps frames coming while it
/// runs.
pub fn frame(ui: &mut Ui, studio: &mut Studio) {
    crate::script_host::pump(studio);
    let rec = recording(studio);
    if let Some(out) = studio.scripts.poll(&mut studio.session, rec) {
        let msg = match &out.result {
            Ok(m) => format!("{}: {m}", out.name),
            Err(e) => format!("{}: {e}", out.name),
        };
        ui.announce(msg);
    }
    if studio.scripts.is_running() {
        ui.request_repaint();
    }
}

/// Shows Scripts in the right column, or puts the Inspector back (F8,
/// the right rail, the View menu). Saved like the other pane choices.
pub fn toggle(studio: &mut Studio) {
    if studio.prefs.show_scripts {
        close(studio);
    } else {
        open(studio);
    }
    studio.prefs.save();
}

fn open(studio: &mut Studio) {
    studio.scripts.return_inspector = studio.prefs.show_inspector;
    studio.scripts.ensure_scanned();
    studio.prefs.set_scripts(true);
}

fn close(studio: &mut Studio) {
    studio.prefs.show_scripts = false;
    if studio.scripts.return_inspector {
        studio.prefs.set_inspector(true);
    }
}

/// Applies a pending open request (`PEEL_SCRIPTS`) and loads the list
/// whenever the column shows, however it was shown (scripts can show it
/// with `ui.set`).
pub fn sync(studio: &mut Studio) {
    if std::mem::take(&mut studio.scripts.open_requested) && !studio.prefs.show_scripts {
        open(studio);
    }
    if studio.prefs.show_scripts {
        studio.scripts.ensure_scanned();
    }
}

/// A button that shows as disabled when `enabled` is false.
fn action_button(ui: &mut Ui, id: W, r: Rect, label: &str, primary: bool, enabled: bool) -> bool {
    let t = ui.theme.clone();
    let clicked = if enabled {
        if primary {
            ui.button_primary(id, r, label).clicked
        } else {
            ui.button(id, r, label).clicked
        }
    } else {
        let _ = ui.interact_button(id, r, label);
        ui.scene.rect(r, t.radius_m, t.control.alpha(0.5));
        ui.text(r, label, t.label, t.text_faint, Align::Center);
        false
    };
    ui.describe_node(id, Semantics::patch().disabled(!enabled));
    clicked
}

enum Row {
    Heading(&'static str),
    Script(Pick, String, String),
    Broken(String, String),
    Empty,
}

/// Draws the Scripts column into `r` (the right column).
pub fn column(ui: &mut Ui, r: Rect, studio: &mut Studio) {
    let t = ui.theme.clone();
    let rec = recording(studio);
    let mut rows = vec![Row::Heading("Factory")];
    for (i, s) in studio.scripts.factory.iter().enumerate() {
        rows.push(Row::Script(
            Pick::Factory(i),
            s.name.clone(),
            s.language.to_string(),
        ));
    }
    rows.push(Row::Heading("Installed"));
    for (p, s) in &studio.scripts.local {
        rows.push(Row::Script(
            Pick::Local(p.clone()),
            s.name.clone(),
            s.language.to_string(),
        ));
    }
    for (f, e) in &studio.scripts.broken {
        rows.push(Row::Broken(f.clone(), e.clone()));
    }
    if studio.scripts.local.is_empty() && studio.scripts.broken.is_empty() {
        rows.push(Row::Empty);
    }
    let selection = selection_summary(&studio.ws);
    let mut pick: Option<Pick> = None;
    let mut add = false;
    let mut refresh = false;
    let mut run = false;
    let mut cancel = false;
    let mut close = false;
    let mut choose_python = false;
    let mut system_python = false;
    let running = studio
        .scripts
        .job
        .as_ref()
        .map(|j| (j.name.clone(), j.cancelling));
    let last = studio.scripts.last.clone();
    let selected = studio.scripts.selected.clone();
    let can_run = running.is_none() && !rec && studio.scripts.selected_script().is_some();
    let dir = studio.scripts.dir.display().to_string();

    let python_selected = studio
        .scripts
        .selected_script()
        .is_some_and(|s| s.language == peel_extensions::Language::Python);
    let python_path = studio
        .scripts
        .python
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| peel_extensions::default_python_label().into());
    ui.scene.rect(r, 0.0, t.panel);
    ui.describe_node(PANEL, Semantics::new(Role::Group).label("Scripts").rect(r));
    {
        let mut body = r.shrink2(12.0, 0.0);

        // Header: title, then Add file, Refresh and Close as quiet icons.
        let mut head = body.cut_top(t.header_h).shrink2(0.0, 5.0);
        let x = ui.icon_button(
            PANEL.with("close"),
            head.cut_right(28.0),
            peel_ui::icons::X,
            false,
            None,
        );
        ui.name(&x, "Close Scripts");
        ui.tooltip(&x, "Close (F8)");
        if x.clicked {
            close = true;
        }
        head.cut_right(2.0);
        let rb = ui.icon_button(
            W::new("scripts-refresh"),
            head.cut_right(28.0),
            peel_ui::icons::REPEAT,
            false,
            None,
        );
        ui.name(&rb, "Refresh");
        ui.tooltip(&rb, &format!("Rescan {dir}"));
        if rb.clicked {
            refresh = true;
        }
        head.cut_right(2.0);
        let ab = ui.icon_button(
            W::new("scripts-add"),
            head.cut_right(28.0),
            peel_ui::icons::PLUS,
            false,
            None,
        );
        ui.name(&ab, "Add file");
        ui.tooltip(&ab, "Add a .js, .lua or .py file");
        if ab.clicked {
            add = true;
        }
        ui.text(head, "Scripts", t.heading, t.text, Align::Start);
        ui.describe_label(PANEL.with("title"), head, "Scripts");
        ui.scene.hline(r.x, r.right(), r.y + t.header_h, t.line);
        body.cut_top(8.0);

        // Python: the interpreter and its file-access warning. The
        // warning is not a usage hint, so it always shows.
        if python_selected {
            // One quiet interpreter control; its menu picks the bundled
            // Python or another interpreter.
            let mut row = body.cut_top(28.0);
            let label = row.cut_left(72.0);
            ui.text(label, "Interpreter", t.caption, t.text_faint, Align::Start);
            let custom = studio.scripts.python.is_some();
            let shown = if custom {
                python_path.clone()
            } else {
                "Bundled Python".to_string()
            };
            let menu = W::new("scripts-python-menu");
            let pick = ui.dropdown(W::new("scripts-python"), row, &shown, "Python interpreter");
            ui.tooltip(&pick, &python_path);
            if pick.clicked {
                ui.open_popup(menu, row);
            }
            let items = [
                peel_ui::MenuItem::new("Bundled Python")
                    .checked(!custom)
                    .enabled(custom),
                peel_ui::MenuItem::new("Choose interpreter…"),
            ];
            match ui.menu(menu, Ui::below(row), &items) {
                Some(0) => system_python = true,
                Some(1) => choose_python = true,
                _ => {}
            }
            body.cut_top(2.0);
            let warn = body.cut_top(22.0);
            let msg = "Python scripts have full access to your files.";
            ui.text(warn, msg, t.caption, t.warn, Align::Start);
            // A hit region too, so tooling and tests can find it on screen.
            ui.interact(W::new("scripts-python-warning"), warn);
            ui.describe_label(W::new("scripts-python-warning"), warn, msg);
            body.cut_top(6.0);
        }

        // Footer: selection summary, Cancel, Run.
        body.cut_bottom(12.0);
        let mut foot = body.cut_bottom(32.0);
        body.cut_bottom(10.0);
        let run_r = foot.cut_right(88.0);
        let run_label = if running.is_some() {
            "Running…"
        } else {
            "Run"
        };
        if action_button(ui, W::new("scripts-run"), run_r, run_label, true, can_run) {
            run = true;
        }
        if running.is_some() {
            foot.cut_right(8.0);
            let cancelling = running.as_ref().is_some_and(|r| r.1);
            if action_button(
                ui,
                W::new("scripts-cancel"),
                foot.cut_right(80.0),
                "Cancel",
                false,
                !cancelling,
            ) {
                cancel = true;
            }
        }
        foot.cut_right(8.0);
        let (note, color) = if rec {
            ("Stop recording to run".to_string(), t.warn)
        } else {
            (selection, t.text_faint)
        };
        let note = ui.fit_text(&note, t.caption, foot.w);
        ui.text(foot, &note, t.caption, color, Align::Start);

        // Output, when there is any.
        if running.is_some() || last.is_some() {
            let out_h = (body.h * 0.4).clamp(72.0, 140.0);
            let out_r = body.cut_bottom(out_h);
            body.cut_bottom(10.0);
            output(ui, out_r, running.as_ref(), last.as_ref());
        }

        // Script list.
        let list_in = Rect {
            x: body.x - 4.0,
            w: body.w + 8.0,
            ..body
        };
        let content_h: f32 = rows
            .iter()
            .map(|r| {
                if matches!(r, Row::Heading(_)) {
                    HEAD_H
                } else {
                    ROW_H
                }
            })
            .sum();
        let sid = W::new("scripts-list");
        let mut off = vec2(0.0, *ui.state::<f32>(sid));
        ui.scroll_area(sid, list_in, vec2(list_in.w, content_h), &mut off);
        *ui.state::<f32>(sid) = off.y;
        ui.scene.push_clip(list_in);
        let mut y = list_in.y - off.y;
        for row in &rows {
            let rh = if matches!(row, Row::Heading(_)) {
                HEAD_H
            } else {
                ROW_H
            };
            let rr = Rect {
                x: list_in.x,
                y,
                w: list_in.w - 8.0,
                h: rh,
            };
            y += rh;
            if rr.bottom() < list_in.y || rr.y > list_in.bottom() {
                continue;
            }
            match row {
                Row::Heading(label) => {
                    let lr = rr.shrink2(8.0, 0.0);
                    ui.text(lr, label, t.caption, t.text_faint, Align::Start);
                    ui.describe_label(W::new(("scripts-heading", *label)), lr, label);
                }
                Row::Script(p, name, lang) => {
                    let on = selected.as_ref() == Some(p);
                    let id = match p {
                        Pick::Factory(i) => W::new(("script-factory", *i)),
                        Pick::Local(path) => W::new(("script-local", path.as_path())),
                    };
                    let resp = ui.interact_node(
                        id,
                        rr,
                        FocusKind::Activate,
                        Semantics::new(Role::ListItem)
                            .label(name.as_str())
                            .value(lang.as_str())
                            .selected(on),
                    );
                    let hot = ui.animate_bool(id, resp.hovered);
                    if on {
                        ui.scene.rect(rr, t.radius_m, t.raised);
                    } else if hot > 0.0 {
                        ui.scene.rect(rr, t.radius_m, t.raised.alpha(hot * 0.5));
                    }
                    let mut inner = rr.shrink2(8.0, 0.0);
                    let lw = ui.measure(lang, t.caption).x;
                    let lang_r = inner.cut_right(lw);
                    inner.cut_right(10.0);
                    ui.text(lang_r, lang, t.caption, t.text_faint, Align::End);
                    let shown = ui.fit_text(name, t.body, inner.w);
                    let c = if on {
                        t.text
                    } else {
                        t.text_dim.mix(t.text, hot)
                    };
                    ui.text(inner, &shown, t.body, c, Align::Start);
                    if shown != *name {
                        ui.tooltip(&resp, name);
                    }
                    if resp.hovered {
                        ui.set_cursor(peel_ui::CursorIcon::Pointer);
                    }
                    ui.focus_ring(id, rr, t.radius_m);
                    if resp.clicked {
                        pick = Some(p.clone());
                    }
                }
                Row::Broken(file, err) => {
                    let id = W::new(("script-broken", file.as_str()));
                    let resp = ui.interact(id, rr);
                    let mut inner = rr.shrink2(8.0, 0.0);
                    let fw = ui.measure(file, t.body).x.min(inner.w * 0.5);
                    let fr = inner.cut_left(fw);
                    let shown = ui.fit_text(file, t.body, fr.w);
                    ui.text(fr, &shown, t.body, t.text_faint, Align::Start);
                    inner.cut_left(10.0);
                    let e = ui.fit_text(err, t.caption, inner.w);
                    ui.text(inner, &e, t.caption, t.warn, Align::Start);
                    let full = format!("{file}: {err}");
                    ui.tooltip(&resp, &full);
                    ui.describe_label(id, rr, &full);
                }
                Row::Empty => {
                    let lr = rr.shrink2(8.0, 0.0);
                    ui.text(
                        lr,
                        "No scripts installed",
                        t.body,
                        t.text_faint,
                        Align::Start,
                    );
                }
            }
        }
        ui.scene.pop_clip();
    }

    if choose_python && let Some(path) = crate::dialogs::open(&[]) {
        studio.scripts.python = Some(path.clone());
        studio.prefs.python = Some(path);
        studio.prefs.save();
    }
    if system_python {
        studio.scripts.python = None;
        studio.prefs.python = None;
        studio.prefs.save();
    }
    let s = &mut studio.scripts;
    if let Some(p) = pick {
        s.selected = Some(p);
    }
    if refresh {
        s.refresh();
    }
    if add && let Some(path) = crate::dialogs::open(&[("Script", &EXTENSIONS[..])]) {
        match s.add_file(&path) {
            Ok(name) => studio.session.notify(format!("Added {name}"), false),
            Err(e) => studio.session.notify(e, true),
        }
    }
    if run
        && let Err(e) = studio
            .scripts
            .run_selected(&studio.session, &studio.ws, rec)
    {
        studio.session.notify(e, true);
    }
    if cancel {
        studio.scripts.cancel();
    }
    if close {
        toggle(studio);
    }
}

/// "3 notes  ·  1 clip", or nothing selected.
fn selection_summary(ws: &Workspace) -> String {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut parts = Vec::new();
    if !ws.roll.selection.is_empty() {
        parts.push(plural(ws.roll.selection.len(), "note", "notes"));
    }
    if !ws.playlist.selection.is_empty() {
        parts.push(plural(ws.playlist.selection.len(), "clip", "clips"));
    }
    if parts.is_empty() {
        "No selection".into()
    } else {
        format!("{} selected", parts.join(", "))
    }
}

/// The running job, or the last result with its log.
fn output(ui: &mut Ui, r: Rect, running: Option<&(String, bool)>, last: Option<&Outcome>) {
    let t = ui.theme.clone();
    ui.scene.rect(r, t.radius_m, t.panel_alt);
    let inner = r.shrink2(10.0, 8.0);
    let sid = W::new("scripts-output");
    let content_h = *ui.state::<f32>(sid.with("h"));
    let mut off = vec2(0.0, *ui.state::<f32>(sid));
    ui.scroll_area(sid, inner, vec2(inner.w, content_h), &mut off);
    *ui.state::<f32>(sid) = off.y;
    let width = inner.w - 10.0;
    ui.scene.push_clip(inner);
    let top = inner.y - off.y;
    let mut y = top;
    let mut line =
        |ui: &mut Ui, text: &str, style: peel_ui::TextStyle, color: peel_ui::Color, gap: f32| {
            let h = ui.text_wrapped(vec2(inner.x, y), width, text, style, color);
            y += h + gap;
        };
    if let Some((name, cancelling)) = running {
        let text = if *cancelling {
            format!("Cancelling {name}…")
        } else {
            format!("Running {name}…")
        };
        line(ui, &text, t.label, t.text_dim, 6.0);
        ui.describe_label(sid.with("status"), inner, &text);
    } else if let Some(out) = last {
        let (text, color) = match &out.result {
            Ok(m) => (format!("{}: {m}", out.name), t.text),
            Err(e) => (format!("{}: {e}", out.name), t.record),
        };
        line(ui, &text, t.label, color, 6.0);
        ui.describe_label(sid.with("status"), inner, &text);
        let skipped = out.logs.len().saturating_sub(MAX_LOG_LINES);
        if skipped > 0 {
            line(
                ui,
                &format!("{skipped} earlier lines"),
                t.caption,
                t.text_faint,
                2.0,
            );
        }
        for l in out.logs.iter().skip(skipped) {
            line(ui, l, t.caption, t.text_dim, 2.0);
        }
        if !out.logs.is_empty() {
            ui.describe_label(sid.with("log"), inner, &out.logs.join("\n"));
        }
    }
    ui.scene.pop_clip();
    *ui.state::<f32>(sid.with("h")) = y - top;
}
