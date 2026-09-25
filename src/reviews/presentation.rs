use std::collections::BTreeSet;

use serde_json::Value;

use crate::Result;
use crate::model::{ModelReview, Risk};

use super::{allowed_dependency_path, ci_blockers};

fn approval_protection_ready(protection: Option<&Value>) -> bool {
    let Some(protection) = protection else {
        return false;
    };
    let status = &protection["required_status_checks"];
    protection["enforce_admins"]["enabled"].as_bool() == Some(true)
        && status["strict"].as_bool() == Some(true)
        && (status["contexts"]
            .as_array()
            .is_some_and(|contexts| !contexts.is_empty())
            || status["checks"]
                .as_array()
                .is_some_and(|checks| !checks.is_empty()))
}

pub fn decision(
    review: &ModelReview,
    context: &Value,
    required: &[String],
    protection: Option<&Value>,
) -> Result<(&'static str, Vec<String>)> {
    let mut blockers = review.blockers.clone();
    blockers.extend(ci_blockers(
        context["checks"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default(),
        required,
        protection,
    )?);
    if context["complete_diff"].as_bool() != Some(true) {
        blockers.push("Some changes are binary, too large or missing from the supplied diff; review those manually".to_owned());
    }
    if context["dependency"].as_bool() == Some(true) {
        blockers.extend(dependency_file_blockers(
            context["files"]
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or_default(),
        ));
        if !matches!(
            context["update_type"].as_str(),
            Some("version-update:semver-patch" | "version-update:semver-minor")
        ) {
            blockers.push(
                "This is not a verified patch or minor update; review it manually".to_owned(),
            );
        }
        if context["maintainer_changes"] != "false" {
            blockers.push(
                "Maintainer changes have not been ruled out; check package ownership".to_owned(),
            );
        }
        if !approval_protection_ready(protection) {
            blockers.push(
                "The base branch is not protected with strict required checks; merge this manually"
                    .to_owned(),
            );
        }
    }
    blockers.sort();
    blockers.dedup();
    let event = if blockers.is_empty() && review.risk == Risk::Low {
        "APPROVE"
    } else {
        "COMMENT"
    };
    Ok((event, blockers))
}

pub fn render(
    review: &ModelReview,
    context: &Value,
    event: &str,
    blockers: &[String],
    marker: &str,
) -> String {
    let ready = event == "APPROVE";
    let head = context["head"].as_str().unwrap_or_default();
    let checks = context["checks"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let files = context["files"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut lines = vec![
        marker.to_owned(),
        format!(
            "### {}",
            if ready {
                "Ready to merge"
            } else {
                "Needs attention"
            }
        ),
        String::new(),
        format!(
            "Reviewed head: `{}` · CI snapshot: {}",
            head.chars().take(12).collect::<String>(),
            ci_snapshot(checks)
        ),
        String::new(),
    ];
    if ready {
        lines.push(
            "Everything Rady could verify is clear; GitHub still decides whether this branch can merge."
                .to_owned(),
        );
    } else {
        lines.push("This isn’t ready to merge yet.".to_owned());
    }
    lines.extend([String::new(), "What changed".to_owned()]);
    if files.is_empty() {
        lines.push("- No changed-file details were supplied".to_owned());
    } else {
        lines.extend(files.iter().take(12).map(|file| {
            format!(
                "- `{}` · +{} −{}",
                markdown_text(file["filename"].as_str().unwrap_or("unknown")),
                file["additions"].as_u64().unwrap_or_default(),
                file["deletions"].as_u64().unwrap_or_default(),
            )
        }));
        if files.len() > 12 {
            lines.push(format!("- …and {} more files", files.len() - 12));
        }
    }
    lines.extend([String::new(), "Checks".to_owned()]);
    if checks.is_empty() {
        lines.push("- No GitHub check results were supplied".to_owned());
    } else {
        lines.extend(checks.iter().take(20).map(|item| {
            format!(
                "- {} — {}",
                safe_https_link(
                    item["name"].as_str().unwrap_or("unknown"),
                    item["url"].as_str().unwrap_or_default()
                ),
                markdown_text(item["state"].as_str().unwrap_or("unknown"))
            )
        }));
        if checks.len() > 20 {
            lines.push(format!("- …and {} more checks", checks.len() - 20));
        }
    }
    lines.extend([
        String::new(),
        "What to do next".to_owned(),
        next_action(event, blockers).to_owned(),
        String::new(),
        markdown_text(&review.summary),
    ]);
    if !review.observations.is_empty() {
        lines.extend([String::new(), "Notes".to_owned()]);
    }
    lines.extend(
        review
            .observations
            .iter()
            .map(|item| format!("- {}", markdown_text(item))),
    );
    if !blockers.is_empty() {
        lines.extend([String::new(), "Before merge".to_owned()]);
        lines.extend(
            blockers
                .iter()
                .map(|item| format!("- {}", markdown_text(item))),
        );
    }
    if event != "APPROVE" && context["dependency"].as_bool() == Some(true) {
        repair_handoff(&mut lines, review, context, blockers);
    }
    if !review.minor.is_empty() {
        lines.extend([String::new(), "Small things".to_owned()]);
        lines.extend(
            review
                .minor
                .iter()
                .map(|item| format!("- {}", markdown_text(item))),
        );
    }
    lines.extend([
        String::new(),
        format!("Risk: {:?}", review.risk),
        String::new(),
        "I reviewed the supplied diff and GitHub check results; I didn’t run tests.".to_owned(),
    ]);
    let body = lines[1..]
        .join("\n")
        .replace('@', "@\u{200b}")
        .replace("<!--", "&lt;!--");
    format!("{marker}\n{body}")
}

fn dependency_file_blockers(files: &[Value]) -> Vec<String> {
    if files.is_empty() {
        return vec!["Dependabot changed no inspectable files; review it manually".to_owned()];
    }
    let unexpected = files.iter().any(|file| {
        !file["filename"]
            .as_str()
            .is_some_and(allowed_dependency_path)
            || file["previous_filename"]
                .as_str()
                .is_some_and(|path| !allowed_dependency_path(path))
    });
    unexpected
        .then(|| {
            "Dependabot changed files outside approved dependency files; review it manually"
                .to_owned()
        })
        .into_iter()
        .collect()
}

fn ci_wait_only(blockers: &[String]) -> bool {
    !blockers.is_empty()
        && blockers.iter().all(|blocker| {
            blocker.starts_with('`')
                && (blocker.contains(" hasn't reported for this commit yet")
                    || blocker.contains(" is still running ("))
        })
}

fn repair_handoff(
    lines: &mut Vec<String>,
    review: &ModelReview,
    context: &Value,
    blockers: &[String],
) {
    if ci_wait_only(blockers) {
        return;
    }
    let number = context["number"]
        .as_u64()
        .map_or_else(|| "unknown".to_owned(), |value| format!("#{value}"));
    let url = context["url"]
        .as_str()
        .filter(|value| value.starts_with("https://"));
    let source = url.map_or(number.clone(), |value| format!("{number} {value}"));
    let head = context["head"].as_str().unwrap_or("unknown");
    let mut unique = BTreeSet::new();
    unique.extend(
        blockers
            .iter()
            .filter(|blocker| !blocker.trim().is_empty())
            .cloned(),
    );
    lines.extend([
        "Fix the blockers below. If code needs to change, open a maintainer replacement PR instead of pushing to the Dependabot branch.".to_owned(),
        String::new(),
        "For the follow-up PR".to_owned(),
        format!("- Source: {}", markdown_text(&source)),
        format!("- Reviewed head: {}", markdown_text(head)),
        format!("- Review risk: {:?}", review.risk),
    ]);
    if let Some(update_type) = context["update_type"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("- Update type: {}", markdown_text(update_type)));
    }
    if let Some(maintainer_changes) = context["maintainer_changes"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        lines.push(format!(
            "- Maintainer changes: {}",
            markdown_text(maintainer_changes)
        ));
    }
    lines.extend([
        "- Allowed files: approved manifests, lockfiles, Actions workflows and Dockerfiles only"
            .to_owned(),
        "- Required outcome: every configured and protected check passes on this commit".to_owned(),
        "- Blockers:".to_owned(),
    ]);
    lines.extend(unique.into_iter().take(12).map(|blocker| {
        format!(
            "  - {}",
            markdown_text(&blocker.chars().take(500).collect::<String>())
        )
    }));
}

fn safe_https_link(label: &str, url: &str) -> String {
    let label = markdown_text(label);
    if url.starts_with("https://")
        && !url.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || character == '<'
                || character == '>'
        })
    {
        format!("[{label}](<{url}>)")
    } else {
        label
    }
}

fn ci_snapshot(checks: &[Value]) -> String {
    let successful = checks
        .iter()
        .filter(|check| check["state"].as_str() == Some("success"))
        .count();
    format!("{successful}/{} checks successful", checks.len())
}

fn next_action(event: &str, blockers: &[String]) -> &'static str {
    if event == "APPROVE" {
        "Review the Files changed tab, then use GitHub's Merge control when it is enabled"
    } else if ci_wait_only(blockers) {
        "Wait for the named checks to finish on the reviewed head, then rerun Rady"
    } else {
        "Resolve the blockers below, then rerun Rady on the new head"
    }
}

fn markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if character.is_control() {
            escaped.push(' ');
            continue;
        }
        if matches!(
            character,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '#' | '!' | '|' | '~' | '^' | '$'
        ) {
            escaped.push('\\');
        }
        if character == '&' {
            escaped.push_str("&amp;");
        } else {
            escaped.push(character);
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn protection_requires_admin_enforcement_and_strict_checks() -> Result<()> {
        let success = |name: &str| json!({"name": name, "state": "success", "app_id": null});
        let protected = json!({"enforce_admins": {"enabled": true}, "required_status_checks": {"strict": true, "contexts": ["protected"], "checks": []}});
        let unprotected = json!({"enforce_admins": {"enabled": false}, "required_status_checks": {"strict": false, "contexts": [], "checks": []}});
        for (protection, required, rows, blockers, ready) in [
            (None, vec!["test"], vec![success("test")], 0, false),
            (
                Some(unprotected),
                vec!["test"],
                vec![success("test")],
                0,
                false,
            ),
            (
                Some(protected),
                vec!["test"],
                vec![success("test"), success("protected")],
                0,
                true,
            ),
            (
                Some(json!({"required_status_checks": {"contexts": ["protected"]}})),
                vec!["test"],
                vec![success("test")],
                1,
                false,
            ),
        ] {
            assert_eq!(
                ci_blockers(
                    &rows,
                    &required.into_iter().map(str::to_owned).collect::<Vec<_>>(),
                    protection.as_ref()
                )?
                .len(),
                blockers
            );
            assert_eq!(approval_protection_ready(protection.as_ref()), ready);
        }
        Ok(())
    }

    #[test]
    fn rendering_neutralises_mentions_and_hidden_markers() {
        let review = ModelReview {
            summary: "Reviewed. @person &#64;team &commat;team <!-- marker [click](https://example.test)\n# fake".to_owned(),
            risk: Risk::Low,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let body = render(
            &review,
            &json!({"dependency": false, "head": "a".repeat(40), "checks": []}),
            "COMMENT",
            &[],
            "<!-- safe -->",
        );
        assert!(body.contains("@\u{200b}person"));
        assert!(body.contains("&amp;\\#64;team"));
        assert!(body.contains("&amp;commat;team"));
        assert!(!body.contains(" &#64;team"));
        assert!(!body.contains(" &commat;team"));
        assert!(!body["<!-- safe -->".len()..].contains("<!--"));
        assert!(body.contains(r"\[click\]"));
        assert!(!body.contains("\n# fake"));
    }

    #[test]
    fn render_leads_with_a_bounded_evidence_snapshot_and_next_action() {
        let review = ModelReview {
            summary: "Evidence summary".to_owned(),
            risk: Risk::Low,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let context = json!({"dependency": false, "head": "a".repeat(40), "files": [{"filename": "Cargo.toml", "additions": 2, "deletions": 1}, {"filename": "src/lib.rs", "additions": 0, "deletions": 4}], "checks": [{"name": "test", "state": "success", "url": "https://github.com/owner/repo/actions/runs/1"}, {"name": "unsafe", "state": "failure", "url": "javascript:alert(1)"}]});
        for (event, blockers, heading, action) in [
            (
                "APPROVE",
                vec![],
                "### Ready to merge",
                "use GitHub's Merge control when it is enabled",
            ),
            (
                "COMMENT",
                vec!["`test` is still running (pending)".to_owned()],
                "### Needs attention",
                "Wait for the named checks to finish",
            ),
            (
                "COMMENT",
                vec!["`test` needs attention (failure)".to_owned()],
                "### Needs attention",
                "Resolve the blockers below",
            ),
        ] {
            let body = render(&review, &context, event, &blockers, "<!-- marker -->");
            for expected in [
                heading,
                "Reviewed head: `aaaaaaaaaaaa` · CI snapshot: 1/2 checks successful",
                "What changed",
                "`Cargo.toml` · +2 −1",
                "`src/lib.rs` · +0 −4",
                "Checks",
                "[test](<https://github.com/owner/repo/actions/runs/1>) — success",
                "unsafe — failure",
                "What to do next",
                action,
            ] {
                assert!(body.contains(expected), "missing {expected} in {body}");
            }
            assert!(!body.contains("javascript:"));
        }
    }

    #[test]
    fn repair_handoff_distinguishes_waiting_from_replacement_work() {
        let review = ModelReview {
            summary: "Reviewed. Safe handoff test".to_owned(),
            risk: Risk::Medium,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let context = json!({"number": 42, "url": "https://github.com/owner/repo/pull/42", "head": "a".repeat(40), "update_type": "version-update:semver-patch", "maintainer_changes": "false"});
        for (blockers, waiting) in [
            (
                vec!["`test` hasn't reported for this commit yet".to_owned()],
                true,
            ),
            (
                vec![
                    "The base branch is not protected with strict required checks; merge this manually".to_owned(),
                    "The base branch is not protected with strict required checks; merge this manually".to_owned(),
                ],
                false,
            ),
            (vec!["`test` needs attention (failure)".to_owned()], false),
        ] {
            let mut lines = Vec::new();
            repair_handoff(&mut lines, &review, &context, &blockers);
            let body = lines.join("\n");
            if waiting {
                assert!(body.is_empty());
            } else {
                for expected in [
                    "If code needs to change, open a maintainer replacement PR instead of pushing to the Dependabot branch.",
                    r"- Source: \#42 https://github.com/owner/repo/pull/42",
                    "- Reviewed head: aaaa",
                    "- Update type: version-update:semver-patch",
                    "- Maintainer changes: false",
                    "- Allowed files: approved manifests, lockfiles, Actions workflows and Dockerfiles only",
                    "- Required outcome: every configured and protected check passes on this commit",
                ] {
                    assert!(body.contains(expected), "missing {expected}");
                }
                if blockers
                    .iter()
                    .any(|blocker| blocker.contains("base branch is not protected"))
                {
                    assert_eq!(body.matches("base branch is not protected").count(), 1);
                }
            }
        }
    }

    #[test]
    fn dependency_paths_allow_manifests_and_reject_source() {
        for (paths, expected) in [
            (
                &[
                    "Cargo.toml",
                    "Cargo.lock",
                    "workspace/package.json",
                    "workspace/pnpm-lock.yaml",
                ][..],
                true,
            ),
            (
                &[
                    "api/requirements-dev.txt",
                    "api/constraints.txt",
                    "service/pom.xml",
                    "app/Package.resolved",
                ][..],
                true,
            ),
            (
                &[
                    "src/Directory.Packages.props",
                    "web/build.gradle.kts",
                    ".github/workflows/ci.yml",
                    "service/Dockerfile",
                ][..],
                true,
            ),
            (&[".github/dependabot.yml"][..], false),
            (&["src/lib.rs"][..], false),
            (&[".github/workflows/release.rs"][..], false),
            (&["setup.py"][..], false),
            (&["solution.sln"][..], false),
            (&["../Cargo.toml"][..], false),
        ] {
            let files = paths
                .iter()
                .map(|path| json!({"filename": path}))
                .collect::<Vec<_>>();
            assert_eq!(dependency_file_blockers(&files).is_empty(), expected);
        }
        assert!(
            !dependency_file_blockers(&[
                json!({"filename": "Cargo.toml", "previous_filename": "src/lib.rs"})
            ])
            .is_empty()
        );
    }

    #[test]
    fn dependency_path_gate_does_not_change_regular_reviews() {
        let review = ModelReview {
            summary: "Reviewed".to_owned(),
            risk: Risk::Low,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let protection = json!({"enforce_admins": {"enabled": true}, "required_status_checks": {"checks": [], "contexts": [], "strict": true}});
        let mut context = json!({"complete_diff": true, "checks": [], "dependency": false, "files": [{"filename": "src/lib.rs"}], "update_type": "version-update:semver-patch", "maintainer_changes": "false"});
        assert_eq!(
            decision(&review, &context, &[], Some(&protection))
                .unwrap()
                .0,
            "APPROVE"
        );
        context["dependency"] = json!(true);
        let (event, blockers) = decision(&review, &context, &[], Some(&protection)).unwrap();
        assert_eq!(event, "COMMENT");
        assert!(
            blockers
                .iter()
                .any(|blocker| blocker.contains("outside approved"))
        );
    }

    #[test]
    fn dependency_approval_requires_trusted_update_metadata_and_protection() -> Result<()> {
        let review = ModelReview {
            summary: "Reviewed".to_owned(),
            risk: Risk::Low,
            observations: vec![],
            blockers: vec![],
            minor: vec![],
        };
        let protection = json!({"enforce_admins": {"enabled": true}, "required_status_checks": {"checks": [], "contexts": ["test"], "strict": true}});
        for (update_type, maintainer_changes, protected, event) in [
            ("version-update:semver-patch", "false", true, "APPROVE"),
            ("version-update:semver-minor", "false", true, "APPROVE"),
            ("version-update:semver-major", "false", true, "COMMENT"),
            ("unsupported", "unknown", true, "COMMENT"),
            ("version-update:semver-patch", "unknown", true, "COMMENT"),
            ("version-update:semver-patch", "false", false, "COMMENT"),
        ] {
            let context = json!({
                "complete_diff": true,
                "checks": [{"name": "test", "state": "success", "app_id": null}],
                "dependency": true,
                "files": [{"filename": "Cargo.lock"}],
                "update_type": update_type,
                "maintainer_changes": maintainer_changes,
            });
            assert_eq!(
                decision(
                    &review,
                    &context,
                    &["test".to_owned()],
                    protected.then_some(&protection),
                )?
                .0,
                event
            );
        }
        Ok(())
    }
}
