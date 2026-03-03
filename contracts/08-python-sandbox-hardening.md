# Contract 08: Python Sandbox Hardening
**Agent:** Codex
**Branch:** `codex/08-python-sandbox`

## Prompt (copy-paste this into Codex)

```
Read CLAUDE.md first (it applies to all agents).

Your task: Harden the Python subprocess sandbox with network and filesystem controls.

The Python runtime in crates/openfang-runtime/src/subprocess_sandbox.rs (or python_runtime.rs) currently lets Python agents access the full filesystem and network with no restrictions.

1. Add two config fields to the appropriate config struct:
   - python_allow_network: bool, default true, with #[serde(default = "default_true")]
   - python_allowed_paths: Vec<String>, default empty (empty = allow all)
   Add defaults to the Default impl.

2. When python_allow_network is false:
   - Remove all proxy env vars (http_proxy, https_proxy, HTTP_PROXY, HTTPS_PROXY, ALL_PROXY, no_proxy)
   - Set the env var OPENFANG_NETWORK_DISABLED=1
   - If on Linux, attempt to use unshare to disable network namespace (best-effort, don't fail if unavailable)

3. When python_allowed_paths is non-empty:
   - Set env var OPENFANG_ALLOWED_PATHS as a colon-separated list of allowed paths
   - Pass the working directory as the FIRST allowed path always

4. Add a timeout config field python_timeout_seconds with default 120 if it doesn't exist already

5. Write tests:
   - Test that when python_allow_network is false, the subprocess env does NOT contain http_proxy
   - Test that when python_allowed_paths is set, OPENFANG_ALLOWED_PATHS env var is correct
   - Test that default config allows network (backward compatible)

When you're done, verify:
1. cargo build --workspace --lib passes
2. cargo test --workspace passes
3. cargo clippy --workspace --all-targets -- -D warnings passes

You are NOT done until all three checks pass.
```

## Verification

```bash
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test python_sandbox -- --nocapture
```
