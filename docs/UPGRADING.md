# Upgrading to Pekin 0.6.8

This is the current path from any earlier Rady release, beginning with 0.1.0. You do not need to install each intermediate version. For the release-by-release story, see the [changelog](CHANGELOG.md).

Pekin is one Rust binary. Install or build it, then use `pekin --help`:

```sh
cargo install pekin --locked
pekin --help
```

## Approved-write dispatch

Pekin can now prepare a branch and pull request for an explicit change request. The person who made the request must approve that exact request from the same issue or pull request:

```text
@pekin fix the parser error for empty package names
@pekin approve 123456789
```

The approval is not transferable. Before dispatch, the service rechecks that the conversation is open, the request and approval have the same author, the author still has write, maintain, or admin permission, the bot proposal still matches, and the default branch still points to the recorded commit. It then uses a short-lived token limited to that repository, checkpoints each completed step on an isolated `pekin/...` branch, and opens one pull request. It never merges.

If those checks fail, the base advances, the run is cancelled, or validation fails, no default-branch change is made. Pekin posts a result when the run finishes. A later service pass can mark an abandoned claim expired after two hours; repositories beyond the bounded comment scan need manual review. Claims are never auto-retried. Inspect a missing result before submitting a new request and approval.

## Pekin cutover

The hosted GitHub App and mention are now **Pekin** and `@pekin`. The central service uses `PEKIN_APP_SLUG=pekin`. There is no legacy App name, slug, registration flow, token command, static token, App-ID setting, or compatibility alias.

Before enabling the central workflow, rename the public GitHub App to the `pekin` slug and recreate every central `RADY_*` secret or variable with its `PEKIN_*` name. At minimum that includes `PEKIN_APP_CLIENT_ID`, `PEKIN_APP_PRIVATE_KEY`, `PEKIN_APP_SLUG`, `PEKIN_GEMINI_API_KEY`, and `PEKIN_RELEASE_TOKEN`; keep optional provider keys and private-repository opt-ins under the same new prefix. Old names are ignored.

Pekin 0.6.8 changes the consent contract. Every connected repository must rerun setup as an administrator:

```sh
pekin setup --repo owner/repo --check test --accept-terms
```

This replaces the old receipt with Terms `2026-09-26-t3` and Privacy `2026-09-26-p3`. Existing `.github/pekin.json` files are intentionally stale until then. The renewed agreement discloses that hosted editing may send repository files selected by its constrained provider CLI for the approved task.

The machine-readable setup preview and result now use `app`; consumers of the retired `new_app` or `identity` fields must update. The repository configuration remains schema 1 with `source`, `checks`, and `agreement`. Its configured `source` remains a pin: central review rejects work when it does not match the trusted Pekin source.

For scripts, use the setup equivalent:

```sh
pekin dependasolve --repo owner/repo --check test --apply --accept-terms
```

`dependasolve` only configures the repository. Reviews run later in the central service; it is not a direct review or merge command.

## Review behaviour

The central service polls recent installed work in a bounded, fair rotating window. Mention and discovery tokens are short-lived and installation-scoped. A selected review gets a repository-scoped token with only its required permissions.

Pekin verifies the source pin, Dependabot evidence, and chosen checks. Unsupported, grouped, or ambiguous dependency updates receive a comment for manual review. The App registration reserves Contents write access for approved coding delivery. Mentions and reviews remain read-only. A delivery token is short-lived and limited to the approved repository; Pekin uses it only for the isolated delivery branch and cannot merge. Repository owners retain the final decision.

## Removed assumptions

Do not configure an App ID, token command, static token, or workflow in a target repository. Keep the App private key and every service secret only on the central host. Use agent operations only under `pekin agent …`; `pekin setup`, `pekin code`, and `pekin dependasolve` remain top-level.

Older `rady` or `dependasolver` binaries, App names, `RADY_*` settings, `.github/rady.json`, and local run folders are not Pekin aliases. Keep any historical run evidence you still need; setup does not move it into Pekin’s new state path.

Mentions remain read-only:

```text
@pekin What changed here, and what should I check?
```

Polling is not real-time. A comment or pull request is handled on a later scheduled cycle.
