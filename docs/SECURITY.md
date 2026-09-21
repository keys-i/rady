# Security policy

## Reporting a vulnerability

Use this repository's **Security** tab and select **Report a vulnerability**. Do not open a public issue for a suspected vulnerability.

Include a minimal reproduction, affected version or commit, impact, and safe mitigation ideas. Redact credentials, private keys, tokens, personal data, and production URLs or data. We will use GitHub's private advisory discussion for coordinated disclosure.

## Supported versions

The latest `main` branch is supported. Fixes are made there; no backports or response-time commitments are promised.

## Rady agent runs

Use authenticated agent CLIs only on machines and runners you control. The included GitHub review workflow rejects public repositories and requires a trusted self-hosted runner; do not place a subscription credential, model API key, or harness login in a public workflow.

Rady blocks known credential files and private-key material in a proposed change, uses an isolated worktree, and checks that validated files do not change before publication. These checks are not a complete sandbox for an arbitrary custom adapter. Review `RADY_AGENT_COMMAND` and `RADY_REVIEW_COMMAND` as privileged local programs, keep their credentials outside repositories, and use least-privilege GitHub access.

Human evidence reports are generated locally with raw HTML escaped and unsafe link schemes neutralised. They contain no scripts, remote fonts or network resources. Treat retained `run.json`, `run.html`, logs and worktrees as potentially sensitive because they can contain source paths, diffs and command output.
