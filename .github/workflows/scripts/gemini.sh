#!/usr/bin/env bash
set -euo pipefail

readonly script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
readonly policy="$script_dir/gemini-policy.toml"
readonly workspace="$PWD"
[[ -r "$policy" && -n "${GEMINI_API_KEY:-}" ]] || exit 1
compgen -G '/etc/gemini-cli/policies/*.toml' > /dev/null && exit 1
[[ -d "$workspace" ]] || exit 1
for unsafe in .gemini .env GEMINI.md; do
  unsafe_path="$workspace/$unsafe"
  if [[ -e "$unsafe_path" || -L "$unsafe_path" ]]; then
    printf 'Koelu stopped: hosted editing will not load repository-controlled Gemini configuration: %s\n' "$unsafe" >&2
    exit 1
  fi
done
cd -- "$workspace"

export GEMINI_CLI_HOME="${RUNNER_TEMP:?}/koelu-gemini"

approval_mode=auto_edit
if [[ "${KOELU_READ_ONLY:-0}" == 1 ]]; then
  approval_mode=plan
fi

arguments=(--skip-trust --approval-mode "$approval_mode" --admin-policy "$policy" --output-format json --prompt '')
if [[ -n "${KOELU_MODEL:-}" ]]; then
  arguments+=(--model "$KOELU_MODEL")
fi
gemini "${arguments[@]}" | jq -er '.response | strings'
