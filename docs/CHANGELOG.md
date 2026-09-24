# Changelog

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
