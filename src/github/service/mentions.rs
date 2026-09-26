use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::Value;
use tempfile::TempDir;

use crate::Result;
use crate::agent;
use crate::delivery::quality::AcceptanceCheck;
use crate::delivery::{self, Config, DeliveryAuth};
use crate::github::{self, GitHub};
use crate::setup;
use crate::ui::{OutputMode, Theme};

use super::tokens::ServiceTokenProvider;
use super::{ServeArgs, owner_matches, record_sweep_failure, validate_owner_filter};

const MAX_MENTION_CURSORS: usize = 512;
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DELIVERY_CHECK: &str = "git diff --check";
const CLAIM_LEASE: Duration = Duration::from_secs(2 * 60 * 60);
// ponytail: scan only the newest 400 comments; use indexed durable claims if busy repositories need more
const MAX_CLAIM_SCAN_PAGES: u64 = 4;
const COMMENTS_PER_PAGE: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
struct ClaimRecord {
    issue: u64,
    approval: u64,
    comment: u64,
    issued_at: u64,
}

#[derive(Default)]
struct ClaimScan {
    claims: BTreeMap<u64, ClaimRecord>,
    results: BTreeSet<u64>,
}

pub(super) fn sweep_with_token(
    arguments: &ServeArgs,
    token: &str,
    tokens: &ServiceTokenProvider,
    cursors: &mut BTreeMap<String, u64>,
) -> Result<()> {
    validate_owner_filter(arguments.owner.as_deref())?;
    let repositories =
        github::authenticated_pages("installation/repositories", "repositories", token)?;
    let model = env::var("KOELU_MODEL")
        .ok()
        .filter(|value| !value.is_empty());
    let mut failures = Vec::new();
    bound_cursors(cursors);
    let mut repositories = repositories.iter().collect::<Vec<_>>();
    repositories.sort_by_key(|repository| repository["full_name"].as_str().unwrap_or_default());
    for repository in repositories {
        if repository["archived"].as_bool() == Some(true)
            || repository["disabled"].as_bool() == Some(true)
        {
            continue;
        }
        let Some(name) = repository["full_name"].as_str().filter(|name| {
            owner_matches(repository, arguments.owner.as_deref())
                && github::validate_repository(name).is_ok()
        }) else {
            continue;
        };
        let private = repository["private"].as_bool();
        let github = GitHub::new(name, token)?;
        let accepted = match github
            .raw_optional("contents/.github/koelu.json")?
            .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        {
            Some(configuration) => match setup::verified_configuration(&github, &configuration) {
                Ok(accepted) => accepted,
                Err(error) => {
                    record_sweep_failure(&mut failures, name, &error);
                    continue;
                }
            },
            None => false,
        };
        if !accepted {
            continue;
        }
        if let Err(error) = terminalize_stale_claims(&github) {
            record_sweep_failure(&mut failures, name, &error);
        }
        let comments = match github.pages_after_id(
            "issues/comments?sort=created&direction=desc",
            cursors.get(name).copied(),
        ) {
            Ok(value) => value,
            Err(error) => {
                record_sweep_failure(&mut failures, name, &error);
                continue;
            }
        };
        let mut repo_failed = false;
        for comment in &comments {
            let Some(body) = comment["body"].as_str().filter(|body| {
                matches!(
                    comment["author_association"].as_str(),
                    Some("OWNER" | "MEMBER" | "COLLABORATOR")
                ) && crate::mentions::is_invocation(body)
            }) else {
                continue;
            };
            let (Some(comment_id), Some(issue)) = (
                comment["id"].as_u64(),
                comment_issue(name, comment["issue_url"].as_str().unwrap_or_default()),
            ) else {
                record_sweep_failure(
                    &mut failures,
                    name,
                    &anyhow!("GitHub returned an invalid mention comment"),
                );
                repo_failed = true;
                continue;
            };
            let _ = body;
            if let Err(error) = crate::mentions::respond_for_repository(
                &github,
                issue,
                comment_id,
                model.as_deref(),
                arguments.harness,
                private,
            ) {
                if crate::mentions::is_hosted_unavailable(&error) {
                    eprintln!(
                        "{name}: model providers are cooling down; Koelu will retry these mentions next pass"
                    );
                    repo_failed = true;
                    break;
                }
                record_sweep_failure(&mut failures, name, &error);
                repo_failed = true;
                continue;
            }
            if let Err(error) =
                dispatch_approved_write(arguments, tokens, &github, issue, comment_id)
            {
                record_sweep_failure(&mut failures, name, &error);
                repo_failed = true;
            }
        }
        if !repo_failed {
            if let Some(last) = comments
                .iter()
                .filter_map(|comment| comment["id"].as_u64())
                .max()
            {
                remember_cursor(cursors, name, last);
            }
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    let noun = if failures.len() == 1 {
        "problem"
    } else {
        "problems"
    };
    bail!(
        "the mention pass hit {} {noun}: {}",
        failures.len(),
        failures.join("; ")
    )
}

fn dispatch_approved_write(
    arguments: &ServeArgs,
    tokens: &ServiceTokenProvider,
    github: &GitHub,
    issue: u64,
    approval_comment: u64,
) -> Result<()> {
    let Some(approved) = crate::mentions::approved_write(github, issue, approval_comment)? else {
        return Ok(());
    };
    let claim_marker = crate::mentions::claim_marker(approval_comment, unix_now()?);
    let claim = github.api(
        &format!("issues/{issue}/comments"),
        Some(&serde_json::json!({"body": format!(
            "{}\n\nPreparing the approved change.",
            claim_marker,
        )})),
        "POST",
    )?;
    let claim_id = match claim["id"].as_u64().filter(|id| *id > 0) {
        Some(id) => id,
        None => {
            let error = anyhow!("GitHub returned an invalid dispatch claim");
            post_dispatch_result(github, issue, approval_comment, Err(&error))?;
            return Err(error);
        }
    };
    let owns = match owns_claim(github, issue, approval_comment, claim_id) {
        Ok(owns) => owns,
        Err(error) => {
            post_dispatch_result(github, issue, approval_comment, Err(&error))?;
            return Err(error);
        }
    };
    if !owns {
        return Ok(());
    }

    let result = (|| {
        let token = tokens.delivery_token(github.repo())?;
        let checkout = checkout_repository(github.repo(), &token)?;
        let config = hosted_config(
            arguments,
            github.repo(),
            &checkout.path().join("repository"),
            &approved,
        );
        let auth =
            DeliveryAuth::approved_installation(github.repo(), token, issue, &approved, claim_id)?;
        let state = delivery::deliver_hosted(config, auth)?;
        state["url"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("hosted delivery finished without a pull request URL"))
    })();
    post_dispatch_result(github, issue, approval_comment, result.as_ref())?;
    result.map(|_| ())
}

fn post_dispatch_result(
    github: &GitHub,
    issue: u64,
    approval_comment: u64,
    result: std::result::Result<&String, &anyhow::Error>,
) -> Result<()> {
    let body = match result {
        Ok(url) => format!(
            "{}\n\nOpened pull request: {url}",
            crate::mentions::result_marker(approval_comment)
        ),
        Err(error) => format!(
            "{}\n\nCouldn't create the approved change: {}\n\nCreate a new request and approval to retry.",
            crate::mentions::result_marker(approval_comment),
            terminal_error(error),
        ),
    };
    github.api(
        &format!("issues/{issue}/comments"),
        Some(&serde_json::json!({"body": body})),
        "POST",
    )?;
    Ok(())
}

fn owns_claim(github: &GitHub, issue: u64, approval_comment: u64, claim: u64) -> Result<bool> {
    let bot = format!("{}[bot]", app_slug());
    let comments = github.pages(
        &format!("issues/{issue}/comments?sort=created&direction=asc"),
        None,
    )?;
    Ok(unfinished_claim(&comments, &bot, approval_comment, claim))
}

fn unfinished_claim(comments: &[Value], bot: &str, approval_comment: u64, claim: u64) -> bool {
    first_claim(comments, bot, approval_comment) == Some(claim)
        && !comments.iter().any(|comment| {
            comment["user"]["login"]
                .as_str()
                .is_some_and(|login| login.eq_ignore_ascii_case(bot))
                && comment["body"].as_str().is_some_and(|body| {
                    crate::mentions::result_from_body(body) == Some(approval_comment)
                })
        })
}

fn first_claim(comments: &[Value], bot: &str, approval_comment: u64) -> Option<u64> {
    comments
        .iter()
        .filter(|comment| {
            comment["user"]["login"]
                .as_str()
                .is_some_and(|login| login.eq_ignore_ascii_case(bot))
                && comment["body"].as_str().is_some_and(|body| {
                    crate::mentions::claim_from_body(body)
                        .is_some_and(|claim| claim.approval_comment == approval_comment)
                })
        })
        .filter_map(|comment| comment["id"].as_u64())
        .min()
}

fn terminalize_stale_claims(github: &GitHub) -> Result<()> {
    let bot = format!("{}[bot]", app_slug());
    let mut scan = ClaimScan::default();
    for page in 1..=MAX_CLAIM_SCAN_PAGES {
        let comments = github.page("issues/comments?sort=created&direction=desc", page)?;
        for comment in &comments {
            scan_claim_record(&mut scan, github.repo(), &bot, comment)?;
        }
        if comments.len() < COMMENTS_PER_PAGE {
            break;
        }
    }
    for claim in stale_claims(&scan, unix_now()?) {
        if owns_claim(github, claim.issue, claim.approval, claim.comment)? {
            let error = anyhow!("the dispatcher lease expired before it could finish");
            post_dispatch_result(github, claim.issue, claim.approval, Err(&error))?;
        }
    }
    Ok(())
}

fn scan_claim_record(
    scan: &mut ClaimScan,
    repository: &str,
    bot: &str,
    comment: &Value,
) -> Result<()> {
    if !comment["user"]["login"]
        .as_str()
        .is_some_and(|login| login.eq_ignore_ascii_case(bot))
    {
        return Ok(());
    }
    let body = comment["body"].as_str().unwrap_or_default();
    if let Some(approval) = crate::mentions::result_from_body(body) {
        scan.results.insert(approval);
    }
    let Some(claim) = crate::mentions::claim_from_body(body) else {
        return Ok(());
    };
    let issue = comment_issue(
        repository,
        comment["issue_url"].as_str().unwrap_or_default(),
    )
    .ok_or_else(|| anyhow!("GitHub returned an invalid hosted write claim"))?;
    let comment_id = comment["id"]
        .as_u64()
        .filter(|id| *id > 0)
        .ok_or_else(|| anyhow!("GitHub returned an invalid hosted write claim"))?;
    let record = ClaimRecord {
        issue,
        approval: claim.approval_comment,
        comment: comment_id,
        issued_at: claim.issued_at,
    };
    scan.claims
        .entry(record.approval)
        .and_modify(|first| {
            if record.comment < first.comment {
                *first = record.clone();
            }
        })
        .or_insert(record);
    Ok(())
}

fn stale_claims(scan: &ClaimScan, now: u64) -> Vec<&ClaimRecord> {
    scan.claims
        .values()
        .filter(|claim| {
            !scan.results.contains(&claim.approval)
                && now.saturating_sub(claim.issued_at) >= CLAIM_LEASE.as_secs()
        })
        .collect()
}

fn unix_now() -> Result<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| anyhow!("system clock is before the Unix epoch"))
        .map(|duration| duration.as_secs())
}

fn checkout_repository(repository: &str, token: &str) -> Result<TempDir> {
    let root = tempfile::tempdir()?;
    let checkout = root.path().join("repository");
    let git = agent::which("git").ok_or_else(|| anyhow!("install Git for hosted delivery"))?;
    let credentials = STANDARD.encode(format!("x-access-token:{token}"));
    let environment = BTreeMap::from([
        ("GIT_TERMINAL_PROMPT".to_owned(), "0".to_owned()),
        ("GIT_CONFIG_COUNT".to_owned(), "1".to_owned()),
        (
            "GIT_CONFIG_KEY_0".to_owned(),
            "http.https://github.com/.extraheader".to_owned(),
        ),
        (
            "GIT_CONFIG_VALUE_0".to_owned(),
            format!("AUTHORIZATION: Basic {credentials}"),
        ),
    ]);
    let remote = format!("https://github.com/{repository}.git");
    let output = agent::execute(
        git.as_os_str(),
        [
            "clone",
            "--no-tags",
            remote.as_str(),
            checkout.to_string_lossy().as_ref(),
        ],
        root.path(),
        b"",
        Duration::from_secs(120),
        &environment,
        false,
        None,
    )?;
    if output.code != 0 {
        bail!("couldn't create a hosted delivery workspace")
    }
    Ok(root)
}

fn hosted_config(
    arguments: &ServeArgs,
    repository: &str,
    directory: &Path,
    approved: &crate::mentions::ApprovedWrite,
) -> Config {
    Config {
        task: approved.task.clone(),
        directory: PathBuf::from(directory),
        repo: Some(repository.to_owned()),
        checks: vec![DELIVERY_CHECK.to_owned()],
        harness: arguments.harness,
        agents: 1,
        model: env::var("KOELU_MODEL")
            .ok()
            .filter(|value| !value.is_empty()),
        model_choices: Vec::new(),
        review_model: None,
        plan: None,
        acceptance_checks: vec![AcceptanceCheck {
            criterion: 0,
            command: DELIVERY_CHECK.to_owned(),
            expected_exit: 0,
            expected_output: Some(String::new()),
            files: Vec::new(),
        }],
        max_tokens: None,
        orchestrator_model: None,
        orchestrator_harness: None,
        base: Some(approved.base.clone()),
        attempts: 2,
        timeout: DELIVERY_TIMEOUT,
        benchmarks: Vec::new(),
        benchmark_runs: 1,
        benchmark_warmups: 0,
        benchmark_metric: None,
        max_benchmark_noise: 10.0,
        max_regression: 5.0,
        max_files: 20,
        max_lines: 1_000,
        theme: Theme::Plain,
        output: OutputMode::Json,
        seed_patch: None,
        resumed_from: None,
        expected_start: Some(approved.base_sha.clone()),
        mcp_servers: Vec::new(),
        ghost: false,
    }
}

fn app_slug() -> String {
    env::var("KOELU_APP_SLUG")
        .ok()
        .filter(|slug| !slug.is_empty())
        .unwrap_or_else(|| "koelu".to_owned())
}

fn terminal_error(error: &anyhow::Error) -> String {
    error
        .to_string()
        .chars()
        .filter(|character| !character.is_control() || *character == ' ')
        .take(1_000)
        .collect()
}

fn remember_cursor(cursors: &mut BTreeMap<String, u64>, repository: &str, comment: u64) {
    cursors.insert(repository.to_owned(), comment);
    bound_cursors(cursors);
}

fn bound_cursors(cursors: &mut BTreeMap<String, u64>) {
    if cursors.len() <= MAX_MENTION_CURSORS {
        return;
    }
    let mut newest = cursors
        .iter()
        .map(|(repository, comment)| (repository.clone(), *comment))
        .collect::<Vec<_>>();
    newest.sort_unstable_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    newest.truncate(MAX_MENTION_CURSORS);
    let keep = newest
        .into_iter()
        .map(|(repository, _)| repository)
        .collect::<BTreeSet<_>>();
    cursors.retain(|repository, _| keep.contains(repository));
}

fn comment_issue(repo: &str, issue_url: &str) -> Option<u64> {
    let prefix = format!("https://api.github.com/repos/{repo}/issues/");
    issue_url
        .strip_prefix(&prefix)?
        .parse::<u64>()
        .ok()
        .filter(|number| *number > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn central_sweep_accepts_only_canonical_issue_urls() {
        for (repo, url, expected) in [
            (
                "keys-i/koelu",
                "https://api.github.com/repos/keys-i/koelu/issues/42",
                Some(42),
            ),
            (
                "keys-i/koelu",
                "https://api.github.com/repos/other/koelu/issues/42",
                None,
            ),
            (
                "keys-i/koelu",
                "https://api.github.com/repos/keys-i/koelu/issues/0",
                None,
            ),
            ("keys-i/koelu", "not-a-url", None),
        ] {
            assert_eq!(comment_issue(repo, url), expected, "{url}");
        }
    }

    #[test]
    fn dispatch_claims_choose_the_first_bot_marker_and_terminal_errors_are_bounded() {
        let marker = crate::mentions::claim_marker(9, 1);
        let mut comments = vec![
            serde_json::json!({"id": 8, "user": {"login": "koelu[bot]"}, "body": marker}),
            serde_json::json!({"id": 4, "user": {"login": "someone"}, "body": marker}),
            serde_json::json!({"id": 3, "user": {"login": "Koelu[bot]"}, "body": marker}),
        ];
        assert_eq!(first_claim(&comments, "koelu[bot]", 9), Some(3));
        assert!(unfinished_claim(&comments, "koelu[bot]", 9, 3));
        assert!(!unfinished_claim(&comments, "koelu[bot]", 9, 8));
        comments.push(serde_json::json!({
            "id": 10,
            "user": {"login": "koelu[bot]"},
            "body": crate::mentions::result_marker(9),
        }));
        assert!(!unfinished_claim(&comments, "koelu[bot]", 9, 3));
        assert_eq!(terminal_error(&anyhow!("failed\nnow")), "failednow");
        assert_eq!(
            terminal_error(&anyhow!("{}", "x".repeat(1_001)))
                .chars()
                .count(),
            1_000
        );
    }

    #[test]
    fn stale_claim_selection_is_leased_and_winner_bound() -> Result<()> {
        let now = 10_000;
        for (case, claims, result, expected) in [
            (
                "active",
                vec![(3, "koelu[bot]", now - CLAIM_LEASE.as_secs() + 1)],
                false,
                None,
            ),
            (
                "stale",
                vec![(3, "koelu[bot]", now - CLAIM_LEASE.as_secs())],
                false,
                Some(3),
            ),
            (
                "completed",
                vec![(3, "koelu[bot]", now - CLAIM_LEASE.as_secs())],
                true,
                None,
            ),
            (
                "wrong bot",
                vec![(3, "other[bot]", now - CLAIM_LEASE.as_secs())],
                false,
                None,
            ),
            (
                "duplicate winner",
                vec![
                    (8, "koelu[bot]", now - CLAIM_LEASE.as_secs()),
                    (3, "koelu[bot]", now - CLAIM_LEASE.as_secs() + 1),
                ],
                false,
                None,
            ),
            (
                "claim then abort recovery",
                vec![(3, "koelu[bot]", now - CLAIM_LEASE.as_secs())],
                false,
                Some(3),
            ),
        ] {
            let mut scan = ClaimScan::default();
            for (id, bot, issued_at) in claims {
                scan_claim_record(
                    &mut scan,
                    "owner/repository",
                    "koelu[bot]",
                    &serde_json::json!({
                        "id": id,
                        "user": {"login": bot},
                        "issue_url": "https://api.github.com/repos/owner/repository/issues/7",
                        "body": crate::mentions::claim_marker(9, issued_at),
                    }),
                )?;
            }
            if result {
                scan_claim_record(
                    &mut scan,
                    "owner/repository",
                    "koelu[bot]",
                    &serde_json::json!({
                        "id": 10,
                        "user": {"login": "koelu[bot]"},
                        "issue_url": "https://api.github.com/repos/owner/repository/issues/7",
                        "body": crate::mentions::result_marker(9),
                    }),
                )?;
            }
            assert_eq!(
                stale_claims(&scan, now)
                    .into_iter()
                    .map(|claim| claim.comment)
                    .next(),
                expected,
                "{case}"
            );
        }
        Ok(())
    }

    #[test]
    fn hosted_writes_use_progressive_checkpoints() {
        let arguments = ServeArgs {
            owner: None,
            harness: crate::agent::Harness::Command,
            interval: 30,
            max_reviews: 4,
            app_client_id: None,
            app_private_key_file: None,
            once: true,
        };
        let approved = crate::mentions::ApprovedWrite {
            task: "fix the parser".to_owned(),
            base: "main".to_owned(),
            base_sha: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            approval_comment: 2,
            request_comment: 1,
            actor: "maintainer".to_owned(),
        };

        let config = hosted_config(
            &arguments,
            "owner/repository",
            Path::new("repository"),
            &approved,
        );

        assert!(!config.ghost);
        assert_eq!(
            config.expected_start.as_deref(),
            Some(approved.base_sha.as_str())
        );
    }

    #[test]
    fn cursor_memory_survives_another_installation_and_has_a_recent_window_fallback() {
        let mut cursors = BTreeMap::from([
            ("first/repository".to_owned(), 3_100),
            ("second/repository".to_owned(), 3_099),
        ]);
        bound_cursors(&mut cursors);
        assert_eq!(cursors.get("first/repository"), Some(&3_100));

        for comment in 1..=(MAX_MENTION_CURSORS as u64 + 1) {
            remember_cursor(
                &mut cursors,
                &format!("owner/repository-{comment}"),
                comment,
            );
        }

        assert_eq!(cursors.len(), MAX_MENTION_CURSORS);
        assert_eq!(cursors.get("first/repository"), Some(&3_100));
        assert!(!cursors.contains_key("owner/repository-1"));
    }
}
