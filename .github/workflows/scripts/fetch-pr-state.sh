#!/usr/bin/env bash
set -euo pipefail

[[ "$PR_NUMBER" =~ ^[1-9][0-9]*$ ]] || {
  echo 'PR number must be positive' >&2
  exit 1
}
pull=''
for _ in 1 2 3 4 5 6; do
  pull=$(gh api "repos/$GH_REPO/pulls/$PR_NUMBER")
  mergeable=$(jq -r '.mergeable // "unknown"' <<<"$pull")
  mergeable_state=$(jq -r '.mergeable_state // "unknown"' <<<"$pull")
  if [[ "$mergeable" != unknown && "$mergeable_state" != unknown ]]; then
    break
  fi
  sleep 2
done
jq -e --arg head "$EXPECTED_HEAD" '
  .state == "open" and .draft == false and
  .head.sha == $head and
  (.html_url | type == "string" and startswith("https://")) and
  (.base.ref | type == "string" and length > 0) and
  (.base.sha | type == "string" and test("^[a-f0-9]{40}$"))
' <<<"$pull" > /dev/null
event="$RUNNER_TEMP/rady-pull-request-event.json"
jq -n --argjson pull "$pull" '{pull_request: $pull}' > "$event"
{
  echo "event=$event"
  echo "url=$(jq -r .html_url <<<"$pull")"
  echo "head=$(jq -r .head.sha <<<"$pull")"
  echo "base=$(jq -r .base.ref <<<"$pull")"
  echo "base_sha=$(jq -r .base.sha <<<"$pull")"
  echo "mergeable=$mergeable"
  echo "mergeable_state=$mergeable_state"
} >> "$GITHUB_OUTPUT"
