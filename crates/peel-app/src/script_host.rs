//! Live scripting calls execute on the session/UI thread, using the same
//! controller as MCP plus desktop selection and view state.
use crate::{
    prefs::Prefs,
    studio::Studio,
    workspace::{BrowserTab, DockTab, Focus, MixerSize, Workspace},
};
use peel_document::Id;
use peel_session::Session;
use serde::Deserialize;
use serde_json::{Value, json};

pub fn pump(studio: &mut Studio) {
    // Temporarily separate the job/controller so each call can update desktop
    // state before the script receives its reply, including after Open/New.
    let mut running = studio.scripts.job.take();
    let mut control = std::mem::take(&mut studio.scripts.control);
    control.tick();
    if let Some(run) = &mut running {
        run.job.pump(|name, args| {
            let before = studio.session.project().id;
            let recording = studio.session.is_recording();
            let target = studio.session.live_target();
            let result = execute(
                &mut studio.session,
                &mut studio.ws,
                &mut studio.prefs,
                &mut control,
                name,
                args,
            );
            if recording != studio.session.is_recording() {
                studio.recorder.armed = studio.session.is_recording();
            }
            if target != studio.session.live_target() {
                studio.ws.selected_channel = studio.session.live_target();
            }
            if studio.session.project().id != before {
                studio.reset_workspace();
                studio.session.set_live_target(studio.ws.selected_channel);
            }
            result
        });
    }
    studio.scripts.job = running;
    studio.scripts.control = control;
}

pub fn execute(
    session: &mut Session,
    ws: &mut Workspace,
    prefs: &mut Prefs,
    control: &mut peel_mcp::Control,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    match name {
        "ui.get" => Ok(view(session, ws, prefs)),
        "ui.set" => update(session, ws, prefs, args),
        "tools" => {
            let mut tools = serde_json::to_value(peel_mcp::tools()).map_err(|e| e.to_string())?;
            let tools = tools.as_array_mut().expect("tool catalog is an array");
            tools.push(json!({"name":"ui.get","description":"Read workspace selection, active editor and visible panels","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}));
            tools.push(json!({"name":"ui.set","description":"Set workspace selection, active editor and visible panels. Inspector and Scripts share a column.","inputSchema":{"type":"object","additionalProperties":false,"properties":{
                "dock":{"enum":["rack","piano_roll","mixer","perform"]},
                "browser_tab":{"enum":["instruments","effects","samples","patterns"]},
                "pattern":{"type":"string"},"channel":{"type":"string"},
                "notes":{"type":"array","items":{"type":"string"}},"clips":{"type":"array","items":{"type":"string"}},
                "focus":{"type":"object","properties":{"kind":{"enum":["channel","track","clip","none"]},"id":{"type":"string"}}},
                "panels":{"type":"object","additionalProperties":false,"properties":{"browser":{"type":"boolean"},"inspector":{"type":"boolean"},"scripts":{"type":"boolean"},"editor":{"type":"boolean"},"help":{"type":"boolean"},"meter":{"type":"boolean"},"status_details":{"type":"boolean"}}},
                "mixer_size":{"enum":["compact","expanded","full"]},
                "mixer_expanded":{"type":"boolean"},
                "effect":{"oneOf":[{"type":"object","required":["kind","device"],"additionalProperties":false,"properties":{"kind":{"const":"open"},"device":{"type":"string"}}},{"type":"object","required":["kind"],"additionalProperties":false,"properties":{"kind":{"const":"close"}}}]},
                "sizes":{"type":"object","additionalProperties":false,"properties":{"browser":{"type":"number","minimum":180,"maximum":420},"inspector":{"type":"number","minimum":240,"maximum":480},"editor":{"type":"number","minimum":180,"maximum":1200}}}
            }}}));
            Ok(Value::Array(tools.clone()))
        }
        _ => {
            let args = args
                .as_object()
                .cloned()
                .ok_or("Host call arguments must be an object")?;
            let name = if name.starts_with("peel_") {
                name.to_string()
            } else {
                format!("peel_{name}")
            };
            let action = peel_mcp::decode(&name, args)?;
            let result = control.execute(session, action);
            session.tick();
            result
        }
    }
}

fn view(session: &Session, ws: &Workspace, prefs: &Prefs) -> Value {
    let focus = ws.focus.map(|f| match f {
        Focus::Channel(id) => json!({"kind":"channel","id":id}),
        Focus::Track(id) => json!({"kind":"track","id":id}),
        Focus::Clip(id) => json!({"kind":"clip","id":id}),
    });
    json!({"dock":match ws.dock {DockTab::Rack=>"rack",DockTab::PianoRoll=>"piano_roll",DockTab::Mixer=>"mixer",DockTab::Perform=>"perform"},
        "browser_tab":match ws.browser_tab {BrowserTab::Instruments=>"instruments",BrowserTab::Effects=>"effects",BrowserTab::Samples=>"samples",BrowserTab::Patterns=>"patterns"},
        "pattern":session.selected_pattern(),"channel":ws.selected_channel,
        "notes":ws.roll.selection,"clips":ws.playlist.selection,"focus":focus,
        "panels":{"browser":prefs.show_browser,"inspector":prefs.show_inspector,"scripts":prefs.show_scripts,"editor":prefs.show_editor,"help":prefs.show_help,"meter":prefs.show_meter,"status_details":prefs.show_status_details},
        "sizes":{"browser":ws.browser_w,"inspector":ws.inspector_w,"editor":ws.dock_h},
        "mixer_size":match ws.mixer_size {MixerSize::Compact=>"compact",MixerSize::Expanded=>"expanded",MixerSize::Full=>"full"},
        "mixer_expanded":ws.mixer_restore_h.is_some(),
        "effect":ws.fx.target.map(|t|json!({"track":t.track,"device":t.device}))})
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Panels {
    browser: Option<bool>,
    inspector: Option<bool>,
    scripts: Option<bool>,
    editor: Option<bool>,
    help: Option<bool>,
    meter: Option<bool>,
    status_details: Option<bool>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Sizes {
    browser: Option<f32>,
    inspector: Option<f32>,
    editor: Option<f32>,
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ViewFocus {
    Channel { id: Id },
    Track { id: Id },
    Clip { id: Id },
    None,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum EffectView {
    Open { device: Id },
    Close,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Update {
    dock: Option<String>,
    browser_tab: Option<String>,
    pattern: Option<Id>,
    channel: Option<Id>,
    notes: Option<Vec<Id>>,
    clips: Option<Vec<Id>>,
    panels: Panels,
    sizes: Sizes,
    focus: Option<ViewFocus>,
    mixer_size: Option<String>,
    mixer_expanded: Option<bool>,
    effect: Option<EffectView>,
}
fn update(
    session: &mut Session,
    ws: &mut Workspace,
    prefs: &mut Prefs,
    args: Value,
) -> Result<Value, String> {
    let change: Update = serde_json::from_value(args).map_err(|e| e.to_string())?;
    if change.panels.inspector == Some(true) && change.panels.scripts == Some(true) {
        return Err("Inspector and Scripts share the right column; choose one".into());
    }
    for (value, min, max, name) in [
        (change.sizes.browser, 180.0, 420.0, "browser"),
        (change.sizes.inspector, 240.0, 480.0, "inspector"),
        (change.sizes.editor, 180.0, 1200.0, "editor"),
    ] {
        if value.is_some_and(|v| !v.is_finite() || !(min..=max).contains(&v)) {
            return Err(format!("{name} size must be between {min} and {max}"));
        }
    }
    let dock = change
        .dock
        .as_deref()
        .map(|d| match d {
            "rack" => Ok(DockTab::Rack),
            "piano_roll" => Ok(DockTab::PianoRoll),
            "mixer" => Ok(DockTab::Mixer),
            "perform" => Ok(DockTab::Perform),
            _ => Err("Unknown editor"),
        })
        .transpose()?;
    let browser_tab = change
        .browser_tab
        .as_deref()
        .map(|b| match b {
            "instruments" => Ok(BrowserTab::Instruments),
            "effects" => Ok(BrowserTab::Effects),
            "samples" => Ok(BrowserTab::Samples),
            "patterns" => Ok(BrowserTab::Patterns),
            _ => Err("Unknown browser tab"),
        })
        .transpose()?;
    let pattern_id = change.pattern.unwrap_or(session.selected_pattern());
    let pattern = session.project().pattern(pattern_id);
    if change.pattern.is_some() && pattern.is_none() {
        return Err("Unknown pattern".into());
    }
    if change
        .channel
        .is_some_and(|id| session.project().channel(id).is_none())
    {
        return Err("Unknown channel".into());
    }
    if change.notes.as_ref().is_some_and(|ids| {
        ids.iter()
            .any(|id| !pattern.is_some_and(|p| p.notes.iter().any(|n| n.id == *id)))
    }) {
        return Err("Selected notes must belong to the current pattern".into());
    }
    if change.clips.as_ref().is_some_and(|ids| {
        ids.iter().any(|id| {
            !session
                .project()
                .arrangement()
                .clips
                .iter()
                .any(|c| c.id == *id)
        })
    }) {
        return Err("Selected clips must belong to the active arrangement".into());
    }
    let focus = change
        .focus
        .map(|f| match f {
            ViewFocus::Channel { id } if session.project().channel(id).is_some() => {
                Ok(Some(Focus::Channel(id)))
            }
            ViewFocus::Track { id } if session.project().mixer.track(id).is_some() => {
                Ok(Some(Focus::Track(id)))
            }
            ViewFocus::Clip { id }
                if session
                    .project()
                    .arrangement()
                    .clips
                    .iter()
                    .any(|c| c.id == id) =>
            {
                Ok(Some(Focus::Clip(id)))
            }
            ViewFocus::None => Ok(None),
            _ => Err("Unknown focus target"),
        })
        .transpose()?;
    let mixer_size = change
        .mixer_size
        .as_deref()
        .map(|size| match size {
            "compact" => Ok(MixerSize::Compact),
            "expanded" => Ok(MixerSize::Expanded),
            "full" => Ok(MixerSize::Full),
            _ => Err("Unknown mixer size"),
        })
        .transpose()?;
    let effect = change
        .effect
        .map(|effect| match effect {
            EffectView::Close => Ok(None),
            EffectView::Open { device } => session
                .project()
                .mixer
                .tracks
                .iter()
                .find(|t| t.inserts.iter().any(|d| d.id == device))
                .map(|t| Some((t.id, device)))
                .ok_or("Unknown insert effect"),
        })
        .transpose()?;
    // Apply only after the complete request has been validated.
    if let Some(dock) = dock {
        ws.dock = dock;
        prefs.show_editor = true;
    }
    if let Some(tab) = browser_tab {
        ws.browser_tab = tab;
        prefs.show_browser = true;
    }
    if let Some(pattern) = change.pattern {
        session.select_pattern(pattern);
        ws.roll.selection.clear();
    }
    if let Some(channel) = change.channel {
        ws.selected_channel = Some(channel);
        ws.focus = Some(Focus::Channel(channel));
        session.set_live_target(Some(channel));
    }
    if let Some(notes) = change.notes {
        ws.roll.selection = notes.into_iter().collect();
    }
    if let Some(clips) = change.clips {
        ws.playlist.selection = clips.into_iter().collect();
    }
    if let Some(on) = change.panels.browser {
        prefs.show_browser = on;
    }
    if let Some(on) = change.panels.inspector {
        prefs.show_inspector = on;
        if on {
            prefs.show_scripts = false;
        }
    }
    if let Some(on) = change.panels.scripts {
        prefs.show_scripts = on;
        if on {
            prefs.show_inspector = false;
        }
    }
    if let Some(on) = change.panels.editor {
        prefs.show_editor = on;
    }
    if let Some(on) = change.panels.help {
        prefs.show_help = on;
    }
    if let Some(focus) = focus {
        ws.focus = focus;
    }
    if let Some(on) = change.panels.meter {
        prefs.show_meter = on;
    }
    if let Some(on) = change.panels.status_details {
        prefs.show_status_details = on;
    }
    if let Some(w) = change.sizes.browser {
        ws.browser_w = w;
    }
    if let Some(w) = change.sizes.inspector {
        ws.inspector_w = w;
    }
    if let Some(h) = change.sizes.editor {
        ws.dock_h = h;
    }
    if let Some(size) = mixer_size {
        ws.mixer_size = size;
    }
    if let Some(expanded) = change.mixer_expanded {
        if expanded && ws.mixer_restore_h.is_none() {
            ws.mixer_restore_h = Some(ws.dock_h);
            ws.dock_h = 4000.0;
            ws.dock = DockTab::Mixer;
            prefs.show_editor = true;
        } else if !expanded && let Some(height) = ws.mixer_restore_h.take() {
            ws.dock_h = height;
        }
    }
    if let Some(effect) = effect {
        if let Some((track, device)) = effect {
            ws.fx.open(track, device);
            ws.focus = Some(Focus::Track(track));
        } else {
            ws.fx.close();
            session.set_analysis_device(None);
        }
    }
    prefs.capture_layout(ws);
    prefs.save();
    Ok(view(session, ws, prefs))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn opening_a_project_from_a_script_updates_selection_before_the_next_call() {
        let session = Session::new_silent(peel_session::template::beat_and_melody());
        let mut studio = Studio::new(session);
        studio.scripts.factory=vec![peel_extensions::Script {
            name:"New project".into(),language:peel_extensions::Language::JavaScript,
            source:"peel.call('new_project',{title:'Fresh',demo:true,discard_changes:true}); const ui=peel.call('ui.get'); if(!ui.channel) throw Error('no selected channel');".into()
        }];
        studio.scripts.selected = Some(crate::scripts::Pick::Factory(0));
        studio
            .scripts
            .run_selected(&studio.session, &studio.ws, false)
            .unwrap();
        let start = std::time::Instant::now();
        while studio.scripts.is_running() {
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            pump(&mut studio);
            studio.scripts.poll(&mut studio.session, false);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(studio.scripts.last.as_ref().unwrap().result.is_ok());
        assert!(studio.ws.selected_channel.is_some());
        assert_eq!(studio.session.live_target(), studio.ws.selected_channel);
    }

    #[test]
    fn view_calls_control_selection_and_reject_bad_requests_atomically() {
        let mut session = Session::new_silent(peel_session::template::beat_and_melody());
        let mut ws = Workspace::default();
        let mut prefs = Prefs::default();
        let mut control = peel_mcp::Control::default();
        let pattern = session.project().patterns[0].id;
        let channel = session.project().channels[0].id;
        let note = session.project().patterns[0].notes[0].id;
        let state=execute(&mut session,&mut ws,&mut prefs,&mut control,"ui.set",json!({"dock":"mixer","pattern":pattern,"channel":channel,"notes":[note],"panels":{"browser":false,"help":true,"meter":false,"status_details":false},"sizes":{"browser":280,"inspector":320,"editor":360}})).unwrap();
        assert_eq!(state["dock"], "mixer");
        assert_eq!(ws.selected_channel, Some(channel));
        assert!(!prefs.show_browser);
        assert!(prefs.show_help);
        assert!(!prefs.show_meter);
        assert!(!prefs.show_status_details);
        assert_eq!(prefs.browser_w, Some(280.0));
        assert_eq!(prefs.inspector_w, Some(320.0));
        assert_eq!(prefs.dock_h, Some(360.0));
        assert_eq!(prefs.dock_tab, Some(DockTab::Mixer));
        assert!(ws.roll.selection.contains(&note));
        assert!(!session.doc.can_undo());
        assert!(
            execute(
                &mut session,
                &mut ws,
                &mut prefs,
                &mut control,
                "ui.set",
                json!({"dock":"rack","channel":Id::new()})
            )
            .is_err()
        );
        assert_eq!(ws.dock, DockTab::Mixer);
        assert!(
            execute(
                &mut session,
                &mut ws,
                &mut prefs,
                &mut control,
                "ui.set",
                json!({"dock":"rack","sizes":{"browser":1}})
            )
            .is_err()
        );
        assert_eq!(ws.dock, DockTab::Mixer);
        assert_eq!(ws.browser_w, 280.0);
        let state = execute(
            &mut session,
            &mut ws,
            &mut prefs,
            &mut control,
            "status",
            json!({}),
        )
        .unwrap();
        assert!(state.is_object());
        assert!(
            execute(
                &mut session,
                &mut ws,
                &mut prefs,
                &mut control,
                "tools",
                json!({})
            )
            .unwrap()
            .as_array()
            .unwrap()
            .len()
                > 20
        );
    }
    #[test]
    fn scripts_open_effects_and_choose_mixer_layout_atomically() {
        let mut session = Session::new_silent(peel_session::template::beat_and_melody());
        let mut ws = Workspace::default();
        let mut prefs = Prefs::default();
        let mut control = peel_mcp::Control::default();
        let track = session.project().mixer.tracks[0].id;
        let effect = peel_document::DeviceInstance::new("peel.fx.eq", 1);
        let device = effect.id;
        session.doc.edit("Add EQ", |p| {
            p.mixer.track_mut(track).unwrap().inserts.push(effect)
        });
        let history = session.doc.history_labels().count();
        let old_height = ws.dock_h;
        let state = execute(&mut session,&mut ws,&mut prefs,&mut control,"ui.set",json!({"mixer_size":"full","mixer_expanded":true,"effect":{"kind":"open","device":device}})).unwrap();
        assert_eq!(state["effect"]["device"], json!(device));
        assert_eq!(state["mixer_size"], "full");
        assert_eq!(state["mixer_expanded"], true);
        assert_eq!(ws.dock, DockTab::Mixer);
        assert_eq!(ws.focus, Some(Focus::Track(track)));
        assert!(
            execute(
                &mut session,
                &mut ws,
                &mut prefs,
                &mut control,
                "ui.set",
                json!({"mixer_size":"compact","effect":{"kind":"open","device":Id::new()}})
            )
            .is_err()
        );
        assert_eq!(ws.mixer_size, MixerSize::Full);
        assert_eq!(ws.fx.target.unwrap().device, device);
        execute(
            &mut session,
            &mut ws,
            &mut prefs,
            &mut control,
            "ui.set",
            json!({"mixer_expanded":false,"effect":{"kind":"close"}}),
        )
        .unwrap();
        assert_eq!(ws.dock_h, old_height);
        assert!(ws.fx.target.is_none());
        assert_eq!(session.doc.history_labels().count(), history);
    }
}
