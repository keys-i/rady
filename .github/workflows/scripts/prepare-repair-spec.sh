#!/usr/bin/env bash
set -euo pipefail

target/release/rady agent prepare-repair \
  --repo "$GH_REPO" \
  --pr "$PR_NUMBER" \
  --expected-head "$EXPECTED_HEAD" \
  --expected-base "$EXPECTED_BASE" \
  --output "$SPEC"
jq -e '
  type == "object" and
  (.task | type == "string" and length > 0 and length <= 32000) and
  (.scope | type == "array" and length > 0 and length <= 20 and all(.[]; type == "string" and length > 0)) and
  (.tasks | type == "array" and length == 1) and
  .checks == ["test"] and
  .acceptance_checks == [{
    "criterion": 0,
    "command": "git diff --check",
    "expected_exit": 0,
    "expected_output": "",
    "files": []
  }]
' "$SPEC" > /dev/null
echo "spec=$SPEC" >> "$GITHUB_OUTPUT"
