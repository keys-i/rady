use std::fmt::Write as _;

use anyhow::{anyhow, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::github::GitHub;

use super::{
    COMMENTS_PER_PAGE, Invocation, app_slug, invocation, issue_comment_count, issue_is_open,
    trusted_prompt, valid_login,
};

const MAX_APPROVAL_COMMENT_PAGES: u64 = 31;
const WRITE_PROPOSAL_MARKER: &str = "<!-- pekin:write-proposal:";
const WRITE_CLAIM_MARKER: &str = "<!-- pekin:write-claim:";
const WRITE_RESULT_MARKER: &str = "<!-- pekin:write-result:";

/// A write request whose approval and source still match GitHub's live state
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ApprovedWrite {
    pub task: String,
    pub base: String,
    pub base_sha: String,
    pub approval_comment: u64,
    pub request_comment: u64,
    pub actor: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct WriteProposal {
    // Request identity is read by the mention deduplicator
    pub(super) request: u64,
    actor: String,
    digest: String,
    base: String,
    sha: String,
}

/// A durable hosted-write claim bound to its approval and issue time
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WriteClaim {
    pub approval_comment: u64,
    pub issued_at: u64,
}

#[derive(Default)]
struct ApprovalEvidence {
    proposal: bool,
    dispatched: bool,
    first_claim: Option<u64>,
    result: bool,
}

/// Revalidate an approval before a dispatcher can create a branch or pull request
pub(crate) fn approved_write(
    github: &GitHub,
    issue: u64,
    approval_comment: u64,
) -> Result<Option<ApprovedWrite>> {
    approved_write_for_claim(github, issue, approval_comment, None)
}

/// Revalidate a claimed request immediately before hosted publication
pub(crate) fn approved_write_with_claim(
    github: &GitHub,
    issue: u64,
    approval_comment: u64,
    claim_comment: u64,
) -> Result<Option<ApprovedWrite>> {
    approved_write_for_claim(github, issue, approval_comment, Some(claim_comment))
}

fn approved_write_for_claim(
    github: &GitHub,
    issue: u64,
    approval_comment: u64,
    claim_comment: Option<u64>,
) -> Result<Option<ApprovedWrite>> {
    let approval = github.api(&format!("issues/comments/{approval_comment}"), None, "GET")?;
    let Some(approval) = trusted_prompt(&approval, issue, approval_comment)? else {
        return Ok(None);
    };
    let Invocation::Approve(request_comment) = invocation(&approval.prompt) else {
        return Ok(None);
    };
    let origin = github.api(&format!("issues/comments/{request_comment}"), None, "GET")?;
    let Some(origin) = trusted_prompt(&origin, issue, request_comment)? else {
        return Ok(None);
    };
    let Invocation::WriteRequest(task) = invocation(&origin.prompt) else {
        return Ok(None);
    };
    if !approval.actor.eq_ignore_ascii_case(&origin.actor) {
        return Ok(None);
    }
    let issue_value = github.api(&format!("issues/{issue}"), None, "GET")?;
    let issue_open = issue_is_open(&issue_value, issue)?;
    if !issue_open {
        return Ok(None);
    }
    let permission = github.api(
        &format!("collaborators/{}/permission", approval.actor),
        None,
        "GET",
    )?;
    if !live_write_permission(&permission) {
        return Ok(None);
    }
    let expected = write_proposal(github, request_comment, &origin.actor, &task)?;
    let bot = format!("{}[bot]", app_slug());
    let evidence = approval_evidence(
        github,
        issue,
        issue_comment_count(&issue_value)?,
        approval_comment,
        &expected,
        &bot,
    )?;
    let allowed = match claim_comment {
        Some(claim) => claimed_approval_boundary_allows(
            &approval.actor,
            &origin.actor,
            issue_open,
            &evidence,
            &permission,
            claim,
        ),
        None => approval_boundary_allows(
            &approval.actor,
            &origin.actor,
            issue_open,
            &evidence,
            &permission,
        ),
    };
    if !allowed {
        return Ok(None);
    }
    Ok(Some(ApprovedWrite {
        task,
        base: expected.base,
        base_sha: expected.sha,
        approval_comment,
        request_comment,
        actor: origin.actor,
    }))
}

fn approval_boundary_allows(
    approval_actor: &str,
    origin_actor: &str,
    issue_open: bool,
    evidence: &ApprovalEvidence,
    permission: &Value,
) -> bool {
    approval_actor.eq_ignore_ascii_case(origin_actor)
        && issue_open
        && !evidence.dispatched
        && live_write_permission(permission)
        && evidence.proposal
}

fn claimed_approval_boundary_allows(
    approval_actor: &str,
    origin_actor: &str,
    issue_open: bool,
    evidence: &ApprovalEvidence,
    permission: &Value,
    claim_comment: u64,
) -> bool {
    approval_actor.eq_ignore_ascii_case(origin_actor)
        && issue_open
        && live_write_permission(permission)
        && evidence.proposal
        && !evidence.result
        && evidence.first_claim == Some(claim_comment)
}

fn approval_evidence(
    github: &GitHub,
    issue: u64,
    count: u64,
    approval_comment: u64,
    expected: &WriteProposal,
    bot: &str,
) -> Result<ApprovalEvidence> {
    let pages = approval_comment_pages(count)?;
    let mut evidence = ApprovalEvidence::default();
    for page in 1..=pages {
        let comments = github.page(
            &format!("issues/{issue}/comments?sort=created&direction=asc"),
            page,
        )?;
        extend_approval_evidence(&mut evidence, &comments, approval_comment, expected, bot);
    }
    Ok(evidence)
}

fn approval_comment_pages(count: u64) -> Result<u64> {
    let pages = count.div_ceil(COMMENTS_PER_PAGE);
    if pages > MAX_APPROVAL_COMMENT_PAGES {
        bail!(
            "GitHub issue has more than {} comments; approve manually",
            MAX_APPROVAL_COMMENT_PAGES * COMMENTS_PER_PAGE
        );
    }
    Ok(pages)
}

fn extend_approval_evidence(
    evidence: &mut ApprovalEvidence,
    comments: &[Value],
    approval_comment: u64,
    expected: &WriteProposal,
    bot: &str,
) {
    for comment in comments {
        let login = comment["user"]["login"].as_str().unwrap_or_default();
        let body = comment["body"].as_str().unwrap_or_default();
        if !login.eq_ignore_ascii_case(bot) {
            continue;
        }
        evidence.proposal |= proposal_from_body(body).is_some_and(|proposal| proposal == *expected);
        if claim_from_body(body).is_some_and(|claim| claim.approval_comment == approval_comment) {
            evidence.dispatched = true;
            if let Some(id) = comment["id"].as_u64() {
                evidence.first_claim = Some(evidence.first_claim.map_or(id, |first| first.min(id)));
            }
        }
        if result_from_body(body) == Some(approval_comment) {
            evidence.dispatched = true;
            evidence.result = true;
        }
    }
}

/// Marker written before dispatching so a later sweep cannot create a duplicate pull request
pub(crate) fn claim_marker(approval_comment: u64, issued_at: u64) -> String {
    format!("{WRITE_CLAIM_MARKER}{approval_comment}:{issued_at} -->")
}

pub(crate) fn claim_from_body(body: &str) -> Option<WriteClaim> {
    let value = body.split_once(WRITE_CLAIM_MARKER)?.1.split_once(" -->")?.0;
    let (approval_comment, issued_at) = value.split_once(':')?;
    Some(WriteClaim {
        approval_comment: approval_comment.parse().ok().filter(|value| *value > 0)?,
        issued_at: issued_at.parse().ok()?,
    })
}

pub(crate) fn result_from_body(body: &str) -> Option<u64> {
    body.split_once(WRITE_RESULT_MARKER)?
        .1
        .split_once(" -->")?
        .0
        .parse()
        .ok()
        .filter(|value| *value > 0)
}

/// Marker written after dispatching with the pull request URL as plain body text
pub(crate) fn result_marker(approval_comment: u64) -> String {
    format!("{WRITE_RESULT_MARKER}{approval_comment} -->")
}

pub(super) fn write_proposal(
    github: &GitHub,
    request: u64,
    actor: &str,
    task: &str,
) -> Result<WriteProposal> {
    let repository = github.api("", None, "GET")?;
    if repository["full_name"]
        .as_str()
        .is_none_or(|name| !name.eq_ignore_ascii_case(github.repo()))
    {
        bail!("GitHub returned a different repository while preparing the write request");
    }
    let base = repository["default_branch"]
        .as_str()
        .filter(|branch| valid_branch(branch))
        .ok_or_else(|| anyhow!("GitHub returned no valid default branch"))?;
    let branch = github.api(&format!("branches/{}", percent_encode(base)), None, "GET")?;
    if branch["name"].as_str() != Some(base) {
        bail!("GitHub returned a different default branch while preparing the write request");
    }
    let sha = branch["commit"]["sha"]
        .as_str()
        .filter(|sha| valid_sha(sha))
        .ok_or_else(|| anyhow!("GitHub returned an invalid default branch commit"))?;
    Ok(WriteProposal {
        request,
        actor: actor.to_owned(),
        digest: prompt_digest(task),
        base: base.to_owned(),
        sha: sha.to_owned(),
    })
}

pub(super) fn proposal_marker(proposal: &WriteProposal) -> String {
    let payload = json!({
        "request": proposal.request,
        "actor": proposal.actor,
        "digest": proposal.digest,
        "base": proposal.base,
        "sha": proposal.sha,
    });
    format!("{WRITE_PROPOSAL_MARKER}{payload} -->")
}

pub(super) fn proposal_from_body(body: &str) -> Option<WriteProposal> {
    let payload = body
        .split(WRITE_PROPOSAL_MARKER)
        .nth(1)?
        .split_once(" -->")?
        .0;
    let value = serde_json::from_str::<Value>(payload).ok()?;
    let proposal = WriteProposal {
        request: value["request"].as_u64().filter(|id| *id > 0)?,
        actor: value["actor"]
            .as_str()
            .filter(|actor| valid_login(actor))?
            .to_owned(),
        digest: value["digest"]
            .as_str()
            .filter(|digest| valid_digest(digest))?
            .to_owned(),
        base: value["base"]
            .as_str()
            .filter(|base| valid_branch(base))?
            .to_owned(),
        sha: value["sha"]
            .as_str()
            .filter(|sha| valid_sha(sha))?
            .to_owned(),
    };
    Some(proposal)
}

pub(super) fn parse_approval(prompt: &str) -> Option<u64> {
    let (verb, id) = prompt.split_once(' ')?;
    if !verb.eq_ignore_ascii_case("approve") || id.is_empty() {
        return None;
    }
    id.parse::<u64>().ok().filter(|id| *id > 0)
}

fn live_write_permission(permission: &Value) -> bool {
    matches!(
        permission["permission"].as_str(),
        Some("write" | "maintain" | "admin")
    )
}

fn prompt_digest(prompt: &str) -> String {
    format!("{:x}", Sha256::digest(prompt.as_bytes()))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_branch(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains("-->")
        && !value.chars().any(char::is_control)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(char::from(byte));
            }
            _ => write!(&mut encoded, "%{byte:02X}").expect("writing to a string cannot fail"),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::super::{LEGACY_BOT_LOGIN, legacy_reply_marker, prior_reply_exists, reply_marker};
    use super::*;

    #[test]
    fn approval_boundary_rejects_changed_or_incomplete_evidence() {
        for (case, allowed) in [
            ("valid", true),
            ("different actor", false),
            ("closed issue", false),
            ("revoked permission", false),
            ("stale base", false),
            ("missing proposal", false),
            ("already claimed", false),
        ] {
            let mut approval_actor = "keys-i";
            let origin_actor = "keys-i";
            let mut issue_open = true;
            let mut permission = json!({"permission": "write"});
            let mut evidence = ApprovalEvidence {
                proposal: true,
                dispatched: false,
                ..ApprovalEvidence::default()
            };
            match case {
                "different actor" => approval_actor = "other",
                "closed issue" => issue_open = false,
                "revoked permission" => permission = json!({"permission": "read"}),
                "stale base" => evidence.proposal = false,
                "missing proposal" => evidence.proposal = false,
                "already claimed" => evidence.dispatched = true,
                "valid" => {}
                _ => unreachable!(),
            }
            assert_eq!(
                approval_boundary_allows(
                    approval_actor,
                    origin_actor,
                    issue_open,
                    &evidence,
                    &permission,
                ),
                allowed,
                "{case}"
            );
        }
    }

    #[test]
    fn claimed_approval_boundary_accepts_only_the_winning_unfinished_claim() {
        for (case, first_claim, result, expected) in [
            ("unclaimed", None, false, false),
            ("correct claim", Some(9), false, true),
            ("wrong claim", Some(8), false, false),
            ("result", Some(9), true, false),
        ] {
            let evidence = ApprovalEvidence {
                proposal: true,
                dispatched: first_claim.is_some() || result,
                first_claim,
                result,
            };
            assert_eq!(
                claimed_approval_boundary_allows(
                    "keys-i",
                    "keys-i",
                    true,
                    &evidence,
                    &json!({"permission": "maintain"}),
                    9,
                ),
                expected,
                "{case}"
            );
        }
    }

    #[test]
    fn approval_scan_finds_old_proposals_and_stops_claimed_dispatches() -> Result<()> {
        let proposal = WriteProposal {
            request: 42,
            actor: "keys-i".to_owned(),
            digest: prompt_digest("fix the failing test"),
            base: "main".to_owned(),
            sha: "a".repeat(40),
        };
        let mut page_one =
            vec![json!({"user": {"login": "pekin[bot]"}, "body": proposal_marker(&proposal)})];
        page_one.extend(
            (1..100).map(|id| json!({"user": {"login": format!("user-{id}")}, "body": "noise"})),
        );
        let page_two = (100..=101)
            .map(|id| json!({"user": {"login": format!("user-{id}")}, "body": "noise"}))
            .collect::<Vec<_>>();
        let mut evidence = ApprovalEvidence::default();
        for page in [&page_one, &page_two] {
            extend_approval_evidence(&mut evidence, page, 9, &proposal, "pekin[bot]");
        }
        assert!(evidence.proposal);
        assert!(!evidence.dispatched);

        let claimed = vec![json!({
            "user": {"login": "pekin[bot]"},
            "body": claim_marker(9, 1),
        })];
        extend_approval_evidence(&mut evidence, &claimed, 9, &proposal, "pekin[bot]");
        assert!(evidence.dispatched);

        let mut missing = ApprovalEvidence::default();
        let mut stale = proposal.clone();
        stale.sha = "b".repeat(40);
        extend_approval_evidence(&mut missing, &page_one, 9, &stale, "pekin[bot]");
        assert!(!missing.proposal);
        assert_eq!(approval_comment_pages(3_100)?, 31);
        assert!(approval_comment_pages(3_101).is_err());
        Ok(())
    }

    #[test]
    fn write_markers_are_bound_and_idempotent() {
        let proposal = WriteProposal {
            request: 42,
            actor: "keys-i".to_owned(),
            digest: prompt_digest("fix the failing test"),
            base: "main".to_owned(),
            sha: "a".repeat(40),
        };
        assert_eq!(
            proposal_from_body(&proposal_marker(&proposal)),
            Some(proposal.clone())
        );
        assert_eq!(
            claim_from_body(&claim_marker(9, 1)),
            Some(WriteClaim {
                approval_comment: 9,
                issued_at: 1,
            })
        );
        assert_eq!(result_from_body(&result_marker(9)), Some(9));
        for (body, expected) in [
            (claim_marker(9, 1), true),
            (result_marker(9), true),
            (legacy_reply_marker(9), false),
        ] {
            let comments = vec![json!({
                "user": {"login": "pekin[bot]"},
                "body": body,
            })];
            let mut evidence = ApprovalEvidence::default();
            extend_approval_evidence(&mut evidence, &comments, 9, &proposal, "pekin[bot]");
            assert_eq!(evidence.dispatched, expected);
        }
        for marker in [reply_marker(9), legacy_reply_marker(9)] {
            let comments = vec![json!({
                "user": {"login": "pekin[bot]"},
                "body": marker,
            })];
            assert!(prior_reply_exists(&comments, 9));
        }
        let comments = vec![json!({
            "user": {"login": LEGACY_BOT_LOGIN},
            "body": legacy_reply_marker(9),
        })];
        assert!(prior_reply_exists(&comments, 9));
        assert_eq!(percent_encode("feature/a b"), "feature%2Fa%20b");
    }
}
