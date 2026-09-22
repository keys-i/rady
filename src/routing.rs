use anyhow::bail;
use serde_json::Value;

use crate::Result;

const MAX_CHOICES: usize = 8;
const MAX_CHOICE_CHARS: usize = 200;
const MAX_TEXT_BYTES: usize = 16_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier {
    Fast,
    Balanced,
    Deep,
}

pub fn select(evidence: &Value) -> Tier {
    let (files, changes, patch_bytes) = scopes(evidence)
        .filter_map(|scope| scope["files"].as_array())
        .fold(
            (0_usize, 0_u64, 0_usize),
            |(count, changes, bytes), files| {
                let (changes, bytes) =
                    files
                        .iter()
                        .take(64)
                        .fold((changes, bytes), |(changes, bytes), file| {
                            (
                                changes
                                    .saturating_add(file["additions"].as_u64().unwrap_or_default())
                                    .saturating_add(file["deletions"].as_u64().unwrap_or_default()),
                                bytes.saturating_add(file["patch"].as_str().map_or(0, str::len)),
                            )
                        });
                (count.saturating_add(files.len()), changes, bytes)
            },
        );
    let text_bytes = scopes(evidence)
        .flat_map(|scope| {
            ["task", "request", "title", "description", "body"]
                .into_iter()
                .filter_map(move |name| scope[name].as_str())
        })
        .map(str::len)
        .sum::<usize>();
    let sensitive = scopes(evidence).any(|scope| {
        ["task", "request", "title", "description", "body"]
            .into_iter()
            .filter_map(|name| scope[name].as_str())
            .any(|text| {
                [
                    "security",
                    "vulnerability",
                    "cve-",
                    "secret",
                    "credential",
                    "merge conflict",
                    "conflict",
                ]
                .into_iter()
                .any(|term| contains_ascii(text, term))
            })
    });
    let failed_checks = scopes(evidence).any(|scope| {
        scope["checks"].as_array().is_some_and(|checks| {
            checks.iter().take(128).any(|check| {
                matches!(
                    check["state"].as_str(),
                    Some(
                        "failure"
                            | "cancelled"
                            | "timed_out"
                            | "action_required"
                            | "startup_failure"
                    )
                )
            })
        })
    });

    if sensitive
        || failed_checks
        || scopes(evidence).any(|scope| scope["complete_diff"].as_bool() == Some(false))
        || files > 12
        || changes > 1_500
        || patch_bytes > 20_000
        || text_bytes > 4_000
    {
        Tier::Deep
    } else if files > 0 || changes > 0 || patch_bytes > 0 || text_bytes > 600 {
        Tier::Balanced
    } else {
        Tier::Fast
    }
}

fn scopes(evidence: &Value) -> impl Iterator<Item = &Value> {
    std::iter::once(evidence)
        .chain(evidence.get("issue"))
        .chain(evidence.get("pull_request"))
}

pub fn model_choice<'a>(
    explicit: Option<&'a str>,
    choices: &'a [String],
    tier: Tier,
) -> Result<Option<&'a str>> {
    if let Some(model) = explicit {
        if model.trim().is_empty() || model.chars().count() > MAX_CHOICE_CHARS {
            bail!("model must be nonempty and at most {MAX_CHOICE_CHARS} characters");
        }
        return Ok(Some(model));
    }
    if choices.len() > MAX_CHOICES {
        bail!("at most {MAX_CHOICES} model choices are supported");
    }
    for (index, choice) in choices.iter().enumerate() {
        if choice.trim().is_empty() || choice.chars().count() > MAX_CHOICE_CHARS {
            bail!(
                "model choice {} must be nonempty and at most {MAX_CHOICE_CHARS} characters",
                index + 1
            );
        }
        if choices[..index].iter().any(|previous| previous == choice) {
            bail!("model choices must be distinct");
        }
    }
    let choice = match tier {
        Tier::Fast => choices.first(),
        Tier::Balanced => choices.get(choices.len() / 2),
        Tier::Deep => choices.last(),
    };
    Ok(choice.map(String::as_str))
}

fn contains_ascii(text: &str, term: &str) -> bool {
    text.as_bytes()
        .get(..text.len().min(MAX_TEXT_BYTES))
        .unwrap_or_default()
        .windows(term.len())
        .any(|part| part.eq_ignore_ascii_case(term.as_bytes()))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn routes_evidence_and_orders_choices() -> Result<()> {
        let choices = ["fast", "balanced", "deep"]
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        for (evidence, tier, expected) in [
            (json!({"task": "format this"}), Tier::Fast, "fast"),
            (
                json!({"title": "Update dependency", "files": [{"additions": 2, "deletions": 1, "patch": "+x"}]}),
                Tier::Balanced,
                "balanced",
            ),
            (
                json!({"title": "Fix security vulnerability", "complete_diff": true}),
                Tier::Deep,
                "deep",
            ),
            (
                json!({
                    "request": "Can this merge?",
                    "issue": {"title": "Dependency update", "body": ""},
                    "pull_request": {
                        "files": [{"additions": 2, "deletions": 1, "patch": "+x"}],
                        "complete_diff": true,
                        "checks": [{"state": "failure"}],
                    },
                }),
                Tier::Deep,
                "deep",
            ),
        ] {
            assert_eq!(select(&evidence), tier);
            assert_eq!(model_choice(None, &choices, tier)?, Some(expected));
        }
        Ok(())
    }

    #[test]
    fn explicit_model_wins_and_invalid_choices_fail() {
        let choices = vec!["fast".to_owned(), "fast".to_owned()];
        assert_eq!(
            model_choice(Some("fixed"), &choices, Tier::Deep)
                .unwrap()
                .unwrap(),
            "fixed"
        );
        assert!(model_choice(None, &choices, Tier::Fast).is_err());
        assert!(model_choice(None, &[" ".to_owned()], Tier::Fast).is_err());
    }
}
