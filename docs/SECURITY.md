# Security policy

## Report privately

Use this repository's **Security** tab → **Report a vulnerability**. Do not open a public issue. Include a small reproduction, affected version or commit, impact, and a safe mitigation idea. Remove credentials, keys, tokens, personal data, and production URLs or data. We coordinate in GitHub's private advisory discussion.

## Support

`main` is the supported line. Fixes land there; backports and response times are not promised.

## Where Rady may run

Use authenticated agent CLIs only on machines and runners you control. Private repositories may review every open, non-draft pull request. Public repositories first pass GitHub-hosted preflight and then admit only same-repository Dependabot updates or same-repository pull requests from an `OWNER`, `MEMBER`, or `COLLABORATOR`. Public forks and custom adapters are rejected; public reviews require the bounded Codex or Claude harness.

`RADY_MODEL_CHOICES` lists native-harness models from least to most capable. `RADY_MODEL` pins one and overrides tier selection. Broad work is split into at most eight serial, scoped tasks. Failed evidence may move to the next model within the attempt limit; unchanged repairs stop. Never put subscription credentials, model API keys, or harness logins in a public workflow.

## Mentions and hosted models

Only an `OWNER`, `MEMBER`, or `COLLABORATOR` can invoke `@radyybot <prompt>`. Replies are read-only evidence; bare `@radyybot` shows usage, and no comment can edit code or publish a change. Treat every comment as untrusted input.

On a machine with authenticated Codex, Rady uses it. GitHub-hosted mentions instead route simple public requests through Gemini 3.5 Flash-Lite then GPT-OSS, and balanced or deep work through Gemini 3.8 Flash, Qwen 3.8 27B, then GPT-OSS. Paid Grok 4.7 is an optional final fallback with `RADY_XAI_API_KEY`. Store provider keys only as repository- or organisation-scoped GitHub Actions secrets. They are removed from native-agent children and never cached.

Private and unknown repositories skip Gemini and Grok unless `RADY_GEMINI_PRIVATE_OK=true` or `RADY_XAI_PRIVATE_OK=true` explicitly allows them. Gemini's free tier may use content to improve Google products. xAI says API input and output are not used for training without permission, but normally retains them for 30 days unless the xAI team enables Zero Data Retention. Rady selects the provider locally; that provider receives bounded issue, pull-request, or comment evidence. Do not enable the responder when that transfer is unacceptable.

## Change and delivery boundaries

Automatic conflict repair is limited to approved same-repository Dependabot patch or minor updates: selected CI must pass, compatibility must be 95–100%, no maintainer files may change, the original pull request must be genuinely conflicted, and at most 20 approved dependency files may be touched. Rady binds the original head and base, stops if the base moves, opens a separate replacement PR, and never writes to the Dependabot branch.

Rady blocks known credential and private-key files, works in an isolated worktree, and confirms validated files remain unchanged before publication. App tokens are short-lived, stripped from agent and check environments, and given only to authenticated Git fetch, push, and GitHub API children. `RADY_AGENT_COMMAND` and `RADY_REVIEW_COMMAND` are privileged local programs, not sandboxes: review them, keep credentials out of repositories, and use least-privilege GitHub access.

Local HTML evidence escapes raw HTML, neutralises unsafe links, and uses no scripts, remote fonts, or network resources. Treat retained `run.json`, `run.html`, logs, and worktrees as sensitive: they can contain source paths, diffs, and command output.
