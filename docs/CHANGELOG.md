# Changelog

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
