#!/usr/bin/env bash
set -euo pipefail

# ============================================================
# OpenFang Build Verification Script
# Last commit: 8739387 - fix(runtime): parse CLI stdout errors
# Files changed: 17 (new claude_code + codex_cli drivers)
# ============================================================

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'
PASS=0
FAIL=0
WARN=0

check() {
  if eval "$2" > /dev/null 2>&1; then
    echo -e "${GREEN}[PASS]${NC} $1"
    ((PASS++))
  else
    echo -e "${RED}[FAIL]${NC} $1"
    ((FAIL++))
  fi
}

warn() {
  echo -e "${YELLOW}[WARN]${NC} $1"
  ((WARN++))
}

echo ""
echo "========================================="
echo "  STEP 1: THE THREE MANDATORY CHECKS"
echo "========================================="
echo ""

echo ">> cargo build --workspace --lib"
if cargo build --workspace --lib 2>&1; then
  echo -e "${GREEN}[PASS]${NC} Build succeeded"
  ((PASS++))
else
  echo -e "${RED}[FAIL]${NC} Build failed — STOP HERE, nothing else matters"
  exit 1
fi

echo ""
echo ">> cargo test --workspace"
if cargo test --workspace 2>&1; then
  echo -e "${GREEN}[PASS]${NC} All tests passed"
  ((PASS++))
else
  echo -e "${RED}[FAIL]${NC} Tests failed"
  ((FAIL++))
fi

echo ""
echo ">> cargo clippy --workspace --all-targets -- -D warnings"
if cargo clippy --workspace --all-targets -- -D warnings 2>&1; then
  echo -e "${GREEN}[PASS]${NC} Zero clippy warnings"
  ((PASS++))
else
  echo -e "${RED}[FAIL]${NC} Clippy warnings found"
  ((FAIL++))
fi

echo ""
echo "========================================="
echo "  STEP 2: NEW DRIVERS ARE NOT DEAD CODE"
echo "========================================="
echo ""

check "claude_code module declared in mod.rs" \
  "grep -q 'pub mod claude_code' crates/openfang-runtime/src/drivers/mod.rs"

check "codex_cli module declared in mod.rs" \
  "grep -q 'pub mod codex_cli' crates/openfang-runtime/src/drivers/mod.rs"

check "claude-code provider in create_driver()" \
  "grep -q 'claude-code' crates/openfang-runtime/src/drivers/mod.rs"

check "codex provider in create_driver()" \
  "grep -q '\"codex\"' crates/openfang-runtime/src/drivers/mod.rs"

check "claude-code in known_providers()" \
  "grep -q 'claude-code' crates/openfang-runtime/src/drivers/mod.rs"

check "codex in known_providers()" \
  "grep -q 'codex' crates/openfang-runtime/src/drivers/mod.rs"

echo ""
echo "========================================="
echo "  STEP 3: NO PANICS IN PRODUCTION CODE"
echo "========================================="
echo ""

# Count panics OUTSIDE of test blocks in the new files
CLAUDE_PANICS=$(sed '/#\[cfg(test)\]/,$d' crates/openfang-runtime/src/drivers/claude_code.rs | grep -c 'panic!\|\.unwrap()' || true)
CODEX_PANICS=$(sed '/#\[cfg(test)\]/,$d' crates/openfang-runtime/src/drivers/codex_cli.rs | grep -c 'panic!\|\.unwrap()' || true)

if [ "$CLAUDE_PANICS" -eq 0 ]; then
  echo -e "${GREEN}[PASS]${NC} claude_code.rs: 0 panics/unwraps in production code"
  ((PASS++))
else
  echo -e "${RED}[FAIL]${NC} claude_code.rs: $CLAUDE_PANICS panics/unwraps in production code"
  ((FAIL++))
fi

if [ "$CODEX_PANICS" -eq 0 ]; then
  echo -e "${GREEN}[PASS]${NC} codex_cli.rs: 0 panics/unwraps in production code"
  ((PASS++))
else
  echo -e "${RED}[FAIL]${NC} codex_cli.rs: $CODEX_PANICS panics/unwraps in production code"
  ((FAIL++))
fi

echo ""
echo "========================================="
echo "  STEP 4: PROPER ERROR TYPES (NO STRING)"
echo "========================================="
echo ""

CLAUDE_STRING_ERR=$(grep -c 'Result<.*String>' crates/openfang-runtime/src/drivers/claude_code.rs || true)
CODEX_STRING_ERR=$(grep -c 'Result<.*String>' crates/openfang-runtime/src/drivers/codex_cli.rs || true)

if [ "$CLAUDE_STRING_ERR" -eq 0 ]; then
  echo -e "${GREEN}[PASS]${NC} claude_code.rs: no Result<T, String> anti-pattern"
  ((PASS++))
else
  echo -e "${RED}[FAIL]${NC} claude_code.rs: $CLAUDE_STRING_ERR uses of Result<T, String>"
  ((FAIL++))
fi

if [ "$CODEX_STRING_ERR" -eq 0 ]; then
  echo -e "${GREEN}[PASS]${NC} codex_cli.rs: no Result<T, String> anti-pattern"
  ((PASS++))
else
  echo -e "${RED}[FAIL]${NC} codex_cli.rs: $CODEX_STRING_ERR uses of Result<T, String>"
  ((FAIL++))
fi

echo ""
echo "========================================="
echo "  STEP 5: NO TODOS/FIXMES IN NEW CODE"
echo "========================================="
echo ""

TODO_COUNT=$(grep -ci 'TODO\|FIXME\|HACK\|XXX' crates/openfang-runtime/src/drivers/claude_code.rs crates/openfang-runtime/src/drivers/codex_cli.rs || true)

if [ "$TODO_COUNT" -eq 0 ]; then
  echo -e "${GREEN}[PASS]${NC} No TODO/FIXME/HACK comments in new drivers"
  ((PASS++))
else
  echo -e "${YELLOW}[WARN]${NC} Found $TODO_COUNT TODO/FIXME/HACK comments"
  grep -ni 'TODO\|FIXME\|HACK\|XXX' crates/openfang-runtime/src/drivers/claude_code.rs crates/openfang-runtime/src/drivers/codex_cli.rs || true
  ((WARN++))
fi

echo ""
echo "========================================="
echo "  STEP 6: TEST COVERAGE FOR NEW CODE"
echo "========================================="
echo ""

CLAUDE_TESTS=$(grep -c '#\[test\]' crates/openfang-runtime/src/drivers/claude_code.rs || true)
CODEX_TESTS=$(grep -c '#\[test\]' crates/openfang-runtime/src/drivers/codex_cli.rs || true)

if [ "$CLAUDE_TESTS" -ge 5 ]; then
  echo -e "${GREEN}[PASS]${NC} claude_code.rs: $CLAUDE_TESTS unit tests"
  ((PASS++))
else
  echo -e "${YELLOW}[WARN]${NC} claude_code.rs: only $CLAUDE_TESTS tests (want at least 5)"
  ((WARN++))
fi

if [ "$CODEX_TESTS" -ge 5 ]; then
  echo -e "${GREEN}[PASS]${NC} codex_cli.rs: $CODEX_TESTS unit tests"
  ((PASS++))
else
  echo -e "${YELLOW}[WARN]${NC} codex_cli.rs: only $CODEX_TESTS tests (want at least 5)"
  ((WARN++))
fi

echo ""
echo "========================================="
echo "  STEP 7: HARDCODED VALUES CHECK"
echo "========================================="
echo ""

HARDCODED_TIMEOUT=$(grep -c 'from_secs(120)' crates/openfang-runtime/src/drivers/claude_code.rs crates/openfang-runtime/src/drivers/codex_cli.rs || true)

if [ "$HARDCODED_TIMEOUT" -gt 0 ]; then
  warn "Found $HARDCODED_TIMEOUT hardcoded 120s timeouts — consider making configurable"
else
  echo -e "${GREEN}[PASS]${NC} No hardcoded timeouts"
  ((PASS++))
fi

echo ""
echo "========================================="
echo "  STEP 8: LIVE INTEGRATION TEST"
echo "========================================="
echo ""
echo "  (Run these manually after starting daemon)"
echo ""
echo "  # Build release binary:"
echo "  cargo build --release -p openfang-cli"
echo ""
echo "  # Start daemon:"
echo "  GROQ_API_KEY=<your-key> target/release/openfang.exe start &"
echo "  sleep 6"
echo ""
echo "  # Verify health:"
echo "  curl -s http://127.0.0.1:4200/api/health"
echo ""
echo "  # Check new providers are listed:"
echo "  curl -s http://127.0.0.1:4200/api/agents | python3 -m json.tool"
echo ""
echo "  # If you have claude CLI installed, test the driver:"
echo "  curl -s -X POST 'http://127.0.0.1:4200/api/agents/<AGENT_ID>/message' \\"
echo "    -H 'Content-Type: application/json' \\"
echo "    -d '{\"message\": \"Say hello in 5 words.\"}'"
echo ""

echo ""
echo "========================================="
echo "  RESULTS"
echo "========================================="
echo ""
echo -e "  ${GREEN}PASS: $PASS${NC}"
echo -e "  ${RED}FAIL: $FAIL${NC}"
echo -e "  ${YELLOW}WARN: $WARN${NC}"
echo ""

if [ "$FAIL" -gt 0 ]; then
  echo -e "${RED}VERDICT: NEEDS FIXING${NC}"
  exit 1
elif [ "$WARN" -gt 0 ]; then
  echo -e "${YELLOW}VERDICT: PASSES WITH WARNINGS${NC}"
  exit 0
else
  echo -e "${GREEN}VERDICT: ALL CLEAR${NC}"
  exit 0
fi
