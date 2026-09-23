use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, anyhow};
use serde_json::{Value, json};

use crate::Result;
use crate::github;

pub(super) const TERMS_VERSION: &str = "2026-09-23";
pub(super) const PRIVACY_VERSION: &str = "2026-09-23";
const CONSENT_ISSUE_TITLE: &str = "Rady service agreement";

pub(super) fn agreement(repo: &str) -> Result<Value> {
    let user = github::api("user", None, "GET", false)?
        .ok_or_else(|| anyhow!("GitHub returned no authenticated user"))?;
    let accepted_by = user["login"]
        .as_str()
        .filter(|login| valid_login(login))
        .ok_or_else(|| anyhow!("GitHub returned an invalid authenticated user"))?;
    let accepted_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    let receipt = create_consent_receipt(repo, accepted_by)?;
    Ok(json!({
        "terms": TERMS_VERSION,
        "privacy": PRIVACY_VERSION,
        "accepted_by": accepted_by,
        "accepted_at_unix": accepted_at,
        "issue": receipt.issue,
        "comment": receipt.comment,
    }))
}

#[derive(Clone, Copy, Debug)]
struct ConsentReceipt {
    issue: u64,
    comment: u64,
}

fn consent_comment(repo: &str) -> String {
    format!(
        "Rady service agreement acceptance\n\nI accept the Rady Terms of Use ({TERMS_VERSION}) and Privacy Policy ({PRIVACY_VERSION}) for {repo}."
    )
}

fn create_consent_receipt(repo: &str, accepted_by: &str) -> Result<ConsentReceipt> {
    let issue = github::api(
        &format!("repos/{repo}/issues"),
        Some(&json!({
            "title": CONSENT_ISSUE_TITLE,
            "body": format!("Accepted by @{accepted_by}. This closed issue is Rady's service-agreement receipt."),
        })),
        "POST",
        false,
    )?
    .ok_or_else(|| anyhow!("GitHub returned no consent issue"))?;
    let number = issue["number"]
        .as_u64()
        .filter(|number| *number > 0)
        .ok_or_else(|| anyhow!("GitHub returned an invalid consent issue"))?;
    let comment = github::api(
        &format!("repos/{repo}/issues/{number}/comments"),
        Some(&json!({"body": consent_comment(repo)})),
        "POST",
        false,
    )?
    .ok_or_else(|| anyhow!("GitHub returned no consent comment"))?;
    let comment = comment["id"]
        .as_u64()
        .filter(|comment| *comment > 0)
        .ok_or_else(|| anyhow!("GitHub returned an invalid consent comment"))?;
    github::api(
        &format!("repos/{repo}/issues/{number}"),
        Some(&json!({"state": "closed"})),
        "PATCH",
        false,
    )?;
    Ok(ConsentReceipt {
        issue: number,
        comment,
    })
}

pub(crate) fn accepted_configuration(value: &Value) -> bool {
    value["schema"].as_u64() == Some(1)
        && value["agreement"]["terms"].as_str() == Some(TERMS_VERSION)
        && value["agreement"]["privacy"].as_str() == Some(PRIVACY_VERSION)
        && value["agreement"]["accepted_by"]
            .as_str()
            .is_some_and(valid_login)
        && value["agreement"]["accepted_at_unix"]
            .as_u64()
            .is_some_and(|time| time > 0)
        && value["agreement"]["issue"]
            .as_u64()
            .is_some_and(|number| number > 0)
        && value["agreement"]["comment"]
            .as_u64()
            .is_some_and(|number| number > 0)
}

pub(crate) fn verified_configuration(github: &github::GitHub, value: &Value) -> Result<bool> {
    if !accepted_configuration(value) {
        return Ok(false);
    }
    let agreement = &value["agreement"];
    let accepted_by = agreement["accepted_by"]
        .as_str()
        .ok_or_else(|| anyhow!("accepted configuration omitted its signer"))?;
    let issue = agreement["issue"]
        .as_u64()
        .ok_or_else(|| anyhow!("accepted configuration omitted its receipt issue"))?;
    let comment = agreement["comment"]
        .as_u64()
        .ok_or_else(|| anyhow!("accepted configuration omitted its receipt comment"))?;
    let issue_value = github.api_optional(&format!("issues/{issue}"), None, "GET")?;
    let comment_value = github.api_optional(&format!("issues/comments/{comment}"), None, "GET")?;
    let permission = github.api_optional(
        &format!("collaborators/{accepted_by}/permission"),
        None,
        "GET",
    )?;
    Ok(receipt_matches(
        github.repo(),
        accepted_by,
        issue,
        comment,
        issue_value.as_ref(),
        comment_value.as_ref(),
        permission.as_ref(),
    ))
}

pub(super) fn verified_existing_configuration(repo: &str, value: &Value) -> Result<bool> {
    if !accepted_configuration(value) {
        return Ok(false);
    }
    let agreement = &value["agreement"];
    let accepted_by = agreement["accepted_by"]
        .as_str()
        .ok_or_else(|| anyhow!("accepted configuration omitted its signer"))?;
    let issue = agreement["issue"]
        .as_u64()
        .ok_or_else(|| anyhow!("accepted configuration omitted its receipt issue"))?;
    let comment = agreement["comment"]
        .as_u64()
        .ok_or_else(|| anyhow!("accepted configuration omitted its receipt comment"))?;
    let issue_value = github::api(&format!("repos/{repo}/issues/{issue}"), None, "GET", true)?;
    let comment_value = github::api(
        &format!("repos/{repo}/issues/comments/{comment}"),
        None,
        "GET",
        true,
    )?;
    let permission = github::api(
        &format!("repos/{repo}/collaborators/{accepted_by}/permission"),
        None,
        "GET",
        true,
    )?;
    Ok(receipt_matches(
        repo,
        accepted_by,
        issue,
        comment,
        issue_value.as_ref(),
        comment_value.as_ref(),
        permission.as_ref(),
    ))
}

fn receipt_matches(
    repo: &str,
    accepted_by: &str,
    issue: u64,
    comment: u64,
    issue_value: Option<&Value>,
    comment_value: Option<&Value>,
    permission: Option<&Value>,
) -> bool {
    let issue_url = format!("https://github.com/{repo}/issues/{issue}");
    let comment_issue_url = format!("https://api.github.com/repos/{repo}/issues/{issue}");
    issue_value.is_some_and(|value| {
        value["number"].as_u64() == Some(issue)
            && value["html_url"].as_str() == Some(&issue_url)
            && value["title"].as_str() == Some(CONSENT_ISSUE_TITLE)
            && value["state"].as_str() == Some("closed")
            && value.get("pull_request").is_none()
    }) && comment_value.is_some_and(|value| {
        value["id"].as_u64() == Some(comment)
            && value["issue_url"].as_str() == Some(&comment_issue_url)
            && value["user"]["login"].as_str() == Some(accepted_by)
            && value["body"].as_str() == Some(&consent_comment(repo))
    }) && permission.is_some_and(|value| value["permission"].as_str() == Some("admin"))
}

fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 100
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_validation_is_table_driven() {
        for (configuration, accepted) in [
            (
                json!({
                    "schema": 1,
                    "agreement": {
                        "terms": TERMS_VERSION,
                        "privacy": PRIVACY_VERSION,
                        "accepted_by": "keys-i",
                        "accepted_at_unix": 1,
                        "issue": 1,
                        "comment": 2,
                    }
                }),
                true,
            ),
            (json!({"schema": 1, "agreement": null}), false),
            (
                json!({
                    "schema": 1,
                    "agreement": {
                        "terms": TERMS_VERSION,
                        "privacy": PRIVACY_VERSION,
                        "accepted_by": "keys-i",
                        "accepted_at_unix": 1,
                    }
                }),
                false,
            ),
            (
                json!({
                    "schema": 1,
                    "agreement": {
                        "terms": "old",
                        "privacy": PRIVACY_VERSION,
                        "accepted_by": "keys-i",
                        "accepted_at_unix": 1,
                        "issue": 1,
                        "comment": 2,
                    }
                }),
                false,
            ),
        ] {
            assert_eq!(accepted_configuration(&configuration), accepted);
        }
    }

    #[test]
    fn consent_receipt_requires_exact_authenticated_evidence() {
        let repo = "keys-i/rady";
        let issue = json!({
            "number": 7,
            "html_url": "https://github.com/keys-i/rady/issues/7",
            "title": CONSENT_ISSUE_TITLE,
            "state": "closed",
        });
        let comment = json!({
            "id": 9,
            "issue_url": "https://api.github.com/repos/keys-i/rady/issues/7",
            "user": {"login": "keys-i"},
            "body": consent_comment(repo),
        });
        let permission = json!({"permission": "admin"});
        for (issue_value, comment_value, permission_value, valid) in [
            (issue.clone(), comment.clone(), permission.clone(), true),
            (
                json!({"state": "open"}),
                comment.clone(),
                permission.clone(),
                false,
            ),
            (
                issue.clone(),
                json!({"user": {"login": "other"}}),
                permission.clone(),
                false,
            ),
            (issue, comment, json!({"permission": "write"}), false),
        ] {
            assert_eq!(
                receipt_matches(
                    repo,
                    "keys-i",
                    7,
                    9,
                    Some(&issue_value),
                    Some(&comment_value),
                    Some(&permission_value),
                ),
                valid
            );
        }
    }
}
