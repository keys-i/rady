use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context;
use latex2mathml::{DisplayStyle, latex_to_mathml};
use pulldown_cmark::{CowStr, Event, Options, Parser, Tag, TagEnd};

use crate::Result;

use super::{ReportState, Theme};

const PEKIN_DUCK_PNG: &[u8] = include_bytes!("../../assets/pekin-app-duck.png");

pub fn write_report(
    path: &Path,
    title: &str,
    markdown: &str,
    state: ReportState,
    theme: Theme,
) -> Result<()> {
    let body = markdown_html(markdown)?;
    let title = escape_html(title);
    let theme_name = match theme {
        Theme::Auto => "auto",
        Theme::Dawn => "dawn",
        Theme::Moss => "moss",
        Theme::Tide => "tide",
        Theme::Dusk => "dusk",
        Theme::Plain => "dawn",
    };
    let (state_name, state_label, state_detail, state_attributes) = match state {
        ReportState::Active => (
            "active",
            "Working",
            "The evidence will stay here as it arrives",
            r#"role="status" aria-label="Run in progress""#,
        ),
        ReportState::Complete => (
            "complete",
            "Ready to inspect",
            "Evidence kept with this run",
            r#"role="progressbar" aria-label="Run complete" aria-valuemin="0" aria-valuemax="100" aria-valuenow="100""#,
        ),
        ReportState::Stopped => (
            "stopped",
            "Stopped safely",
            "Everything collected so far is still here",
            r#"role="status" aria-label="Run stopped""#,
        ),
    };
    let mascot = format!("data:image/png;base64,{}", base64(PEKIN_DUCK_PNG));
    let document = REPORT_TEMPLATE
        .replace("{{theme}}", theme_name)
        .replace("{{title}}", &title)
        .replace("{{state}}", state_name)
        .replace("{{state_label}}", state_label)
        .replace("{{state_detail}}", state_detail)
        .replace("{{state_attributes}}", state_attributes)
        .replace("{{mascot}}", &mascot)
        .replace(
            "{{auto_checked}}",
            if theme == Theme::Auto { "checked" } else { "" },
        )
        .replace(
            "{{dawn_checked}}",
            if matches!(theme, Theme::Dawn | Theme::Plain) {
                "checked"
            } else {
                ""
            },
        )
        .replace(
            "{{moss_checked}}",
            if theme == Theme::Moss { "checked" } else { "" },
        )
        .replace(
            "{{tide_checked}}",
            if theme == Theme::Tide { "checked" } else { "" },
        )
        .replace(
            "{{dusk_checked}}",
            if theme == Theme::Dusk { "checked" } else { "" },
        )
        .replace("{{body}}", &body);
    write_report_atomically(path, document.as_bytes())
}

static REPORT_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn write_report_atomically(path: &Path, document: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .context("report path must name a file")?
        .to_string_lossy();
    let temporary = (0..32)
        .find_map(|_| {
            let sequence = REPORT_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let candidate = parent.join(format!(
                ".{file_name}.{}.{}.tmp",
                std::process::id(),
                sequence
            ));
            match fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&candidate)
            {
                Ok(file) => Some(Ok((candidate, file))),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .transpose()?
        .context("could not create a unique temporary report file")?;
    let (temporary_path, mut temporary_file) = temporary;

    let write_result = (|| -> io::Result<()> {
        temporary_file.write_all(document)?;
        temporary_file.sync_all()
    })();
    drop(temporary_file);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&temporary_path);
        return Err(error).with_context(|| format!("could not write report {}", path.display()));
    }

    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error).with_context(|| format!("could not replace report {}", path.display()));
    }
    Ok(())
}

pub(super) fn markdown_html(markdown: &str) -> Result<String> {
    if markdown.len() > 512 * 1024 {
        anyhow::bail!("report Markdown exceeds 512 KiB");
    }
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_MATH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_SUPERSCRIPT
        | Options::ENABLE_SUBSCRIPT;
    let parser = Parser::new_ext(markdown, options).map(safe_event);
    let mut output = String::with_capacity(markdown.len() + markdown.len() / 2);
    pulldown_cmark::html::push_html(&mut output, parser);
    Ok(output)
}

fn safe_event<'a>(event: Event<'a>) -> Event<'a> {
    match event {
        Event::InlineMath(source) => math_event(&source, DisplayStyle::Inline),
        Event::DisplayMath(source) => math_event(&source, DisplayStyle::Block),
        Event::Html(value) | Event::InlineHtml(value)
            if matches!(value.as_ref(), "<u>" | "</u>" | "<mark>" | "</mark>") =>
        {
            Event::Html(value)
        }
        Event::Html(value) | Event::InlineHtml(value) => Event::Text(value),
        Event::Start(Tag::Image { .. }) => Event::Text(CowStr::Borrowed("[Image: ")),
        Event::End(TagEnd::Image) => Event::Text(CowStr::Borrowed("]")),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let destination = if safe_link(&dest_url) {
                dest_url
            } else {
                CowStr::Borrowed("#")
            };
            Event::Start(Tag::Link {
                link_type,
                dest_url: destination,
                title,
                id,
            })
        }
        other => other,
    }
}

fn math_event(source: &str, style: DisplayStyle) -> Event<'static> {
    match latex_to_mathml(source, style) {
        Ok(mathml) => Event::Html(CowStr::Boxed(mathml.into_boxed_str())),
        Err(_) => Event::Text(CowStr::Boxed(escape_html(source).into_boxed_str())),
    }
}

fn safe_link(destination: &str) -> bool {
    let lower = destination.trim().to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with('#')
        || (!lower.contains(':') && !lower.starts_with("//"))
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    let mut chunks = bytes.chunks_exact(3);
    for chunk in &mut chunks {
        encoded.push(ALPHABET[(chunk[0] >> 2) as usize] as char);
        encoded.push(ALPHABET[(((chunk[0] & 3) << 4) | (chunk[1] >> 4)) as usize] as char);
        encoded.push(ALPHABET[(((chunk[1] & 15) << 2) | (chunk[2] >> 6)) as usize] as char);
        encoded.push(ALPHABET[(chunk[2] & 63) as usize] as char);
    }
    let remainder = chunks.remainder();
    if let [first] = remainder {
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[((first & 3) << 4) as usize] as char);
        encoded.push_str("==");
    } else if let [first, second] = remainder {
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 3) << 4) | (second >> 4)) as usize] as char);
        encoded.push(ALPHABET[((second & 15) << 2) as usize] as char);
        encoded.push('=');
    }
    encoded
}

const REPORT_TEMPLATE: &str = r#"<!doctype html>
<html lang="en" data-theme="{{theme}}">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta name="color-scheme" content="light dark">
  <title>{{title}} | Pekin</title>
  <style>
    :root { color-scheme:light; --canvas:#eef3ec; --paper:#fbfcf8; --ink:#17241d; --muted:#526158; --edge:#cad5cc; --accent:#286b4f; --accent-strong:#174b36; --accent-wash:#dbe9de; --code-bg:#121b18; --code-ink:#c9f4da; --warning:#9b4936; --shadow:#1830251c; --sheet-radius:9px; --control-radius:5px; --ease:cubic-bezier(.16,1,.3,1); --mono:"SFMono-Regular","Cascadia Code",Menlo,Consolas,monospace; }
    html[data-theme="dawn"],html:has(input[value="dawn"]:checked) { color-scheme:light; --canvas:#f1eee7; --paper:#fcfaf5; --ink:#292822; --muted:#68655c; --edge:#d7d1c6; --accent:#9f4e32; --accent-strong:#74331f; --accent-wash:#efded4; --code-bg:#24231e; --code-ink:#f5d5a5; --warning:#9f3d31; --shadow:#39291d1b; }
    html[data-theme="moss"],html:has(input[value="moss"]:checked) { color-scheme:light; --canvas:#eef3ec; --paper:#fbfcf8; --ink:#17241d; --muted:#526158; --edge:#cad5cc; --accent:#286b4f; --accent-strong:#174b36; --accent-wash:#dbe9de; --code-bg:#121b18; --code-ink:#c9f4da; --warning:#9b4936; --shadow:#1830251c; }
    html[data-theme="tide"],html:has(input[value="tide"]:checked) { color-scheme:light; --canvas:#edf4f5; --paper:#f9fcfc; --ink:#14252b; --muted:#52666c; --edge:#c8d8da; --accent:#1f6a75; --accent-strong:#124b54; --accent-wash:#d8e9eb; --code-bg:#102126; --code-ink:#bdeef3; --warning:#a14737; --shadow:#152f351b; }
    html[data-theme="dusk"],html:has(input[value="dusk"]:checked) { color-scheme:dark; --canvas:#090d17; --paper:#111827; --ink:#eef2ff; --muted:#aab3c8; --edge:#34405e; --accent:#5ee6c4; --accent-strong:#9cf5df; --accent-wash:#18323a; --code-bg:#070b12; --code-ink:#78f7d5; --warning:#ff8a78; --shadow:#02040a73; }
    @media (prefers-color-scheme:dark) { html[data-theme="auto"],html:has(input[value="auto"]:checked) { color-scheme:dark; --canvas:#090d17; --paper:#111827; --ink:#eef2ff; --muted:#aab3c8; --edge:#34405e; --accent:#5ee6c4; --accent-strong:#9cf5df; --accent-wash:#18323a; --code-bg:#070b12; --code-ink:#78f7d5; --warning:#ff8a78; --shadow:#02040a73; } }
    html:has(input[value="auto"]:checked) { --canvas:#eef3ec; --paper:#fbfcf8; --ink:#17241d; --muted:#526158; --edge:#cad5cc; --accent:#286b4f; --accent-strong:#174b36; --accent-wash:#dbe9de; --code-bg:#121b18; --code-ink:#c9f4da; --warning:#9b4936; --shadow:#1830251c; }
    @media (prefers-color-scheme:dark) { html:has(input[value="auto"]:checked) { --canvas:#090d17; --paper:#111827; --ink:#eef2ff; --muted:#aab3c8; --edge:#34405e; --accent:#5ee6c4; --accent-strong:#9cf5df; --accent-wash:#18323a; --code-bg:#070b12; --code-ink:#78f7d5; --warning:#ff8a78; --shadow:#02040a73; } }
    * { box-sizing:border-box; }
    html { min-width:20rem; background:var(--canvas); color:var(--ink); font-family:"SF Pro Text","Segoe UI Variable Text",system-ui,sans-serif; font-synthesis:none; }
    body { min-height:100dvh; margin:0; padding:clamp(1rem,4vw,4rem); background:repeating-linear-gradient(0deg,color-mix(in srgb,var(--ink) 1.8%,transparent) 0 1px,transparent 1px 4px),var(--canvas); color:var(--ink); line-height:1.65; }
    ::selection { background:var(--accent); color:var(--paper); }
    .report { display:grid; grid-template-columns:minmax(14rem,18rem) minmax(0,50rem); align-items:start; justify-content:center; gap:clamp(2rem,5vw,5rem); width:min(100%,76rem); margin:auto; }
    .rail { position:sticky; top:clamp(1rem,4vw,4rem); display:grid; gap:clamp(1.4rem,3vw,2.4rem); }
    .identity { display:grid; gap:.65rem; justify-items:start; }
    .duck-frame { position:relative; display:block; width:min(100%,13.5rem); transform-origin:50% 90%; transition:transform .18s var(--ease),filter .18s ease; }
    .duck-frame::after { position:absolute; right:-.2rem; bottom:.45rem; padding:.2rem .38rem; border:1px solid var(--edge); border-radius:999px; background:var(--paper); box-shadow:0 .2rem .5rem var(--shadow); color:var(--accent-strong); content:"ON WATCH"; font:700 .58rem/1 var(--mono); letter-spacing:.08em; }
    .duck { display:block; width:100%; height:auto; filter:drop-shadow(0 .65rem 1rem var(--shadow)); transform-origin:50% 90%; }
    .product,.artifact { display:block; }
    .product { font:750 .82rem/1.2 var(--mono); letter-spacing:.1em; }
    .artifact { color:var(--muted); font: .74rem/1.35 var(--mono); }
    .stage { display:grid; gap:1rem; }
    .rail h1 { max-width:13ch; margin:0; font-size:clamp(2rem,4.4vw,3.7rem); font-variation-settings:"wght" 680,"opsz" 42; line-height:1.03; letter-spacing:-.038em; overflow-wrap:normal; }
    .progress { position:relative; height:.44rem; overflow:hidden; border:1px solid var(--edge); border-radius:999px; background:repeating-linear-gradient(90deg,var(--accent-wash) 0 .55rem,transparent .55rem .72rem); box-shadow:inset 0 1px 2px var(--shadow); }
    .progress span { display:block; height:100%; border-radius:inherit; background:linear-gradient(90deg,var(--accent-strong),var(--accent)); transform-origin:left; }
    .progress.active span { width:34%; }
    .progress.complete span { width:100%; }
    .progress.stopped span { width:100%; background:var(--warning); }
    .status-label { display:flex; justify-content:space-between; gap:.8rem; color:var(--muted); font: .74rem/1.4 var(--mono); }
    .status-label strong { color:var(--ink); font-weight:700; }
    fieldset { display:grid; gap:.2rem; margin:0; padding:1rem 0 0; border:0; border-top:1px solid var(--edge); }
    legend { margin-bottom:.45rem; padding:0; color:var(--muted); font:700 .7rem/1.3 var(--mono); letter-spacing:.08em; text-transform:uppercase; }
    .theme-choice { display:grid; grid-template-columns:auto 1fr; align-items:center; gap:.6rem; padding:.42rem .5rem; border:1px solid transparent; border-radius:var(--control-radius); color:var(--muted); font: .76rem/1.35 var(--mono); cursor:pointer; transition:background-color .16s ease,color .16s ease,transform .16s var(--ease); }
    .theme-choice:hover { background:color-mix(in srgb,var(--accent-wash) 55%,transparent); color:var(--ink); }
    .theme-choice input { width:1rem; height:1rem; margin:0; accent-color:var(--accent); }
    .theme-choice input:checked + span { color:var(--ink); font-weight:700; text-decoration:underline; text-decoration-color:var(--accent); text-decoration-thickness:2px; text-underline-offset:.22em; }
    .theme-choice:has(input:checked) { border-color:color-mix(in srgb,var(--accent) 30%,transparent); background:color-mix(in srgb,var(--accent-wash) 45%,transparent); }
    .theme-choice:has(input:focus-visible) { outline:3px solid color-mix(in srgb,var(--accent) 50%,transparent); outline-offset:2px; }
    article { position:relative; min-width:0; padding:clamp(1.5rem,5vw,4.5rem); border:1px solid var(--edge); border-radius:var(--sheet-radius); background:var(--paper); box-shadow:inset .25rem 0 0 color-mix(in srgb,var(--accent) 42%,transparent),0 1.4rem 4rem var(--shadow); overflow-wrap:anywhere; }
    article::before { position:absolute; top:-1px; right:clamp(1.2rem,4vw,3rem); width:clamp(3rem,9vw,6rem); height:3px; background:var(--accent); content:""; }
    article > :first-child { margin-top:0; }
    article > :last-child { margin-bottom:0; }
    article h1 { margin:0 0 1.5rem; font-size:clamp(2rem,5vw,4rem); line-height:1.04; letter-spacing:-.045em; }
    h2 { max-width:22ch; margin:clamp(2.8rem,7vw,5rem) 0 1rem; font-size:clamp(1.45rem,3vw,2.35rem); font-variation-settings:"wght" 690,"opsz" 32; line-height:1.08; letter-spacing:-.035em; }
    h3 { margin:2.2rem 0 .7rem; font-size:clamp(1.1rem,2vw,1.35rem); line-height:1.2; }
    p,li,dd { max-width:72ch; }
    p { margin:0 0 1.1rem; }
    ul,ol { padding-left:1.4rem; }
    li + li { margin-top:.38rem; }
    .task-list-item { list-style:none; margin-left:-1.35rem; }
    .task-list-item input { margin-right:.65rem; accent-color:var(--accent); }
    a { color:var(--accent-strong); font-weight:620; text-decoration-thickness:.08em; text-underline-offset:.22em; transition:opacity .16s ease; }
    a:hover { opacity:.72; }
    a:focus-visible { outline:3px solid color-mix(in srgb,var(--accent) 50%,transparent); outline-offset:3px; border-radius:3px; }
    strong { font-weight:760; }
    em { font-variation-settings:"slnt" -8; }
    u { text-decoration-thickness:.09em; text-underline-offset:.18em; }
    mark { padding:.08em .18em; background:var(--accent-wash); color:var(--ink); }
    code { padding:.12rem .36rem; border:1px solid color-mix(in srgb,var(--edge) 72%,transparent); border-radius:3px; background:var(--accent-wash); color:var(--ink); font: .9em/1.5 var(--mono); }
    pre { max-width:100%; overflow:auto; margin:1.5rem 0; padding:1rem 1.1rem; border:1px solid color-mix(in srgb,var(--accent) 50%,var(--edge)); border-radius:var(--control-radius); background:var(--code-bg); box-shadow:inset .2rem 0 0 var(--accent); color:var(--code-ink); tab-size:2; }
    pre code { padding:0; border:0; background:none; color:inherit; white-space:pre; }
    blockquote { margin:1.8rem 0; padding:.15rem 0 .15rem 1.15rem; border-left:3px solid var(--accent); color:var(--muted); font-size:1.04em; }
    blockquote > :last-child { margin-bottom:0; }
    hr { height:1px; margin:3rem 0; border:0; background:var(--edge); }
    table { display:block; width:100%; max-width:100%; overflow:auto; border-collapse:collapse; margin:1.5rem 0; font-size:.92rem; }
    th,td { min-width:8rem; padding:.65rem .8rem; border-bottom:1px solid var(--edge); text-align:left; vertical-align:top; }
    th { color:var(--muted); font:700 .72rem/1.4 var(--mono); letter-spacing:.05em; text-transform:uppercase; }
    math { font-size:1.05em; }
    math[display="block"] { max-width:100%; overflow:auto; margin:2rem 0; color:var(--ink); }
    .footnote-definition { color:var(--muted); font-size:.88rem; }
    @media (prefers-reduced-motion:no-preference) { .progress.active span { animation:progress-arrive .28s var(--ease) both; transform-origin:left; } .duck-frame { animation:duck-arrive .42s var(--ease) backwards; } .duck-frame:hover { filter:brightness(1.03); transform:translateY(-3px) rotate(1deg) scale(1.015); } .theme-choice:has(input:checked) { animation:theme-settle .2s var(--ease); } .theme-choice:active,a:active { transform:translateY(1px); } }
    @keyframes progress-arrive { from { transform:scaleX(0); } to { transform:scaleX(1); } }
    @keyframes duck-arrive { from { opacity:0; transform:translateY(.45rem) rotate(-1deg) scale(.98); } }
    @keyframes theme-settle { from { transform:translateX(-.18rem); } }
    @media (max-width:780px) { body { padding:1rem; } .report { grid-template-columns:1fr; gap:1.5rem; } .rail { position:static; grid-template-columns:1fr; gap:1.2rem; } .rail h1 { max-width:18ch; font-size:clamp(2rem,11vw,3.5rem); } fieldset { grid-template-columns:repeat(2,minmax(0,1fr)); } legend { grid-column:1/-1; } article { padding:clamp(1.25rem,6vw,2rem); border-radius:var(--sheet-radius); } }
    @media (max-width:420px) { fieldset { grid-template-columns:1fr; } }
    @media (prefers-reduced-motion:reduce) { *,*::before,*::after { scroll-behavior:auto!important; animation-duration:.01ms!important; animation-iteration-count:1!important; transition-duration:.01ms!important; } .progress.active span { transform:none; } }
    @media (prefers-reduced-transparency:reduce) { body { background:var(--canvas); } }
    @media (forced-colors:active) { .duck-frame,.progress,article,pre,code { forced-color-adjust:auto; } .progress span { background:Highlight; } }
    @media print { body { padding:0; background:white; } .report { display:block; width:auto; } .rail { position:static; margin-bottom:2rem; } fieldset { display:none; } article { padding:0; border:0; box-shadow:none; } .progress.active { display:none; } }
  </style>
</head>
<body>
  <main class="report">
    <header class="rail">
      <div class="identity">
        <span class="duck-frame"><img class="duck" src="{{mascot}}" width="489" height="512" alt="Pekin duck inspecting a keyboard"></span>
        <span><span class="product">Pekin</span><span class="artifact">Duck on watch · evidence report</span></span>
      </div>
      <div class="stage">
        <h1>{{title}}</h1>
        <div class="progress {{state}}" {{state_attributes}}><span></span></div>
        <div class="status-label"><strong>{{state_label}}</strong><span>{{state_detail}}</span></div>
      </div>
      <fieldset>
        <legend>Reading theme</legend>
        <label class="theme-choice"><input type="radio" name="theme" value="auto" {{auto_checked}}><span>System</span></label>
        <label class="theme-choice"><input type="radio" name="theme" value="dawn" {{dawn_checked}}><span>Paper tape</span></label>
        <label class="theme-choice"><input type="radio" name="theme" value="moss" {{moss_checked}}><span>Phosphor</span></label>
        <label class="theme-choice"><input type="radio" name="theme" value="tide" {{tide_checked}}><span>Vector</span></label>
        <label class="theme-choice"><input type="radio" name="theme" value="dusk" {{dusk_checked}}><span>Midnight</span></label>
      </fieldset>
    </header>
    <article id="evidence">{{body}}</article>
  </main>
</body>
</html>
"#;
