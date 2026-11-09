use super::*;
use peel_session::template;

fn run_sync(language: Language, source: &str, project: &Project) -> Result<ScriptOutput, String> {
    run(
        Script {
            name: "Test".into(),
            language,
            source: source.into(),
        },
        project.clone(),
        ScriptContext::default(),
        Arc::new(AtomicBool::new(false)),
    )
}
fn message(result: Result<ScriptOutput, String>) -> String {
    match result {
        Ok(_) => panic!("expected failure"),
        Err(e) => e,
    }
}

#[test]
fn both_languages_round_trip_without_dirtying_or_history() {
    for language in [Language::JavaScript, Language::Lua] {
        let mut session = Session::new_silent(template::beat_and_melody());
        let output = run_sync(language, "", session.project()).unwrap();
        assert_eq!(output.project, *session.project());
        assert!(!output.apply(&mut session).unwrap());
        assert!(!session.doc.can_undo());
        assert!(!session.doc.is_dirty());
    }
}

#[test]
fn edits_are_one_undo_step_with_redo() {
    for (language, source) in [
        (
            Language::JavaScript,
            "peel.project.meta.title = 'Scripted'; peel.project.swing = 0.25; peel.log('done');",
        ),
        (
            Language::Lua,
            "peel.project.meta.title = 'Scripted'; peel.project.swing = 0.25; peel.log('done')",
        ),
    ] {
        let mut session = Session::new_silent(template::beat_and_melody());
        let before = session.project().meta.title.clone();
        let output = run_sync(language, source, session.project()).unwrap();
        assert_eq!(output.logs, ["done"]);
        assert!(output.apply(&mut session).unwrap());
        assert_eq!(session.project().meta.title, "Scripted");
        assert_eq!(session.doc.undo_label(), Some("Script: Test"));
        session.doc.undo();
        assert_eq!(session.project().meta.title, before);
        assert_eq!(session.project().swing, 0.0);
        assert!(!session.doc.can_undo());
        session.doc.redo();
        assert_eq!(session.project().swing, 0.25);
    }
}

#[test]
fn factory_actions_use_selection_in_both_languages() {
    let mut session = Session::new_silent(template::beat_and_melody());
    let pattern = &session.project().patterns[0];
    let note = pattern.notes[0];
    let other = pattern.notes[1];
    let context = ScriptContext {
        pattern: Some(pattern.id),
        channel: Some(note.channel),
        notes: vec![note.id],
        ..Default::default()
    };
    let scripts = builtin_scripts();
    for script in scripts.into_iter().take(2) {
        let delta = if script.language == Language::JavaScript {
            12
        } else {
            0
        };
        let output = run(
            script,
            session.project().clone(),
            context.clone(),
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(output.apply(&mut session).unwrap());
        let pattern = &session.project().patterns[0];
        assert_eq!(
            pattern.notes.iter().find(|n| n.id == note.id).unwrap().key,
            note.key + delta
        );
        assert_eq!(
            *pattern.notes.iter().find(|n| n.id == other.id).unwrap(),
            other
        );
    }
}

#[test]
fn exceptions_and_invalid_results_leave_live_document_untouched() {
    let session = Session::new_silent(template::beat_and_melody());
    for (language, source) in [
        (
            Language::JavaScript,
            "peel.project.meta.title='lost'; throw new Error('boom');",
        ),
        (
            Language::Lua,
            "peel.project.meta.title='lost'; error('boom')",
        ),
        (Language::JavaScript, "peel.project.channels[0].pan = 2"),
        (Language::Lua, "peel.project.channels[1].pan = 2"),
        (
            Language::JavaScript,
            "peel.project.patterns[0].length='1/0'",
        ),
        (
            Language::Lua,
            "peel.project.patterns[1].length='999999999999999999999999999/1'",
        ),
        (
            Language::JavaScript,
            "peel.project.assets.push({id:peel.newId(),path:'/private/file',managed:false})",
        ),
    ] {
        assert!(
            run_sync(language, source, session.project()).is_err(),
            "{source}"
        );
        assert!(!session.doc.can_undo());
        assert!(!session.doc.is_dirty());
    }
}

#[test]
fn stale_results_and_identity_changes_are_rejected() {
    let mut session = Session::new_silent(template::beat_and_melody());
    let output = run_sync(
        Language::JavaScript,
        "peel.project.meta.title='stale'",
        session.project(),
    )
    .unwrap();
    session
        .doc
        .edit("User edit", |p| p.meta.title = "Newer".into());
    assert!(output.apply(&mut session).unwrap_err().contains("changed"));
    assert_eq!(session.project().meta.title, "Newer");
    let output = run_sync(
        Language::Lua,
        "peel.project.id = peel.newId()",
        session.project(),
    )
    .unwrap();
    assert!(output.apply(&mut session).unwrap_err().contains("identity"));
}

#[test]
fn runtimes_have_no_direct_system_access() {
    let project = template::beat_and_melody();
    run_sync(Language::JavaScript, "if (typeof process !== 'undefined' || typeof require !== 'undefined' || typeof fetch !== 'undefined') throw Error('system access');", &project).unwrap();
    run_sync(Language::Lua, "assert(io == nil and os == nil and debug == nil and package == nil and require == nil and load == nil and loadfile == nil and dofile == nil and coroutine == nil)", &project).unwrap();
}

#[test]
fn infinite_loops_are_interrupted_and_jobs_cancel() {
    let project = template::beat_and_melody();
    for (language, source) in [
        (Language::JavaScript, "while(true) {}"),
        (Language::Lua, "while true do end"),
    ] {
        assert!(message(run_sync(language, source, &project)).contains("time limit"));
        let mut job = ScriptJob::start(
            Script {
                name: "Loop".into(),
                language,
                source: source.into(),
            },
            project.clone(),
            Default::default(),
        );
        job.cancel();
        let start = Instant::now();
        loop {
            if let Some(result) = job.poll() {
                assert!(message(result).contains("cancelled"));
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(3));
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn cancellation_wins_after_worker_completion() {
    let mut session = Session::new_silent(template::beat_and_melody());
    let output = run_sync(
        Language::JavaScript,
        "peel.project.swing=0.5",
        session.project(),
    )
    .unwrap();
    output.cancelled.store(true, Ordering::Relaxed);
    assert!(
        output
            .apply(&mut session)
            .unwrap_err()
            .contains("cancelled")
    );
    assert!(!session.doc.can_undo());
}

#[test]
fn excessive_logs_and_vm_memory_are_bounded() {
    let project = template::beat_and_melody();
    for (language, source, allocate) in [
        (
            Language::JavaScript,
            "for(let i=0;i<10000;i++) peel.log('x'.repeat(1000));",
            "const a=[]; while(true) a.push(new Uint8Array(1024*1024));",
        ),
        (
            Language::Lua,
            "for i=1,10000 do peel.log(string.rep('x',1000)) end",
            "local a={}; while true do a[#a+1]=string.rep('x',1024*1024) end",
        ),
    ] {
        let output = run_sync(language, source, &project).unwrap();
        assert!(output.logs.iter().map(|s| s.len()).sum::<usize>() <= MAX_LOG_BYTES);
        assert!(run_sync(language, allocate, &project).is_err());
    }
}

#[test]
fn python_uses_packages_and_supports_atomic_edits() {
    // Python is optional for app users. CI can explicitly point to its interpreter.
    if std::process::Command::new(
        std::env::var_os("PEEL_PYTHON").unwrap_or_else(|| "python3".into()),
    )
    .arg("--version")
    .output()
    .is_err()
    {
        return;
    }
    let mut session = Session::new_silent(template::beat_and_melody());
    let output = run_sync(Language::Python, "import fractions\npeel.project['meta']['title'] = 'Café 🎹 ' + str(fractions.Fraction(1, 3))\nprint('Python', 'works')", session.project()).unwrap();
    assert_eq!(output.logs, ["Python works"]);
    assert!(output.apply(&mut session).unwrap());
    assert_eq!(session.project().meta.title, "Café 🎹 1/3");
    session.doc.undo();
    assert!(!session.doc.can_undo());
    assert!(
        message(run_sync(
            Language::Python,
            "raise ValueError('python failure')",
            session.project()
        ))
        .contains("python failure")
    );
}

#[test]
#[ignore = "requires PyTheory in PEEL_PYTHON; run explicitly for integration verification"]
fn pytheory_factory_action_adds_a_real_chord() {
    let mut session = Session::new_silent(template::beat_and_melody());
    let pattern = &session.project().patterns[0];
    let count = pattern.notes.len();
    let context = ScriptContext {
        pattern: Some(pattern.id),
        channel: Some(session.project().channels[0].id),
        ..Default::default()
    };
    let script = builtin_scripts()
        .into_iter()
        .find(|s| s.language == Language::Python)
        .unwrap();
    let output = run(
        script,
        session.project().clone(),
        context,
        Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert!(output.apply(&mut session).unwrap());
    let pattern = &session.project().patterns[0];
    assert_eq!(pattern.notes.len(), count + 3);
    for key in [60, 64, 67] {
        assert!(
            pattern
                .notes
                .iter()
                .any(|n| n.key == key && n.start == peel_document::Time::ZERO)
        );
    }
    session.doc.undo();
    assert_eq!(session.project().patterns[0].notes.len(), count);
}

#[test]
fn full_width_seed_and_revision_survive_both_runtimes() {
    let mut project = template::beat_and_melody();
    project.seed = u64::MAX;
    project.revision = u64::MAX - 1;
    project.extensions.insert(
        "example".into(),
        serde_json::json!({"length":"not a musical time"}),
    );
    for language in [Language::JavaScript, Language::Lua] {
        let output = run_sync(language, "", &project).unwrap();
        assert_eq!(output.project, project);
    }
    project
        .extensions
        .insert("large".into(), serde_json::json!(u64::MAX));
    assert!(message(run_sync(Language::JavaScript, "", &project)).contains("safe range"));
}

#[test]
fn lua_expansion_and_cycles_are_rejected_before_host_conversion() {
    let project = template::beat_and_melody();
    assert!(
        run_sync(
            Language::Lua,
            "local t={}; for i=1,30 do t={t,t} end; peel.project.extensions.expansion=t",
            &project
        )
        .is_err()
    );
    assert!(
        run_sync(
            Language::Lua,
            "peel.project.extensions.cycle=peel.project",
            &project
        )
        .is_err()
    );
}

#[test]
fn python_can_cancel_native_sleep() {
    let executable = std::env::var_os("PEEL_PYTHON").unwrap_or_else(|| {
        if cfg!(windows) {
            "python".into()
        } else {
            "python3".into()
        }
    });
    if std::process::Command::new(executable)
        .arg("--version")
        .output()
        .is_err()
    {
        return;
    }
    let mut job = ScriptJob::start(
        Script {
            name: "Sleep".into(),
            language: Language::Python,
            source: "import time; time.sleep(30)".into(),
        },
        template::beat_and_melody(),
        Default::default(),
    );
    std::thread::sleep(Duration::from_millis(80));
    job.cancel();
    let start = Instant::now();
    loop {
        if let Some(result) = job.poll() {
            assert!(message(result).contains("cancelled"));
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn revisionless_plugin_state_changes_are_preserved() {
    let mut session = Session::new_silent(template::beat_and_melody());
    let edit = run_sync(
        Language::JavaScript,
        "peel.project.swing=0.3",
        session.project(),
    )
    .unwrap();
    let report = run_sync(Language::Lua, "peel.log('report')", session.project()).unwrap();
    session
        .doc
        .patch(|p| p.channels[0].device.state = Some("newly captured state".into()));
    assert!(edit.apply(&mut session).unwrap_err().contains("changed"));
    assert!(!report.apply(&mut session).unwrap());
    assert_eq!(
        session.project().channels[0].device.state.as_deref(),
        Some("newly captured state")
    );
    assert!(!session.doc.can_undo());
}

#[test]
fn read_only_scripts_can_report_loaded_projects_with_legacy_values() {
    let mut project = template::beat_and_melody();
    project.channels[0]
        .device
        .params
        .insert("old.parameter".into(), 3.0);
    for (language, source) in [
        (Language::JavaScript, "peel.log('report')"),
        (Language::Lua, "peel.log('report')"),
    ] {
        let report = run_sync(language, source, &project).unwrap();
        assert_eq!(report.project, project);
        assert_eq!(report.logs, ["report"]);
    }
}
