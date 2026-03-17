#!/usr/bin/env bash
# Sentry -> Codex auto-remediation: fetch findings, fix each via Codex, validate, open PR.
set -euo pipefail

# ---------------------------------------------------------------------------
# Required env vars
# ---------------------------------------------------------------------------
: "${SENTRY_AUTH_TOKEN:?SENTRY_AUTH_TOKEN is required}"
: "${SENTRY_ORG:?SENTRY_ORG is required}"
: "${SENTRY_PROJECT:?SENTRY_PROJECT is required}"

# Optional env vars
SENTRY_QUERY="${SENTRY_QUERY:-is:unresolved level:error}"
CODEX_MODEL="${CODEX_MODEL:-}"
OPENFANG_RUN_INTEGRATION_TESTS="${OPENFANG_RUN_INTEGRATION_TESTS:-false}"

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FINDINGS_FILE="$PROJECT_ROOT/artifacts/sentry-findings.json"
ORIGINAL_BRANCH="$(git -C "$PROJECT_ROOT" rev-parse --abbrev-ref HEAD)"
TIMESTAMP="$(date +%Y%m%dT%H%M%S)"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

# Counters
TOTAL=0
SKIPPED=0
SUCCEEDED=0
FAILED=0

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
log_info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_err()   { echo -e "${RED}[ERR]${NC}   $*"; }
log_step()  { echo -e "${CYAN}[STEP]${NC}  $*"; }

cleanup_branch() {
    local branch="$1"
    log_warn "Cleaning up failed branch: $branch"
    git -C "$PROJECT_ROOT" checkout -- . 2>/dev/null || true
    git -C "$PROJECT_ROOT" clean -fd 2>/dev/null || true
    git -C "$PROJECT_ROOT" checkout "$ORIGINAL_BRANCH" 2>/dev/null || true
    git -C "$PROJECT_ROOT" branch -D "$branch" 2>/dev/null || true
}

validate_changes() {
    log_step "Running validation: cargo build --workspace --lib"
    if ! cargo build --workspace --lib --manifest-path "$PROJECT_ROOT/Cargo.toml" 2>&1; then
        log_err "cargo build failed"
        return 1
    fi

    log_step "Running validation: cargo test --workspace --lib"
    if ! cargo test --workspace --lib --manifest-path "$PROJECT_ROOT/Cargo.toml" 2>&1; then
        log_err "cargo test failed"
        return 1
    fi

    if [ "$OPENFANG_RUN_INTEGRATION_TESTS" = "true" ]; then
        log_step "Running validation: cargo test --workspace (integration tests enabled)"
        if ! cargo test --workspace --manifest-path "$PROJECT_ROOT/Cargo.toml" 2>&1; then
            log_err "cargo test --workspace failed"
            return 1
        fi
    else
        log_warn "Skipping cargo test --workspace integration tests (set OPENFANG_RUN_INTEGRATION_TESTS=true to enable)"
    fi

    log_step "Running validation: cargo clippy"
    if ! cargo clippy --workspace --all-targets --manifest-path "$PROJECT_ROOT/Cargo.toml" -- -D warnings 2>&1; then
        log_err "cargo clippy failed"
        return 1
    fi

    return 0
}

# ---------------------------------------------------------------------------
# Preflight checks
# ---------------------------------------------------------------------------
log_info "Sentry -> Codex auto-remediation starting"
log_info "Org=$SENTRY_ORG  Project=$SENTRY_PROJECT  Query=\"$SENTRY_QUERY\""
log_info "Original branch: $ORIGINAL_BRANCH"

for tool in jq python3 codex gh cargo git; do
    if ! command -v "$tool" &>/dev/null; then
        log_err "Required tool not found: $tool"
        exit 1
    fi
done

# Ensure working tree is clean before we start
if ! git -C "$PROJECT_ROOT" diff --quiet || ! git -C "$PROJECT_ROOT" diff --cached --quiet; then
    log_err "Working tree has uncommitted changes. Commit or stash them first."
    exit 1
fi

# ---------------------------------------------------------------------------
# Step 1: Fetch Sentry findings
# ---------------------------------------------------------------------------
log_step "Fetching Sentry findings via sentry_findings.py"
python3 "$SCRIPT_DIR/harness/sentry_findings.py" \
    --org "$SENTRY_ORG" \
    --project "$SENTRY_PROJECT" \
    --query "$SENTRY_QUERY" \
    --out "$FINDINGS_FILE"

STATUS="$(jq -r '.status' "$FINDINGS_FILE")"
if [ "$STATUS" != "success" ]; then
    ERRORS="$(jq -r '.errors[]' "$FINDINGS_FILE" 2>/dev/null || echo "unknown")"
    log_err "sentry_findings.py failed (status=$STATUS): $ERRORS"
    exit 1
fi

FINDING_COUNT="$(jq '.findings | length' "$FINDINGS_FILE")"
log_info "Fetched $FINDING_COUNT finding(s) from Sentry"

if [ "$FINDING_COUNT" -eq 0 ]; then
    log_info "No findings to process. Exiting."
    exit 0
fi

# ---------------------------------------------------------------------------
# Step 2: Process each finding
# ---------------------------------------------------------------------------
for i in $(seq 0 $((FINDING_COUNT - 1))); do
    TOTAL=$((TOTAL + 1))

    # Extract fields from the normalized finding
    ACTIONABLE="$(jq -r ".findings[$i].actionable" "$FINDINGS_FILE")"
    SUMMARY="$(jq -r ".findings[$i].summary" "$FINDINGS_FILE")"
    SEVERITY="$(jq -r ".findings[$i].severity" "$FINDINGS_FILE")"
    FILEPATH="$(jq -r ".findings[$i].path" "$FINDINGS_FILE")"
    LINE="$(jq -r ".findings[$i].line" "$FINDINGS_FILE")"
    SHORT_ID="$(jq -r ".findings[$i].source.short_id" "$FINDINGS_FILE")"
    PERMALINK="$(jq -r ".findings[$i].source.permalink" "$FINDINGS_FILE")"
    FINDING_ID="$(jq -r ".findings[$i].id" "$FINDINGS_FILE")"

    echo ""
    log_info "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
    log_info "Finding $((i + 1))/$FINDING_COUNT: $SUMMARY"
    log_info "  Severity=$SEVERITY  Path=$FILEPATH  Line=$LINE  ShortID=$SHORT_ID"

    # -----------------------------------------------------------------------
    # Skip non-actionable findings
    # -----------------------------------------------------------------------
    if [ "$ACTIONABLE" != "true" ]; then
        log_warn "Skipping non-actionable finding: $SUMMARY"
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    # -----------------------------------------------------------------------
    # Derive a safe branch name from the short_id
    # -----------------------------------------------------------------------
    if [ -n "$SHORT_ID" ] && [ "$SHORT_ID" != "null" ]; then
        SAFE_ID="$(echo "$SHORT_ID" | tr '[:upper:]' '[:lower:]' | sed 's/[^a-z0-9_-]/-/g')"
    else
        SAFE_ID="$FINDING_ID"
    fi
    BRANCH_NAME="codex/sentry-fix-${SAFE_ID}"

    # Check if branch already exists (skip if so — previous attempt or already fixed)
    if git -C "$PROJECT_ROOT" rev-parse --verify "$BRANCH_NAME" &>/dev/null; then
        log_warn "Branch $BRANCH_NAME already exists. Skipping (may already be fixed)."
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    # -----------------------------------------------------------------------
    # Create temp branch
    # -----------------------------------------------------------------------
    log_step "Creating branch: $BRANCH_NAME"
    git -C "$PROJECT_ROOT" checkout -b "$BRANCH_NAME"

    # -----------------------------------------------------------------------
    # Build Codex prompt
    # -----------------------------------------------------------------------
    PROMPT="You are fixing a Sentry error in the OpenFang project (Rust workspace).

Issue: $SUMMARY
Severity: $SEVERITY"

    if [ -n "$FILEPATH" ] && [ "$FILEPATH" != "null" ] && [ "$FILEPATH" != "" ]; then
        PROMPT="$PROMPT
File: $FILEPATH"
        if [ -n "$LINE" ] && [ "$LINE" != "null" ] && [ "$LINE" != "1" ]; then
            PROMPT="$PROMPT (line $LINE)"
        fi
    fi

    if [ -n "$PERMALINK" ] && [ "$PERMALINK" != "null" ]; then
        PROMPT="$PROMPT
Sentry link: $PERMALINK"
    fi

    PROMPT="$PROMPT

Instructions:
1. Identify the root cause of this error in the codebase.
2. Apply a minimal, targeted fix. Do not refactor unrelated code.
3. Ensure the fix compiles and passes existing tests.
4. If the file path is empty or the error is too vague to fix reliably, explain why and make no changes."

    # -----------------------------------------------------------------------
    # Run Codex
    # -----------------------------------------------------------------------
    log_step "Invoking Codex for remediation..."

    CODEX_ARGS=(exec --json --approval-mode full-auto)
    if [ -n "$CODEX_MODEL" ]; then
        CODEX_ARGS+=(--model "$CODEX_MODEL")
    fi
    CODEX_ARGS+=("$PROMPT")

    CODEX_OUTPUT=""
    if CODEX_OUTPUT="$(cd "$PROJECT_ROOT" && codex "${CODEX_ARGS[@]}" 2>&1)"; then
        log_info "Codex completed successfully"
    else
        log_err "Codex exited with non-zero status"
        log_err "Output: $(echo "$CODEX_OUTPUT" | head -20)"
        cleanup_branch "$BRANCH_NAME"
        FAILED=$((FAILED + 1))
        continue
    fi

    # -----------------------------------------------------------------------
    # Check if Codex made any changes
    # -----------------------------------------------------------------------
    if git -C "$PROJECT_ROOT" diff --quiet && git -C "$PROJECT_ROOT" diff --cached --quiet; then
        log_warn "Codex made no file changes for this finding. Skipping."
        cleanup_branch "$BRANCH_NAME"
        SKIPPED=$((SKIPPED + 1))
        continue
    fi

    CHANGED_FILES="$(git -C "$PROJECT_ROOT" diff --name-only)"
    log_info "Changed files:"
    echo "$CHANGED_FILES" | while IFS= read -r f; do echo "    $f"; done

    # -----------------------------------------------------------------------
    # Validate changes
    # -----------------------------------------------------------------------
    log_step "Validating changes..."
    if ! validate_changes; then
        log_err "Validation FAILED for finding: $SUMMARY"
        cleanup_branch "$BRANCH_NAME"
        FAILED=$((FAILED + 1))
        continue
    fi
    log_info "Validation PASSED"

    # -----------------------------------------------------------------------
    # Commit
    # -----------------------------------------------------------------------
    COMMIT_MSG="fix(sentry): $SUMMARY

Auto-remediation via Codex for Sentry finding $SHORT_ID.
Severity: $SEVERITY
$([ -n "$PERMALINK" ] && [ "$PERMALINK" != "null" ] && echo "Link: $PERMALINK" || true)

Co-Authored-By: Codex <noreply@openai.com>"

    git -C "$PROJECT_ROOT" add -A
    git -C "$PROJECT_ROOT" commit -m "$COMMIT_MSG"
    log_info "Committed on branch $BRANCH_NAME"

    # -----------------------------------------------------------------------
    # Push and open PR
    # -----------------------------------------------------------------------
    log_step "Pushing branch and opening PR..."
    git -C "$PROJECT_ROOT" push -u origin "$BRANCH_NAME"

    PR_TITLE="fix(sentry): $SHORT_ID - $(echo "$SUMMARY" | head -c 60)"

    PR_BODY="$(cat <<EOF
## Summary

Auto-remediation for Sentry finding **$SHORT_ID** (severity: $SEVERITY).

**Issue:** $SUMMARY

$([ -n "$PERMALINK" ] && [ "$PERMALINK" != "null" ] && echo "**Sentry link:** $PERMALINK" || true)

## Changes

$(echo "$CHANGED_FILES" | while IFS= read -r f; do echo "- \`$f\`"; done)

## Validation

All three checks passed before this PR was opened:
- [x] \`cargo build --workspace --lib\`
- [x] \`cargo test --workspace --lib\`
- [x] \`cargo clippy --workspace --all-targets -- -D warnings\`

## Test plan

- [ ] Review the diff to confirm the fix addresses the Sentry error
- [ ] Verify the Sentry issue resolves after merge
- [ ] Monitor for regressions in affected code paths

Generated by \`scripts/sentry_to_codex.sh\` at $TIMESTAMP
EOF
)"

    PR_URL="$(gh pr create \
        --base "$ORIGINAL_BRANCH" \
        --head "$BRANCH_NAME" \
        --title "$PR_TITLE" \
        --body "$PR_BODY" 2>&1)" || {
        log_err "Failed to create PR: $PR_URL"
        FAILED=$((FAILED + 1))
        git -C "$PROJECT_ROOT" checkout "$ORIGINAL_BRANCH"
        continue
    }

    log_info "PR created: $PR_URL"
    SUCCEEDED=$((SUCCEEDED + 1))

    # -----------------------------------------------------------------------
    # Return to original branch for next finding
    # -----------------------------------------------------------------------
    git -C "$PROJECT_ROOT" checkout "$ORIGINAL_BRANCH"
done

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
echo ""
log_info "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
log_info "Sentry -> Codex remediation complete"
log_info "  Total findings: $TOTAL"
log_info "  Succeeded (PR opened): $SUCCEEDED"
log_info "  Skipped (non-actionable / no changes): $SKIPPED"
log_info "  Failed (validation / codex error):     $FAILED"
log_info "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"

if [ "$FAILED" -gt 0 ]; then
    exit 1
fi
exit 0
