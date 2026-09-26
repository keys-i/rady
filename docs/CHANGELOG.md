# Changelog

## [0.6.10](https://github.com/keys-i/koelu/compare/v0.6.9...v0.6.10) (2026-09-27)

- Prepare the repository move to `keys-i/koelu` and accept that name as the trusted solver source
- Point setup, workflows, package metadata, the Homebrew tap, and current documentation to the new repository

## [0.6.9](https://github.com/keys-i/rady/compare/v0.6.7...v0.6.9) (2026-09-27)

- Rename the crate, CLI, GitHub App mention, repository configuration, state paths, central secrets, Homebrew formula, and poster to Koelu
- Keep the duck mascot while taking the new name from the koel
- Require a new repository agreement under Terms `2026-09-27-t4` and Privacy `2026-09-27-p4` before hosted work resumes
- Keep the Rady formula and crate as migration notices; Pekin was never published, and local run evidence is not moved automatically

### Features

* allow an explicit `@koelu` write request to prepare an isolated branch and pull request after the same author approves its exact comment
* run hosted code changes through a pinned, file-only Gemini CLI harness
* checkpoint completed hosted work as progressive commits before opening one pull request

### Breaking changes

* rename the product, crate, binary, App, mention, configuration, environment variables, state paths, and Homebrew formula to Koelu/`koelu` with no command alias
* publish a notice-only `rady` crate and deprecate the old Homebrew formula with Koelu named as its replacement
* require connected repositories to install the Koelu App and rerun `koelu setup`

### Security

* revalidate the request, approval, current author permission, and recorded base revision before dispatch
* use a short-lived repository-scoped delivery token only for the approved write; Koelu never merges
* refuse repository-controlled Gemini configuration before the hosted harness starts
* require renewed consent for the hosted provider-CLI file disclosure

### Recovery

* post a terminal result for every claimed dispatch and never retry a claimed approval automatically
* stop on invalid approval, changed access or base, cancellation, or failed checks without changing the default branch; retry with a fresh request and approval

## [0.6.7](https://github.com/keys-i/rady/compare/v0.6.6...v0.6.7) (2026-09-25)

### Changes

* rename the hosted GitHub App and mention to RadDuck and `@radduck` with no legacy alias
* rotate bounded dependency review windows fairly and honour the configured source pin
* leave unsupported, grouped, or ambiguous Dependabot updates for protected human review

### Breaking changes

* require every connected repository to rerun setup for the new Terms and Privacy receipt
* replace machine-readable setup preview fields `new_app` and `identity` with `app`
* remove the standalone `dependasolver` binary, legacy App registration and auth settings, and root agent aliases

### Security

* keep public App contents access read-only; RadDuck cannot push contents or perform merges
* use short-lived installation tokens for discovery and mentions, then repository-scoped review tokens

## [0.6.6](https://github.com/keys-i/rady/compare/v0.6.5...v0.6.6) (2026-09-25)

### Features

* add opt-in `rady code --browser` verification through a configured local browser MCP

## [0.6.5](https://github.com/keys-i/rady/compare/v0.6.4...v0.6.5) (2026-09-25)

### Features

* add optional authenticated-loopback Laya routing for intent and tier selection
* treat temporary hosted-model outages as retryable service work

### Security

* harden provider credential environments and align setup consent with the current Privacy policy

## [0.6.4](https://github.com/keys-i/rady/compare/v0.6.3...v0.6.4) (2026-09-25)

### Features

* add free-first routing across six model providers, bounded catalogs and cooldowns, plus a Deep evidence scout

## [0.6.3](https://github.com/keys-i/rady/compare/v0.6.2...v0.6.3) (2026-09-25)

### Fixes

* keep animated terminal progress cancellable, non-interleaving, and quiet outside interactive terminals
* pass GitHub App JWTs through standard input instead of process arguments
* bound and neutralize generated pull-request text before publication

### Interface

* make setup a seven-stage flow with one gradual, readable progress line
* make service, review, and recovery messages shorter and more natural

### Security

* request read-only repository contents access for new GitHub App manifests

## [0.6.2](https://github.com/keys-i/rady/compare/v0.6.1...v0.6.2) (2026-09-24)

### Features

* add `rady setup` with repository and CI-check discovery
* centralize GitHub App orchestration without target-repository secrets or workflows

### Security

* verify checkout identity, consent receipts, and administrator access before setup
* preflight bounded local configuration before browser or GitHub writes

## [0.6.1](https://github.com/keys-i/rady/compare/v0.6.0...v0.6.1) (2026-09-24)

### Features

* add an App-authenticated service for mentions and pull-request reviews
* mint and refresh short-lived installation tokens across installed repositories

### Maintenance

* split agent, delivery, provider, review, setup, and GitHub authentication code into focused modules

## [0.6.0](https://github.com/keys-i/rady/compare/v0.5.8...v0.6.0) (2026-09-24)

### Features

* add `rady agent ask`, follow-ups, and the persistent service command
* add bounded project guidance, explicit skills, selected local MCP servers, and conversation memory
* add intent routing, evidence briefs, progressive checkpoints, and `--ghost`

### Security

* expand credential scrubbing for native harnesses and MCP processes

## [0.5.8](https://github.com/keys-i/rady/compare/v0.5.7...v0.5.8) (2026-09-22)

### Bug Fixes

* validate App access centrally ([7d5e65b](https://github.com/keys-i/rady/commit/7d5e65b0b2924109f033af17113c125f8d4f3576))

## Earlier Rady releases (0.1.0–0.5.7)

This is a compact history of the recorded early versions, drawn from their packaged README and upgrade guides. It names milestones visible in those artifacts, not invented patch notes. Missing version numbers are not implied releases; old names and permissions below are historical, not setup advice for Koelu.

| Versions | What the packaged documentation establishes |
| --- | --- |
| 0.1.0 | The native Rust `rady` CLI combined checked local code changes and Dependabot review. It retained JSON and HTML evidence and used an existing Codex or Claude Code login. |
| 0.1.5, 0.1.6, 0.1.7 | The 0.1.x line added default-branch source pinning and a guarded path for same-repository public Dependabot reviews. The packaged guides do not distinguish these three patch releases. |
| 0.2.0, 0.2.1 | Rady moved from separate review Apps to one Rady App, and the interface adopted the duck identity. The packaged guides do not distinguish the patch release. |
| 0.3.1, 0.3.5 | By 0.3.5, the CLI had the `rady agent` command group, guarded read-only `@radyybot` replies, broader trusted same-repository review, and a separate replacement-PR path for eligible conflicted Dependabot updates. |
| 0.4.2 | Mention replies gained a Gemini/Cerebras route; native work gained bounded task decomposition and model selection. |
| 0.5.2, 0.5.3, 0.5.4 | By 0.5.2, mentions preferred a local Codex login when present and used Gemini-first hosted fallbacks otherwise, with optional xAI routing. The available guides do not assign a separate change to each patch. |
| 0.5.5, 0.5.6, 0.5.7 | By 0.5.7, the central `keys-i/rady` service held App and model credentials, recorded repository consent, and answered read-only mentions. The available guides do not assign a separate change to each patch. |

For an upgrade from any of these versions, use the [current Koelu guide](UPGRADING.md); do not replay historical setup instructions.
