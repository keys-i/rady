# Rady privacy policy

Effective 25 September 2026 · version `2026-09-25-p1`

This policy covers the hosted RadDuck service maintained through `keys-i/rady`. A self-hosted operator is responsible for its own deployment.

## Information Rady handles

Rady receives the GitHub account and repository information exposed to its App installation. That can include usernames, issue and pull-request text, comments, diffs, paths, commit IDs, check results, and links. It stores consent receipt IDs, the accepting login, policy versions, and acceptance time in the target repository’s `.github/rady.json`.

Rady uses this information to authenticate work, answer `@radduck`, review eligible pull requests, prevent duplicate work, and protect the service. It does not sell repository content or use it for advertising.

## Where information goes

GitHub hosts App installations, repository data, comments, reviews, configuration, and Actions logs. Hosted model requests may send a bounded copy of relevant evidence to a centrally configured provider. Public-repository evidence may use a configured provider; private or unknown repository evidence needs that provider’s separate opt-in. Provider retention, training, and regional processing follow the provider account and terms in force for that request.

App private keys and model credentials stay in the central service. They are not copied into target repositories, prompts, or local child processes. The service uses short-lived installation-scoped tokens for mentions and discovery, and repository-scoped tokens for selected reviews.

## Retention and control

Rady has no separate user-profile or prompt database. GitHub comments, reviews, configuration, and Actions logs follow GitHub and repository retention settings. Providers retain requests under their own terms. Local CLI evidence remains on the operator’s machine until removed.

You can stop new processing by uninstalling the App or removing `.github/rady.json`, and can remove GitHub content where GitHub allows. For an access, correction, or privacy concern that cannot be handled in the repository, use the private route in [Security](SECURITY.md) without sending unnecessary repository content.

## Automated work

Rady uses rules and model output to prepare answers and reviews. It leaves unsupported or ambiguous dependency updates for manual review and does not enable, disable, or perform merges. Repository owners retain the final merge decision.

Material policy changes get a new version and pause central processing until an authorised administrator reruns setup and accepts them.
