# Security policy

## Reporting a vulnerability

Use this repository's **Security** tab and select **Report a vulnerability**. Do not open a public issue for a suspected vulnerability.

Include a minimal reproduction, affected version or commit, impact, and safe mitigation ideas. Redact credentials, private keys, tokens, personal data, and production URLs or data. We will use GitHub's private advisory discussion for coordinated disclosure.

## Supported versions

The latest `main` branch is supported. Fixes are made there; no backports or response-time commitments are promised.

## Rady agent runs

Use authenticated agent CLIs only on machines and runners you control. Private repositories may review every open, non-draft pull request. Public repositories reach the trusted self-hosted runner only after a GitHub-hosted preflight verifies an open, same-repository Dependabot pull request or same-repository pull request from an `OWNER`, `MEMBER`, or `COLLABORATOR`. Public reviews require the bounded Codex or Claude harness; public forks and custom command adapters are rejected. Do not place a subscription credential, model API key, or harness login in a public workflow.

Issue and pull-request mentions are guarded: only an `OWNER`, `MEMBER`, or `COLLABORATOR` may invoke `@rady <prompt>`. The response is read-only evidence; bare `@rady` gives concise usage, and no comment can edit code or publish a change. Treat comment content as untrusted input regardless of author association.

Automatic conflict repair is limited to approved, same-repository Dependabot patch and minor updates with passing selected CI evidence, 95–100% compatibility, no maintainer changes, a confirmed dirty merge state, and at most 20 approved dependency files. Rady binds the original head and base snapshots, stops publication if the base advances, opens a separate replacement PR, and never writes to the Dependabot branch. The short-lived App token is removed from agent and check environments and supplied only to the authenticated Git fetch, push and GitHub API children.

Rady blocks known credential files and private-key material in a proposed change, uses an isolated worktree, and checks that validated files do not change before publication. These checks are not a complete sandbox for an arbitrary custom adapter. Review `RADY_AGENT_COMMAND` and `RADY_REVIEW_COMMAND` as privileged local programs, keep their credentials outside repositories, and use least-privilege GitHub access.

Human evidence reports are generated locally with raw HTML escaped and unsafe link schemes neutralised. They contain no scripts, remote fonts or network resources. Treat retained `run.json`, `run.html`, logs and worktrees as potentially sensitive because they can contain source paths, diffs and command output.
