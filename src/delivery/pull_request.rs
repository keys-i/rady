use anyhow::{anyhow, bail};
use serde_json::Value;

use super::benchmark::Measurement;
use super::quality::{Plan, ReviewReport};
use crate::Result;

use super::Config;

pub(super) const MAX_PULL_REQUEST_FIELD_BYTES: usize = 32_000;
const MAX_PULL_REQUEST_BODY_BYTES: usize = 60_000;

pub(super) fn pull_request_body(
    config: &Config,
    plan: &Plan,
    report: &ReviewReport,
    verified: &[Value],
    benchmarks: &[Measurement],
) -> Result<String> {
    let mut body = String::from("## Requested change\n\n");
    push_pr_text(&mut body, "task", &config.task)?;
    body.push_str("\n\n## Review summary\n\n");
    push_pr_text(&mut body, "review summary", &report.summary)?;
    body.push_str("\n\n## Validation\n");
    for check in &config.checks {
        body.push_str("- Passed: ");
        push_pr_text(&mut body, "check", check)?;
        body.push('\n');
    }
    body.push_str("\n## Review gates\n");
    for (name, gate) in &report.gates {
        body.push_str("- ");
        push_pr_text(&mut body, "gate name", name)?;
        body.push_str(&format!(": {:?} — ", gate.status));
        push_pr_text(&mut body, "gate evidence", &gate.evidence)?;
        body.push('\n');
    }
    body.push_str("\n## Acceptance evidence\n");
    for item in &report.acceptance {
        let criterion = plan
            .acceptance
            .get(item.criterion)
            .ok_or_else(|| anyhow!("review cited an unknown acceptance criterion"))?;
        body.push_str("- ");
        push_pr_text(&mut body, "acceptance criterion", criterion)?;
        body.push_str(&format!(": {:?} — ", item.status));
        push_pr_text(&mut body, "acceptance evidence", &item.evidence)?;
        body.push('\n');
    }
    body.push_str("\n## Fixed acceptance checks\n");
    for item in verified {
        body.push_str(&format!(
            "- Criterion {}: ",
            item["criterion"].as_u64().unwrap_or(0) + 1,
        ));
        push_pr_text(
            &mut body,
            "acceptance command",
            item["command"].as_str().unwrap_or_default(),
        )?;
        body.push_str(" — ");
        push_pr_text(
            &mut body,
            "acceptance status",
            item["status"].as_str().unwrap_or("unknown"),
        )?;
        body.push('\n');
    }
    body.push_str("\n## Benchmark\n\n");
    let benchmark = if benchmarks.is_empty() {
        "Not run; no benchmark supplied".to_owned()
    } else {
        serde_json::to_string(benchmarks).unwrap_or_else(|_| "Unavailable".to_owned())
    };
    push_pr_text(&mut body, "benchmark", &benchmark)?;
    body.push_str("\n\n## Limitations\n");
    for limitation in report.limitations.iter().chain(&plan.limitations) {
        body.push_str("- ");
        push_pr_text(&mut body, "limitation", limitation)?;
        body.push('\n');
    }
    if body.len() > MAX_PULL_REQUEST_BODY_BYTES {
        bail!(
            "pull request body is too large after safely rendering evidence; reduce the task or evidence"
        );
    }
    Ok(body)
}

fn push_pr_text(body: &mut String, field: &str, value: &str) -> Result<()> {
    let rendered = markdown_text(value);
    if rendered.len() > MAX_PULL_REQUEST_FIELD_BYTES {
        bail!("{field} is too large to publish safely in a pull request");
    }
    body.push_str(&rendered);
    Ok(())
}

pub(super) fn markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len().min(MAX_PULL_REQUEST_FIELD_BYTES + 1));
    for character in value.chars() {
        if escaped.len() > MAX_PULL_REQUEST_FIELD_BYTES {
            break;
        }
        if unsafe_text_control(character) {
            escaped.push(' ');
        } else if character == '@' {
            escaped.push_str("@\u{200b}");
        } else if character == '&' {
            escaped.push_str("&amp;");
        } else {
            if matches!(
                character,
                '\\' | '`'
                    | '*'
                    | '_'
                    | '{'
                    | '}'
                    | '['
                    | ']'
                    | '('
                    | ')'
                    | '<'
                    | '>'
                    | '#'
                    | '!'
                    | '|'
                    | '~'
                    | '^'
                    | '$'
                    | '+'
                    | '-'
                    | '.'
            ) {
                escaped.push('\\');
            }
            escaped.push(character);
        }
    }
    escaped
}

pub(super) fn github_plain_text(value: &str) -> String {
    let mut safe = String::with_capacity(value.len());
    for character in value.chars() {
        if unsafe_text_control(character) {
            safe.push(' ');
        } else if character == '@' {
            safe.push_str("@\u{200b}");
        } else if character == '&' {
            safe.push_str("and");
        } else {
            safe.push(character);
        }
    }
    safe
}

pub(super) fn pull_request_title(task: &str) -> String {
    task.lines()
        .map(|line| github_plain_text(line.trim_start_matches(['#', ' '])))
        .find(|line| !line.trim().is_empty())
        .map(|line| line.chars().take(72).collect())
        .unwrap_or_else(|| "Implement the requested specification".to_owned())
}

fn unsafe_text_control(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
        )
}
