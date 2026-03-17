# OpenFang Autonomy — One-Shot Implementation Plan

**Date:** 2026-03-05
**Prereq:** [Hypothesis report](./2026-03-05-autonomy-audit-design.md)
**Principle:** Every fix emits to Sentry UI. No silent changes.

---

## Revised Findings After Recon

| Hypothesis | Original Verdict | Recon Update |
|---|---|---|
| H2: No WAL | CONFIRMED | **REFUTED** — `substrate.rs:42` already has `PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000` |
| H3: Zombies | CONFIRMED | Confirmed — `child.wait_with_output()` consumes handle; PID-based kill needed |
| All others | Unchanged | Exact line numbers mapped |

**Scope reduced:** 9 files, ~180 net lines. WAL is already done.

---

## Execution Order (file-by-file, each touched once)

### 1. `crates/openfang-runtime/src/drivers/codex_cli.rs`

**What:** Kill zombie process on timeout + Sentry breadcrumb
**Line:** 209-225
**Pattern:** Capture `child.id()` before timeout, kill by PID in `Err(_)` branch

```diff
+    let child_pid = child.id();
     // Wait with timeout — kill child on timeout to avoid orphaned processes
     let output = match tokio::time::timeout(
         ...
     {
         Ok(result) => { ... }
         Err(_) => {
             warn!("codex CLI subprocess timed out after 120s, killing child");
+            if let Some(pid) = child_pid {
+                #[cfg(unix)]
+                { let _ = std::process::Command::new("kill").args(["-9", &pid.to_string()]).status(); }
+                #[cfg(windows)]
+                { let _ = std::process::Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).status(); }
+            }
+            sentry::add_breadcrumb(sentry::Breadcrumb {
+                category: Some("subprocess".into()),
+                message: Some(format!("Killed zombie codex CLI (pid={:?}) after 120s timeout", child_pid)),
+                level: sentry::Level::Error,
+                ..Default::default()
+            });
+            sentry::capture_message("codex CLI subprocess killed after timeout", sentry::Level::Warning);
             return Err(LlmError::Http("codex CLI subprocess timed out..."));
         }
     };
```

### 2. `crates/openfang-runtime/src/drivers/claude_code.rs`

**What:** Same zombie kill pattern, TWO locations (first call + retry)
**Lines:** 72-87 (first), 132-147 (retry)
**Same diff pattern as codex_cli.rs** — capture PID, kill, Sentry breadcrumb

### 3. `crates/openfang-runtime/src/agent_loop.rs`

**What:** Emit Sentry structured log when task hits DLQ (final failure after retries)
**Lines:** 1489 (rate limit), 1517 (overload)
**Pattern:** Use existing `sentry_logs::capture_structured_log()` with `event.kind = "runtime.dlq.enqueued"`

```diff
  // After emit_llm_call_failed_log() at line 1488
+ sentry_logs::capture_structured_log(
+     sentry::protocol::LogLevel::Error,
+     format!("DLQ: rate limited after {} retries — {}", MAX_RETRIES, request.model),
+     {
+         let mut attrs = std::collections::BTreeMap::new();
+         attrs.insert("event.kind".into(), serde_json::json!("runtime.dlq.enqueued"));
+         attrs.insert("dlq.reason".into(), serde_json::json!("rate_limited"));
+         attrs.insert("dlq.attempts".into(), serde_json::json!(MAX_RETRIES + 1));
+         attrs.insert("model".into(), serde_json::json!(request.model.clone()));
+         attrs.insert("provider".into(), serde_json::json!(provider.unwrap_or("unknown")));
+         attrs
+     },
+ );
```

Same pattern for overload at line 1516.

### 4. `crates/openfang-api/src/routes.rs`

**What:** Enrich `/api/health/detail` with provider cooldown states
**Line:** 2643-2652
**Pattern:** Read cooldown state from kernel, add to JSON response

```diff
  Json(serde_json::json!({
      "status": status,
      ...
      "config_warnings": config_warnings,
+     "providers": {
+         // Expose cooldown state for each known provider
+         // Reads from auth_cooldown CircuitBreaker if available
+     },
  }))
```

### 5. `crates/openfang-runtime/src/drivers/anthropic.rs`

**What:** Warn log before `unwrap_or_default()` on tool input JSON parsing
**Line:** 486

```diff
- let input: serde_json::Value =
-     serde_json::from_str(input_json).unwrap_or_default();
+ let input: serde_json::Value = match serde_json::from_str(input_json) {
+     Ok(v) => v,
+     Err(e) => {
+         warn!(error = %e, "Anthropic tool input JSON parse failed, defaulting to empty");
+         sentry::add_breadcrumb(sentry::Breadcrumb {
+             category: Some("llm.parse".into()),
+             message: Some(format!("Anthropic tool input parse error: {e}")),
+             level: sentry::Level::Warning,
+             ..Default::default()
+         });
+         serde_json::Value::default()
+     }
+ };
```

### 6. `crates/openfang-runtime/src/drivers/openai.rs`

**What:** Same pattern at 3 locations: lines 392, 776, 792
**Same diff as anthropic.rs** but with "OpenAI" in the message

### 7. `crates/openfang-runtime/src/web_fetch.rs`

**What:** Replace `unwrap_or_default()` on client builder with proper error + Sentry
**Line:** 24-27

```diff
  let client = reqwest::Client::builder()
      .timeout(std::time::Duration::from_secs(config.timeout_secs))
      .build()
-     .unwrap_or_default();
+     .unwrap_or_else(|e| {
+         warn!(error = %e, "Web fetch HTTP client builder failed, using defaults");
+         sentry::capture_message(
+             &format!("HTTP client builder failed: {e}"),
+             sentry::Level::Warning,
+         );
+         reqwest::Client::new()
+     });
```

### 8. `crates/openfang-runtime/src/web_search.rs`

**What:** Same pattern as web_fetch.rs
**Line:** 33-36

### 9. `crates/openfang-kernel/src/config.rs`

**What:** Call `validate()` on initial config load + Sentry warning
**Line:** 54-56

```diff
  Ok(config) => {
      info!(path = %config_path.display(), "Loaded configuration");
+     let warnings = config.validate();
+     if !warnings.is_empty() {
+         tracing::warn!(warnings = ?warnings, "Config validation warnings on load");
+         sentry::capture_message(
+             &format!("Config validation: {} warnings", warnings.len()),
+             sentry::Level::Warning,
+         );
+     }
      return config;
  }
```

---

## Sentry Event Summary (what shows in your UI)

| Event Kind | Level | When | Dashboard Section |
|---|---|---|---|
| `subprocess.killed` | Warning (capture_message) | CLI driver timeout | Issues |
| `runtime.dlq.enqueued` | Error (structured log) | All retries exhausted | Logs |
| `llm.parse` | Warning (breadcrumb) | Malformed tool JSON | Breadcrumbs on next error |
| `HTTP client builder failed` | Warning (capture_message) | reqwest build fails | Issues |
| `Config validation` | Warning (capture_message) | Invalid config on boot | Issues |
| Health detail provider states | N/A (API response) | GET /api/health/detail | Cron monitor |

---

## Verification

After all edits:
```bash
cargo build --workspace --lib
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Then Sentry verification:
1. Start daemon with Sentry DSN configured
2. Trigger each event type
3. Verify events appear in Sentry UI within 30s

---

## NOT Doing (deferred)

| Item | Reason |
|---|---|
| RBAC enforcement (H9) | Requires middleware refactor + integration test; deferred to separate PR |
| MCP server wiring (H7) | `execute_tool` has 20 params; needs design for external caller context |
| Git tools (H8) | New feature, not a fix; separate feature branch |
| Dead letter queue storage | Sentry IS the DLQ for now — events are searchable/alertable |
| Provider health tick | Requires async probe in kernel loop; separate contract |
