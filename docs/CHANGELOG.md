# Changelog

## [0.5.7](https://github.com/keys-i/rady/compare/v0.6.6...v0.5.7) (2026-09-24)


### Features

* add bounded multi-model agent loops ([3d404c5](https://github.com/keys-i/rady/commit/3d404c5c31d8c9744aa7abb363e80d2907af33c0))
* add community docs and GitHub App setup wizard ([f44c4ff](https://github.com/keys-i/rady/commit/f44c4ff8be0671ed666a3666d5fa5d128f626853))
* add Dependabot auto-merge workflow ([740b7e7](https://github.com/keys-i/rady/commit/740b7e745b7e028cbaa6ac3ff4ba53cd2e251c9f))
* add free-first model orchestration ([6a6abd6](https://github.com/keys-i/rady/commit/6a6abd6928fac5371fbea8df6576bc3d781f79dc))
* add GitHub App setup wizard ([15fe060](https://github.com/keys-i/rady/commit/15fe060d1ee1a73a3f43d01ed3f245b0b9967b83))
* add opt-in browser verification ([5c06881](https://github.com/keys-i/rady/commit/5c06881737230977ef443bbd15a144a853a2242e))
* add optional Grok fallback ([0f585e6](https://github.com/keys-i/rady/commit/0f585e63c29d26d5363993dd57cd278bf32b6fe0))
* add optional Grok fallback ([825dfbc](https://github.com/keys-i/rady/commit/825dfbcd0469156186071145999b8844fd29cb2a))
* add persistent agent runtime ([078c550](https://github.com/keys-i/rady/commit/078c5506ac89b741d4582dfd5114b6007e11cda4))
* add safe GitHub agent primitives ([ed4ae6d](https://github.com/keys-i/rady/commit/ed4ae6dcebb783753c75e1a82a456dc40392c2a4))
* **agent:** add bounded native harness execution ([daddb1d](https://github.com/keys-i/rady/commit/daddb1dd3faeb28ab22e1cc6a9adbd7975d266f6))
* **app:** configure scoped GitHub App credentials ([ba8c230](https://github.com/keys-i/rady/commit/ba8c2302d18b6f98c7271bd8f64d00839e8c66f0))
* automate trusted GitHub reviews ([1d729e8](https://github.com/keys-i/rady/commit/1d729e81a73ec90f390c746f3c08086e27c8f47d))
* **benchmark:** reject noisy performance regressions ([243ecdd](https://github.com/keys-i/rady/commit/243ecdd1103b7adafe483eda4de45c96c076700a))
* centralize Rady orchestration ([77b29c3](https://github.com/keys-i/rady/commit/77b29c31635c0e568e2a6bfaf07158ba956328b1))
* centralize secure radyybot orchestration ([a9bb3fd](https://github.com/keys-i/rady/commit/a9bb3fd2d3cc4feedb31476d7c7209fd0666c4dc))
* **code:** deliver changes through isolated worktrees ([8f8496c](https://github.com/keys-i/rady/commit/8f8496cc6cfd1fa8a0cf75e5e4be3430b7b5ae09))
* finish Grok workflow setup ([9b9f2b3](https://github.com/keys-i/rady/commit/9b9f2b391174de77bc95d77cbe3a8bb43c4ed8d4))
* **github:** isolate authenticated API access ([8f05df4](https://github.com/keys-i/rady/commit/8f05df4e3b96662326e8166dbdcdea500529cb41))
* invoke Rady with [@radyybot](https://github.com/radyybot) ([f9eca00](https://github.com/keys-i/rady/commit/f9eca00f816c71a7b4340fd076f7a505aebc5e98))
* make Rady setup feel alive ([126c37b](https://github.com/keys-i/rady/commit/126c37bff4753d88561ec91b21190081964b06ee))
* make the radyybot service self-contained ([e405e0c](https://github.com/keys-i/rady/commit/e405e0c557bd63ac872c8194d4b5f8867e2dd35d))
* **quality:** validate plans and evidence gates ([1af1540](https://github.com/keys-i/rady/commit/1af15407905bc509c495e8339919a9a9bb2e2080))
* **review:** evaluate dependency updates from evidence ([c80ba10](https://github.com/keys-i/rady/commit/c80ba10d5a3e0cdf8cd6a2a1288cb86952ec8ee1))
* run routed reviews on GitHub-hosted workers ([c1fa7fb](https://github.com/keys-i/rady/commit/c1fa7fb31ed01351571964c9cff72437983fd4a1))
* **runs:** retain recoverable coding sessions ([6bec944](https://github.com/keys-i/rady/commit/6bec9446cb3f69b13de3b79dc4cb9cc2b45f5e39))
* **ui:** add accessible possum-themed rich output ([ad779eb](https://github.com/keys-i/rady/commit/ad779ebdbb16c21c078a0777fb9b9e60c92cb3bf))


### Bug Fixes

* emit valid planning schemas ([2926e4d](https://github.com/keys-i/rady/commit/2926e4d999608f8942b542f77a500e84262171c8))
* exclude internal workflows from discovery ([5f49bb6](https://github.com/keys-i/rady/commit/5f49bb6f61531c6342006684188ba4d43b0e4fc7))
* harden GitHub App publication boundaries ([1412580](https://github.com/keys-i/rady/commit/141258010dd650784806a41fb9eb9ad408a82bde))
* preserve pending work during provider outages ([f7a1e21](https://github.com/keys-i/rady/commit/f7a1e2186cbfa04d121b97f1aa74ad1c21a702e3))
* prevent privileged cache poisoning ([f397630](https://github.com/keys-i/rady/commit/f397630704f8e2cb22cb461d6a7e21b88ff80842))
* propagate model routing through repository setup ([5046c4d](https://github.com/keys-i/rady/commit/5046c4d595354f9db17031fe493094eb88ae3a7e))
* restore release orchestration ([6e3b9b1](https://github.com/keys-i/rady/commit/6e3b9b1f03fcdb784421456f51de3db771c99661))
* secure app fixtures and provider ([b73bdd6](https://github.com/keys-i/rady/commit/b73bdd61ce2e624ed6ddf54f654535a9455e0a6d))
* send Gemini-compatible ([1c1281e](https://github.com/keys-i/rady/commit/1c1281e4ff8297cdf2215d2efb234cd371a281e8))
* stop dependasolve self-triggering ([d76a928](https://github.com/keys-i/rady/commit/d76a928acb5e4b061b7a37eb2a294e824a16ba50))
* stop dependasolve self-triggering ([f51ad36](https://github.com/keys-i/rady/commit/f51ad36cac54a7a092fdd15a439596fb4def3178))
* validate App access centrally ([7d5e65b](https://github.com/keys-i/rady/commit/7d5e65b0b2924109f033af17113c125f8d4f3576))


### Performance Improvements

* reduce Rady action startup ([9ca4afd](https://github.com/keys-i/rady/commit/9ca4afd5887d8952cb4004d26bdbc3e9a4b8910a))
* reduce redundant action ([d545194](https://github.com/keys-i/rady/commit/d54519482fe92e40847f24e6f6ffc46d0393fcf9))

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
* centralize radyybot orchestration without target-repository secrets or workflows

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
