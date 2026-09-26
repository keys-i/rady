use std::cell::Cell;
use std::io::{self, IsTerminal, Write};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

use crate::Result;

mod report;

pub use report::write_report;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    Auto,
    Dawn,
    Moss,
    Tide,
    Dusk,
    Plain,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    #[default]
    Human,
    Json,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportState {
    Active,
    Complete,
    Stopped,
}

#[derive(Debug)]
pub struct Ui {
    theme: Theme,
    mode: OutputMode,
    styled: bool,
    current: usize,
    total: usize,
    determinate: bool,
    motion: bool,
    line_open: Cell<bool>,
    terminal: Arc<Mutex<()>>,
    animation: Mutex<Option<TypingAnimation>>,
    latest_stage: Mutex<Option<StageLine>>,
}

#[derive(Debug)]
struct TypingAnimation {
    stop: mpsc::Sender<()>,
    worker: thread::JoinHandle<()>,
}

#[derive(Clone, Debug)]
struct StageLine {
    accent: &'static str,
    current: usize,
    total: usize,
    determinate: bool,
    label: String,
}

impl Ui {
    #[must_use]
    pub fn new(theme: Theme, mode: OutputMode, total: usize) -> Self {
        let styled = styled_terminal(
            mode,
            theme,
            io::stderr().is_terminal(),
            std::env::var_os("NO_COLOR").is_some(),
        );
        let motion = terminal_motion(
            styled,
            std::env::var_os("PEKIN_REDUCED_MOTION").is_some(),
            std::env::var_os("CI").is_some(),
        );
        Self {
            theme,
            mode,
            styled,
            current: 0,
            total: total.max(1),
            determinate: true,
            motion,
            line_open: Cell::new(false),
            terminal: Arc::new(Mutex::new(())),
            animation: Mutex::new(None),
            latest_stage: Mutex::new(None),
        }
    }

    #[must_use]
    pub fn indeterminate(theme: Theme, mode: OutputMode) -> Self {
        let mut ui = Self::new(theme, mode, 1);
        ui.determinate = false;
        ui
    }

    pub fn title(&self, title: &str, subtitle: &str) {
        if self.mode == OutputMode::Json {
            return;
        }
        self.finish_progress();
        let title = terminal_text(title);
        let subtitle = terminal_text(subtitle);
        let _terminal = self
            .terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.styled {
            eprintln!(
                "\x1b[1;{}m🦆 PEKIN\x1b[0m  \x1b[2mduck on watch\x1b[0m  {title}",
                self.accent()
            );
            eprintln!("\x1b[2m{subtitle}\x1b[0m\n");
        } else {
            eprintln!("Pekin / duck on watch\n{title}\n{subtitle}\n");
        }
    }

    pub fn stage(&mut self, label: &str) {
        self.current = if self.determinate {
            (self.current + 1).min(self.total)
        } else {
            self.current.saturating_add(1)
        };
        if self.mode == OutputMode::Json {
            return;
        }
        let label = terminal_text(label);
        if self.styled {
            self.stop_animation();
            let stage = StageLine {
                accent: self.accent(),
                current: self.current,
                total: self.total,
                determinate: self.determinate,
                label,
            };
            *self
                .latest_stage
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(stage.clone());
            let (visible, cursor, newline, animate) = stage_render_state(&stage, self.motion);
            self.line_open.set(!newline);
            self.write_stage_line(&stage, visible, animate.then_some(0), cursor, newline);
            if animate && !self.start_animation(stage.clone()) {
                self.line_open.set(false);
                self.write_stage_line(&stage, usize::MAX, None, false, true);
            }
        } else if !self.determinate {
            eprintln!("→ {label}");
        } else {
            eprintln!("[step {}/{}] {label}", self.current, self.total);
        }
    }

    pub fn success(&self, message: &str) {
        self.message("✓", &format!("1;{}", self.accent()), message);
    }

    pub fn note(&self, message: &str) {
        self.message("→", "3;37", message);
    }

    pub fn warning(&self, message: &str) {
        self.message("!", "1;38;2;190;104;45", message);
    }

    fn message(&self, symbol: &str, style: &str, message: &str) {
        if self.mode == OutputMode::Json {
            return;
        }
        self.finish_progress();
        let message = terminal_text(message);
        let _terminal = self
            .terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if self.styled {
            eprintln!("\x1b[{style}m{symbol} {message}\x1b[0m");
        } else {
            eprintln!("{symbol} {message}");
        }
    }

    #[must_use]
    pub fn mode(&self) -> OutputMode {
        self.mode
    }

    #[must_use]
    pub fn theme(&self) -> Theme {
        self.theme
    }

    fn accent(&self) -> &'static str {
        terminal_accent(self.theme)
    }

    pub(crate) fn finish_progress(&self) {
        self.stop_animation();
        if self.styled && self.line_open.replace(false) {
            let stage = self
                .latest_stage
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            let _terminal = self
                .terminal
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(stage) = stage {
                Self::write_stage_line_locked(
                    &mut io::stderr().lock(),
                    &stage,
                    usize::MAX,
                    None,
                    false,
                    true,
                );
            } else {
                let _ = writeln!(io::stderr().lock());
            }
        }
    }

    fn stop_animation(&self) {
        let animation = self
            .animation
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(animation) = animation {
            let _ = animation.stop.send(());
            let _ = animation.worker.join();
        }
    }

    fn start_animation(&self, stage: StageLine) -> bool {
        let (stop, stopped) = mpsc::channel();
        let terminal = Arc::clone(&self.terminal);
        let Ok(worker) = thread::Builder::new()
            .name("pekin-progress".into())
            .spawn(move || {
                let length = stage.label.chars().count();
                for visible in 0..length {
                    for phase in 1..=3 {
                        if stopped.recv_timeout(Duration::from_millis(20)).is_ok() {
                            return;
                        }
                        let _terminal = terminal.lock().unwrap_or_else(|error| error.into_inner());
                        Ui::write_stage_line_locked(
                            &mut io::stderr().lock(),
                            &stage,
                            visible,
                            Some(phase),
                            false,
                            false,
                        );
                    }
                    if stopped.recv_timeout(Duration::from_millis(20)).is_ok() {
                        return;
                    }
                    let _terminal = terminal.lock().unwrap_or_else(|error| error.into_inner());
                    Ui::write_stage_line_locked(
                        &mut io::stderr().lock(),
                        &stage,
                        visible + 1,
                        (visible + 1 < length).then_some(0),
                        visible + 1 < length,
                        false,
                    );
                }
            })
        else {
            return false;
        };
        *self
            .animation
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(TypingAnimation { stop, worker });
        true
    }

    fn write_stage_line(
        &self,
        stage: &StageLine,
        visible: usize,
        glitch_phase: Option<usize>,
        cursor: bool,
        newline: bool,
    ) {
        let _terminal = self
            .terminal
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self::write_stage_line_locked(
            &mut io::stderr().lock(),
            stage,
            visible,
            glitch_phase,
            cursor,
            newline,
        );
    }

    fn write_stage_line_locked(
        output: &mut impl Write,
        stage: &StageLine,
        visible: usize,
        glitch_phase: Option<usize>,
        cursor: bool,
        newline: bool,
    ) {
        let _ = write!(output, "\r\x1b[2K");
        if stage.determinate {
            let width = 24;
            let filled = width * stage.current / stage.total;
            let marker = if stage.current == stage.total {
                "✓"
            } else {
                "🦆"
            };
            let _ = write!(
                output,
                "\x1b[{}m{marker}\x1b[0m  \x1b[2mstep {}/{}\x1b[0m  \x1b[{}m{}\x1b[2m{}\x1b[0m  ",
                stage.accent,
                stage.current,
                stage.total,
                stage.accent,
                "━".repeat(filled),
                "─".repeat(width - filled),
            );
        } else {
            let _ = write!(output, "\x1b[{}m🦆\x1b[0m  ", stage.accent);
        }
        let cutoff = scalar_boundary(&stage.label, visible);
        let _ = write!(output, "{}", &stage.label[..cutoff]);
        if let Some(phase) = glitch_phase {
            let _ = write!(output, "\x1b[{}m", stage.accent);
            for (position, _) in stage.label.char_indices().skip(visible) {
                let _ = write!(output, "{}", glitch_glyph(position, phase));
            }
            let _ = write!(output, "\x1b[0m");
        }
        if cursor {
            let _ = write!(output, "\x1b[5;{}m▒\x1b[0m", stage.accent);
        }
        if newline {
            let _ = writeln!(output);
        } else {
            let _ = output.flush();
        }
    }
}

fn scalar_boundary(value: &str, visible: usize) -> usize {
    value
        .char_indices()
        .nth(visible)
        .map_or(value.len(), |(index, _)| index)
}

fn styled_terminal(mode: OutputMode, theme: Theme, is_terminal: bool, no_color: bool) -> bool {
    mode == OutputMode::Human && theme != Theme::Plain && is_terminal && !no_color
}

fn terminal_motion(styled: bool, reduced_motion: bool, ci: bool) -> bool {
    styled && !reduced_motion && !ci
}

fn glitch_glyph(position: usize, phase: usize) -> char {
    const GLYPHS: &[u8] = b"@#$%&*+=?~";
    GLYPHS[(position.wrapping_mul(3).wrapping_add(phase)) % GLYPHS.len()] as char
}

fn stage_render_state(stage: &StageLine, motion: bool) -> (usize, bool, bool, bool) {
    let complete = stage.determinate && stage.current == stage.total;
    let animate = motion && !complete && !stage.label.is_empty();
    (
        if animate { 0 } else { usize::MAX },
        false,
        complete,
        animate,
    )
}

impl Drop for Ui {
    fn drop(&mut self) {
        self.finish_progress();
    }
}

pub fn print_markdown(markdown: &str, theme: Theme) -> Result<()> {
    let styled = theme != Theme::Plain
        && io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none();
    let mut output = io::stdout().lock();
    let accent = terminal_accent(theme);
    for line in markdown.lines() {
        let line = terminal_text(line);
        if !styled {
            writeln!(output, "{line}")?;
            continue;
        }
        let rendered = if let Some(heading) = line.strip_prefix("### ") {
            format!("\x1b[1;4;{accent}m{heading}\x1b[0m")
        } else if let Some(heading) = line.strip_prefix("## ") {
            format!("\x1b[1;{accent}m{heading}\x1b[0m")
        } else if let Some(heading) = line.strip_prefix("# ") {
            format!("\x1b[1;{accent}m{heading}\x1b[0m")
        } else {
            terminal_emphasis(&line)
        };
        writeln!(output, "{rendered}")?;
    }
    Ok(())
}

pub fn json_success_document(kind: &str, result: &serde_json::Value) -> Result<String> {
    #[derive(Serialize)]
    struct Document<'a> {
        schema: u8,
        status: &'static str,
        kind: &'a str,
        result: &'a serde_json::Value,
    }

    Ok(serde_json::to_string_pretty(&Document {
        schema: 1,
        status: "ok",
        kind,
        result,
    })?)
}

fn terminal_accent(theme: Theme) -> &'static str {
    match theme {
        Theme::Auto => "36",
        Theme::Dawn => "38;2;177;83;42",
        Theme::Moss | Theme::Plain => "38;2;28;112;96",
        Theme::Tide => "38;2;30;105;150",
        Theme::Dusk => "38;2;208;155;196",
    }
}

fn terminal_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect()
}

fn terminal_emphasis(line: &str) -> String {
    let mut result = paired_markers(line, "<u>", "</u>", "\x1b[4m", "\x1b[24m");
    result = paired_markers(&result, "**", "**", "\x1b[1m", "\x1b[22m");
    paired_single_asterisks(&result)
}

fn paired_markers(
    value: &str,
    start: &str,
    end: &str,
    open_style: &str,
    close_style: &str,
) -> String {
    let mut result = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start_index) = remaining.find(start) {
        result.push_str(&remaining[..start_index]);
        let content = &remaining[start_index + start.len()..];
        let Some(end_index) = content.find(end) else {
            result.push_str(&remaining[start_index..]);
            return result;
        };
        result.push_str(open_style);
        result.push_str(&content[..end_index]);
        result.push_str(close_style);
        remaining = &content[end_index + end.len()..];
    }
    result.push_str(remaining);
    result
}

fn paired_single_asterisks(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut remaining = value;
    while let Some(start_index) = single_asterisk(remaining) {
        result.push_str(&remaining[..start_index]);
        let content = &remaining[start_index + 1..];
        let Some(end_index) = single_asterisk(content) else {
            result.push_str(&remaining[start_index..]);
            return result;
        };
        result.push_str("\x1b[3m");
        result.push_str(&content[..end_index]);
        result.push_str("\x1b[23m");
        remaining = &content[end_index + 1..];
    }
    result.push_str(remaining);
    result
}

fn single_asterisk(value: &str) -> Option<usize> {
    value.match_indices('*').find_map(|(index, _)| {
        let bytes = value.as_bytes();
        (index.checked_sub(1).and_then(|before| bytes.get(before)) != Some(&b'*')
            && bytes.get(index + 1) != Some(&b'*'))
        .then_some(index)
    })
}

#[cfg(test)]
mod tests;
