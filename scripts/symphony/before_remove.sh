#!/usr/bin/env bash
set -euo pipefail

REPO_SLUG="${OPENFANG_GITHUB_REPO:-gagan114662/openfang}"

if ! command -v gh >/dev/null 2>&1; then
  exit 0
fi

branch="$(git branch --show-current 2>/dev/null || true)"
if [[ -z "$branch" ]]; then
  exit 0
fi

if ! gh auth status >/dev/null 2>&1; then
  exit 0
fi

mapfile -t prs < <(gh pr list --repo "$REPO_SLUG" --head "$branch" --state open --json number --jq '.[].number' 2>/dev/null || true)

for pr in "${prs[@]}"; do
  [[ -n "$pr" ]] || continue
  gh pr close "$pr" \
    --repo "$REPO_SLUG" \
    --comment "Closing because the Linear issue for branch $branch entered a terminal state without merge." \
    >/dev/null 2>&1 || true
done
