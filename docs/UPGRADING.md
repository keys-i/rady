# Upgrading to Rady 0.6.7

Rady is one Rust binary. Install or build it, then use `rady --help`:

```sh
cargo install rady --locked
rady --help
```

## RadDuck cutover

The hosted GitHub App and mention are now **RadDuck** and `@radduck`. The central service uses `RADY_APP_SLUG=radduck`. There is no legacy App name, slug, registration flow, token command, static token, App-ID setting, or compatibility alias.

Rady 0.6.7 changes the consent contract. Every connected repository must rerun setup as an administrator:

```sh
rady setup --repo owner/repo --check test --accept-terms
```

This replaces the old receipt with current Terms and Privacy versions. Existing `.github/rady.json` files are intentionally stale until then.

The machine-readable setup preview and result now use `app`; consumers of the retired `new_app` or `identity` fields must update. The repository configuration remains schema 1 with `source`, `checks`, and `agreement`. Its configured `source` remains a pin: central review rejects work when it does not match the trusted Rady source.

For scripts, use the setup equivalent:

```sh
rady dependasolve --repo owner/repo --check test --apply --accept-terms
```

`dependasolve` only configures the repository. Reviews run later in the central service; it is not a direct review or merge command.

## Review behaviour

The central service polls recent installed work in a bounded, fair rotating window. Mention and discovery tokens are short-lived and installation-scoped. A selected review gets a repository-scoped token with only its required permissions.

Rady verifies the source pin, Dependabot evidence, and chosen checks. Unsupported, grouped, or ambiguous dependency updates receive a comment for manual review. The App has read-only contents access and does not enable, disable, or perform merges. Repository owners retain the final merge decision.

## Removed assumptions

Do not configure an App ID, token command, static token, or workflow in a target repository. Keep the App private key and every service secret only on the central host. Use agent operations only under `rady agent …`; `rady setup`, `rady code`, and `rady dependasolve` remain top-level.

Mentions remain read-only:

```text
@radduck What changed here, and what should I check?
```

Polling is not real-time. A comment or pull request is handled on a later scheduled cycle.
