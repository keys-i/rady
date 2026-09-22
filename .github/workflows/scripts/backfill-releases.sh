#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
config="$root/tools/config/release-backfill.json"
apply=false

case "${1:-}" in
  "") ;;
  --apply) apply=true ;;
  *)
    echo "Usage: .github/workflows/scripts/backfill-releases.sh [--apply]" >&2
    exit 2
    ;;
esac

for command in git jq; do
  command -v "$command" >/dev/null || {
    echo "$command is required" >&2
    exit 1
  }
done
if [[ "$apply" == true ]]; then
  command -v gh >/dev/null || {
    echo "gh is required to publish releases" >&2
    exit 1
  }
fi

schema=$(jq -er '.schema | select(. == 1)' "$config")
repository=$(jq -er '.repository | select(type == "string")' "$config")
from_version=$(jq -er '.fromVersion | select(type == "string")' "$config")
through_version=$(jq -er '.throughVersion | select(type == "string")' "$config")
tag_prefix=$(jq -er '.tagPrefix | select(type == "string")' "$config")
semver='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$'

[[ "$schema" == 1 ]]
[[ "$repository" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] || {
  echo "release repository is invalid" >&2
  exit 1
}
[[ "$from_version" =~ $semver && "$through_version" =~ $semver ]] || {
  echo "release version boundary is invalid" >&2
  exit 1
}
[[ "$tag_prefix" =~ ^[A-Za-z0-9._-]*$ ]] || {
  echo "release tag prefix is invalid" >&2
  exit 1
}

cd "$root"
[[ "$(git rev-parse --show-toplevel)" == "$root" ]] || {
  echo "run from the Rady repository" >&2
  exit 1
}

package_version() {
  awk '
    $0 == "[package]" { package = 1; next }
    /^\[/ { package = 0 }
    package && $1 == "version" && $2 == "=" {
      gsub(/"/, "", $3)
      print $3
      exit
    }
  '
}

plan_file=$(mktemp "${TMPDIR:-/tmp}/rady-release-plan.XXXXXX")
trap 'rm -f -- "$plan_file"' EXIT
found_from=false
found_through=false
previous_version=""

while IFS= read -r commit; do
  version=$(git show "$commit:Cargo.toml" | package_version)
  [[ -n "$version" && "$version" =~ $semver ]] || {
    echo "invalid package version at $commit" >&2
    exit 1
  }
  [[ "$version" != "$previous_version" ]] || continue
  previous_version=$version
  if [[ "$found_from" == false ]]; then
    [[ "$version" == "$from_version" ]] || continue
    found_from=true
  fi
  if awk -F '\t' -v version="$version" '$1 == version { found = 1 } END { exit !found }' "$plan_file"; then
    echo "package version $version appears in more than one history segment" >&2
    exit 1
  fi
  printf '%s\t%s\n' "$version" "$commit" >> "$plan_file"
  if [[ "$version" == "$through_version" ]]; then
    found_through=true
    break
  fi
done < <(git log --first-parent --reverse --format=%H -- Cargo.toml)

[[ "$found_from" == true && "$found_through" == true ]] || {
  echo "Cargo.toml history does not cover $from_version through $through_version" >&2
  exit 1
}
current_version=$(package_version < Cargo.toml)
[[ "$current_version" == "$through_version" ]] || {
  echo "Cargo.toml is $current_version, expected $through_version" >&2
  exit 1
}

echo "Rady retrospective release plan"
while IFS=$'\t' read -r version commit; do
  printf '  %-14s %.12s\n' "${tag_prefix}${version}" "$commit"
done < "$plan_file"

if [[ "$apply" == false ]]; then
  echo "Plan only; rerun with --apply and RADY_RELEASE_CONFIRM=BACKFILL to publish"
  exit 0
fi

[[ "${RADY_RELEASE_CONFIRM:-}" == BACKFILL ]] || {
  echo "set RADY_RELEASE_CONFIRM=BACKFILL to publish" >&2
  exit 1
}
: "${GH_TOKEN:?GH_TOKEN is required to publish releases}"
[[ -z "$(git status --porcelain)" ]] || {
  echo "release backfill requires a clean checkout" >&2
  exit 1
}
git fetch --tags
authenticated_repository=$(gh repo view --json nameWithOwner --jq .nameWithOwner)
[[ "$authenticated_repository" == "$repository" ]] || {
  echo "authenticated GitHub repository does not match $repository" >&2
  exit 1
}
gh api "repos/$repository" --silent

while IFS=$'\t' read -r version commit; do
  tag="${tag_prefix}${version}"
  git merge-base --is-ancestor "$commit" HEAD || {
    echo "$commit is not reachable from HEAD" >&2
    exit 1
  }
  tag_exists=false
  if git rev-parse --verify --quiet "refs/tags/$tag^{commit}" >/dev/null; then
    tag_exists=true
    tagged_commit=$(git rev-list -n 1 "$tag")
    [[ "$tagged_commit" == "$commit" ]] || {
      echo "$tag already points to a different commit; refusing to retag" >&2
      exit 1
    }
  fi
  if release_error=$(gh api "repos/$repository/releases/tags/$tag" --silent 2>&1); then
    echo "$tag already has a release; leaving it unchanged"
    continue
  elif [[ "$release_error" != *"HTTP 404"* ]]; then
    printf '%s\n' "$release_error" >&2
    exit 1
  fi
  latest=(--latest=false)
  [[ "$version" == "$through_version" ]] && latest=(--latest)
  if [[ "$tag_exists" == true ]]; then
    gh release create "$tag" \
      --repo "$repository" \
      --verify-tag \
      --title "Rady $version" \
      --generate-notes \
      "${latest[@]}"
  else
    gh release create "$tag" \
      --repo "$repository" \
      --target "$commit" \
      --title "Rady $version" \
      --generate-notes \
      "${latest[@]}"
  fi
done < "$plan_file"
