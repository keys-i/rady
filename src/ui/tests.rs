use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::Ordering;

use super::report::{base64, markdown_html};
use super::*;

#[test]
fn base64_padding_is_table_driven() {
    for (input, expected) in [
        (b"".as_slice(), ""),
        (b"M".as_slice(), "TQ=="),
        (b"Ma".as_slice(), "TWE="),
        (b"Man".as_slice(), "TWFu"),
    ] {
        assert_eq!(base64(input), expected);
    }
}

#[test]
fn report_renders_rich_markdown_and_escapes_raw_html() -> Result<()> {
    let html = markdown_html(
        "# Result\n\n**Fast** $x^2$ <mark>clear</mark> ~subscript~[^1]\n\n- Parent\n  - Nested\n\n```rust\nlet bounded_output = input.chars().take(120).collect::<String>();\n```\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n[^1]: note <script>x</script> <section><b>raw",
    )?;
    assert!(html.contains("<h1>Result</h1>"));
    assert!(html.contains("<strong>Fast</strong>"));
    assert!(html.contains("<math"));
    assert!(html.contains("<mark>clear</mark>"));
    assert!(html.contains("<sub>subscript</sub>"));
    assert!(html.contains("Nested"));
    assert!(html.contains("<pre><code class=\"language-rust\">"));
    assert!(html.contains("<table>"));
    assert!(html.contains("footnote-definition"));
    assert!(!html.contains("<script>"));
    assert!(!html.contains("<section>"));
    Ok(())
}

#[test]
fn unsafe_links_and_embedded_media_are_neutralised() -> Result<()> {
    let html = markdown_html(
        "[bad](javascript:alert(1)) [good](https://example.com) ![remote](https://example.com/a.png)",
    )?;
    assert!(!html.contains("javascript:"));
    assert!(html.contains("https://example.com"));
    assert!(!html.contains("<img"));
    assert!(html.contains("[Image: remote]"));
    assert!(markdown_html(&"x".repeat(512 * 1024 + 1)).is_err());
    Ok(())
}

#[test]
fn terminal_markup_supports_emphasis_and_underline() {
    let value = terminal_emphasis("**bold** *italic* <u>line</u>");
    assert!(value.contains("\x1b[1m"));
    assert!(value.contains("\x1b[3m"));
    assert!(value.contains("\x1b[4m"));
    for unclosed in ["**bold", "*italic", "<u>line"] {
        assert_eq!(terminal_emphasis(unclosed), unclosed);
    }
    let clean = terminal_text("safe\x1b]8;;https://bad.example\x07text\u{009b}31m");
    assert!(!clean.chars().any(char::is_control));
    assert!(!clean.contains('\x1b'));
    let mut ui = Ui::indeterminate(Theme::Plain, OutputMode::Json);
    for _ in 0..5 {
        ui.stage("bounded work");
    }
    assert_eq!(ui.current, 5);
}

#[test]
fn typing_boundaries_keep_unicode_scalars_intact() {
    let label = "duck 🦆 ready";
    for (visible, expected) in [(0, ""), (1, "d"), (6, "duck 🦆"), (99, label)] {
        assert_eq!(&label[..scalar_boundary(label, visible)], expected);
    }
}

#[test]
fn decoding_phases_replace_every_unsettled_position() {
    let stage = StageLine {
        accent: "36",
        current: 2,
        total: 7,
        determinate: true,
        label: "duck".into(),
    };
    let mut previous = None;
    for (phase, expected) in [(0, "%+~"), (1, "&=@"), (2, "*?#")] {
        assert!(expected.chars().all(|glyph| glyph.is_ascii_graphic()));
        let mut output = Vec::new();
        Ui::write_stage_line_locked(&mut output, &stage, 1, Some(phase), false, false);
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains(&format!("d\x1b[36m{expected}\x1b[0m")));
        assert!(!output.contains("uck"));
        assert_ne!(previous, Some(expected));
        previous = Some(expected);
    }
}

#[test]
fn progress_worker_joins_and_noninteractive_modes_stay_instant() {
    for (mode, theme, terminal, no_color, expected) in [
        (OutputMode::Human, Theme::Dusk, true, false, true),
        (OutputMode::Json, Theme::Dusk, true, false, false),
        (OutputMode::Human, Theme::Plain, true, false, false),
        (OutputMode::Human, Theme::Dusk, false, false, false),
        (OutputMode::Human, Theme::Dusk, true, true, false),
    ] {
        assert_eq!(styled_terminal(mode, theme, terminal, no_color), expected);
    }
    for (styled, reduced_motion, ci, expected) in [
        (true, false, false, true),
        (true, true, false, false),
        (true, false, true, false),
        (false, false, false, false),
    ] {
        assert_eq!(terminal_motion(styled, reduced_motion, ci), expected);
    }

    let ui = Ui::new(Theme::Plain, OutputMode::Human, 2);
    let (stop, stopped) = mpsc::channel();
    let joined = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker_joined = Arc::clone(&joined);
    let worker = thread::spawn(move || {
        let _ = stopped.recv();
        worker_joined.store(true, Ordering::Release);
    });
    *ui.animation
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(TypingAnimation { stop, worker });
    assert!(
        ui.animation
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    );
    ui.finish_progress();
    assert!(joined.load(Ordering::Acquire));
    assert!(
        ui.animation
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_none()
    );
}

#[test]
fn final_progress_stages_are_immediate_and_closed() {
    let stage = StageLine {
        accent: "36",
        current: 7,
        total: 7,
        determinate: true,
        label: "Ready to review".into(),
    };
    assert_eq!(
        stage_render_state(&stage, true),
        (usize::MAX, false, true, false)
    );
    let active = StageLine {
        current: 3,
        ..stage
    };
    assert_eq!(stage_render_state(&active, true), (0, false, false, true));
    assert_eq!(
        stage_render_state(&active, false),
        (usize::MAX, false, false, false)
    );
}

#[test]
fn json_success_documents_share_one_envelope() -> Result<()> {
    for (kind, result) in [
        (
            "code",
            serde_json::json!({"id": "run_1", "status": "ready"}),
        ),
        (
            "dependasolve",
            serde_json::json!({"repository": "owner/repo", "apply": false}),
        ),
    ] {
        let document: serde_json::Value =
            serde_json::from_str(&json_success_document(kind, &result)?)?;
        assert_eq!(document["schema"], 1);
        assert_eq!(document["status"], "ok");
        assert_eq!(document["kind"], kind);
        assert_eq!(document["result"], result);
    }
    Ok(())
}

#[test]
fn report_write_replaces_destination_without_orphaning_temporary_file() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("run.html");
    fs::write(&path, "old report")?;

    write_report(
        &path,
        "Replacement",
        "**Current** evidence",
        ReportState::Complete,
        Theme::Dusk,
    )?;

    let report = fs::read_to_string(&path)?;
    assert!(report.contains("Replacement"));
    assert!(report.contains("<strong>Current</strong>"));
    assert!(!report.contains("old report"));
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    assert_no_temporary_report_files(directory.path(), "run.html")?;
    Ok(())
}

#[test]
fn report_write_cleans_temporary_file_when_replacement_fails() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("occupied.html");
    fs::create_dir(&path)?;

    assert!(
        write_report(
            &path,
            "Replacement",
            "**Current** evidence",
            ReportState::Complete,
            Theme::Dusk,
        )
        .is_err()
    );
    assert_no_temporary_report_files(directory.path(), "occupied.html")?;
    Ok(())
}

fn assert_no_temporary_report_files(directory: &Path, file_name: &str) -> Result<()> {
    let prefix = format!(".{file_name}.");
    assert!(fs::read_dir(directory)?.all(|entry| {
        entry
            .map(|entry| !entry.file_name().to_string_lossy().starts_with(&prefix))
            .unwrap_or(false)
    }));
    Ok(())
}

#[test]
fn report_theme_matrix_renders_complete_self_contained_documents() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for theme in [
        Theme::Auto,
        Theme::Dawn,
        Theme::Moss,
        Theme::Tide,
        Theme::Dusk,
        Theme::Plain,
    ] {
        for (state, state_label) in [
            (ReportState::Active, "Working"),
            (ReportState::Complete, "Ready to inspect"),
            (ReportState::Stopped, "Stopped safely"),
        ] {
            let path = directory.path().join(format!("{theme:?}-{state:?}.html"));
            write_report(
                &path,
                "Evidence <safe>",
                "**Ready** *calm* <u>clear</u> $x^2$",
                state,
                theme,
            )?;
            let report = fs::read_to_string(path)?;
            assert!(!report.contains("{{"), "{theme:?} {state:?}");
            assert!(report.contains("Evidence &lt;safe&gt;"), "{theme:?}");
            assert!(report.contains("<strong>Ready</strong>"), "{theme:?}");
            assert!(report.contains("<em>calm</em>"), "{theme:?}");
            assert!(report.contains("<u>clear</u>"), "{theme:?}");
            assert!(report.contains("<math"), "{theme:?}");
            assert!(report.contains("class=\"duck\""), "{theme:?}");
            assert!(
                report.contains("alt=\"Pekin duck inspecting a keyboard\""),
                "{theme:?}"
            );
            assert!(report.contains("Duck on watch"), "{theme:?}");
            assert!(report.contains(state_label), "{theme:?} {state:?}");
            assert!(report.contains("data:image/png;base64,"), "{theme:?}");
            assert!(!report.contains("@keyframes duck-idle"), "{theme:?}");
            assert!(report.contains("@keyframes theme-settle"), "{theme:?}");
            assert!(!report.contains("possum"), "{theme:?}");
            assert!(!report.contains("<script"), "{theme:?}");
            assert_eq!(
                report.contains("aria-valuenow=\"100\""),
                state == ReportState::Complete
            );
            if state == ReportState::Active {
                assert!(report.contains("role=\"status\" aria-label=\"Run in progress\""));
            }
        }
    }
    Ok(())
}
