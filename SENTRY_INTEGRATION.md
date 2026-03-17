# Sentry AI Monitoring - Complete Integration

## ✅ **Fully Integrated with OpenFang**

OpenFang includes **production-grade Sentry integration** for comprehensive AI agent monitoring and observability.

---

## 🎯 **What's Monitored**

### 1. **Error Tracking**
- Runtime exceptions
- Agent loop failures
- Tool execution errors
- API request failures
- Channel adapter errors
- Memory substrate issues
- Network errors (OFP/A2A)

### 2. **Performance Monitoring**
- Transaction tracking
- Span instrumentation
- Latency metrics
- Configurable sample rate (0.0-1.0)
- Full distributed tracing

### 3. **Cost Anomaly Detection**
- **High-cost agent loops:** Automatic alert when single loop exceeds $10
- Cost attribution per agent
- Budget quota violations
- Token usage spikes

### 4. **Custom Event Tracking**
With tags for:
- Agent ID
- Agent name
- Model used
- Provider
- Tool executions
- Channel activity

---

## 📋 **Configuration**

### Config File (`~/.openfang/config.toml`)

```toml
[sentry]
dsn = "https://your-key@o123456.ingest.sentry.io/789012"
environment = "production"           # or "staging", "development"
traces_sample_rate = 1.0            # 1.0 = 100%, 0.1 = 10%
include_prompts = false             # false = privacy-safe (recommended)
error_tracking = true               # Enable error capture
performance_monitoring = true       # Enable performance traces
attach_stacktrace = false           # Recommended for canonical wide events
enable_logs = true                  # Enable canonical wide-event capture
wide_event_attribute_max_bytes = 16384
wide_event_payload_max_bytes = 524288
claude_capture_payloads = true
mcp_capture_payloads = true

# Custom tags (optional)
[sentry.tags]
deployment = "us-west-2"
team = "ai-ops"
version = "v1.2.3"
```

### Environment Variable Alternative

```bash
export SENTRY_DSN="https://your-key@o123456.ingest.sentry.io/789012"
export SENTRY_ENVIRONMENT="production"
```

---

## 🔧 **Implementation Details**

### Initialization
```rust
// kernel.rs:493-527
fn initialize_sentry(config: &KernelConfig) -> Option<sentry::ClientInitGuard> {
    let dsn = config.sentry.dsn.as_ref()?;

    let guard = sentry::init((
        dsn.clone(),
        sentry::ClientOptions {
            release: Some(env!("CARGO_PKG_VERSION").into()),
            environment: Some(config.sentry.environment.clone().into()),
            traces_sample_rate: config.sentry.traces_sample_rate,
            send_default_pii: config.sentry.include_prompts,
            attach_stacktrace: config.sentry.attach_stacktrace,
            ..Default::default()
        },
    ));

    // Set custom tags
    sentry::configure_scope(|scope| {
        for (key, value) in &config.sentry.tags {
            scope.set_tag(key, value);
        }
    });

    Some(guard)  // Keep alive for process lifetime
}
```

### Cost Anomaly Alerts
```rust
// kernel.rs:2151-2157
if cost > 10.0 {
    sentry::capture_message(
        &format!("High cost agent loop: ${:.2} for agent {}", cost, agent_id),
        sentry::Level::Warning,
    );
}
```

### Automatic Context
Every Sentry event includes:
- **Release:** Package version (`CARGO_PKG_VERSION`)
- **Environment:** Production/staging/dev
- **Tags:** Custom tags from config
- **Stacktrace:** Full Rust backtrace
- **User context:** Agent ID (if privacy-safe mode)

---

## 📊 **What You See in Sentry Dashboard**

### Issues Tab
- **Error grouping** by type
- **Stack traces** with source file references
- **Breadcrumbs** showing events leading to error
- **Agent context:** Which agent caused the error
- **Model context:** Which LLM/provider was used

### Performance Tab
- **Transaction overview:** API endpoints, agent loops
- **Span waterfall:** Tool execution timeline
- **Latency percentiles:** p50, p75, p95, p99
- **Throughput metrics:** Requests per minute
- **Database queries:** Memory substrate performance

### Alerts & Notifications
- **Cost spike alerts:** When agent loops exceed $10
- **Error rate alerts:** Configurable thresholds
- **Performance degradation:** Slow transaction alerts
- **Custom alerts:** Based on tags, environment, etc.

### Dashboards
- **Agent health:** Per-agent error rates
- **Model performance:** Latency by provider
- **Channel activity:** Message throughput per platform
- **Cost tracking:** Real-time spend monitoring

---

## 🛡️ **Privacy & Security**

## Native Debug Files

OpenFang now defaults `attach_stacktrace = false` for Sentry message events. This keeps canonical
wide events such as `api.request` and `runtime.agent_loop.completed` from showing Sentry
`Processing Error` warnings when local debug files have not been uploaded yet.

If you want native stacktrace symbolication for local or release binaries, upload the matching
debug files explicitly:

```bash
./scripts/upload_sentry_debug_files.sh target/debug/openfang
```

The helper resolves `SENTRY_AUTH_TOKEN` from the environment first, then falls back to
`sentry.auth_token` in `~/.openfang/config.toml`. Override org/project with `SENTRY_ORG` and
`SENTRY_PROJECT` when needed.

### Privacy-Safe Mode (Recommended)
```toml
include_prompts = false  # Do NOT send prompts/completions to Sentry
```

**When enabled:**
- ✅ Error messages captured
- ✅ Stack traces included
- ✅ Performance metrics tracked
- ✅ Agent IDs logged
- ❌ User prompts excluded
- ❌ LLM completions excluded
- ❌ Sensitive data excluded

### Development Mode
```toml
include_prompts = true  # Include prompts for debugging
```

**Only use in:**
- Local development
- Staging environments
- Non-production testing

**Never in production** (PII/privacy risk)

---

## 📈 **Sampling Strategy**

### Production
```toml
traces_sample_rate = 0.1  # 10% sampling
```
- Reduces Sentry quota usage
- Still catches most issues
- Cost-effective for high-traffic systems

### Staging
```toml
traces_sample_rate = 1.0  # 100% sampling
```
- Full visibility for testing
- Catch all edge cases

### Local Development
```toml
dsn = null  # Sentry disabled
```
- No network overhead
- Faster development iteration

---

## 🎛️ **What Gets Tracked**

### Automatically (via sentry-tracing)
All `tracing` logs at ERROR and WARN levels:
```rust
error!("Failed to connect to IMAP server: {}", e);  // → Sentry issue
warn!("Local provider offline");                    // → Sentry warning
```

### Manually Captured Events

**1. High-Cost Alerts:**
```rust
// kernel.rs:2153
if cost > 10.0 {
    sentry::capture_message("High cost agent loop: $12.45", Level::Warning);
}
```

**2. Context Tags:**
```rust
sentry::configure_scope(|scope| {
    scope.set_tag("agent_id", agent_id);
    scope.set_tag("model", "gemini-2.0-flash-exp");
    scope.set_tag("provider", "gemini");
    scope.set_tag("channel", "telegram");
});
```

**3. Breadcrumbs:**
- Agent spawn events
- Tool executions
- Channel messages
- API requests
- Workflow steps

---

## 🔌 **Dependencies**

```toml
# Cargo.toml
[dependencies]
sentry = "0.34"
sentry-tracing = "0.34"
```

**Enabled in:**
- `openfang-kernel` (main integration)
- `openfang-api` (canonical API request logs + local telemetry ingestion)
- `openfang-cli` (`openfang mcp` tool-call visibility for Claude Desktop)
- `openfang-desktop` (desktop lifecycle and notification forwarding)

## Canonical Event Families

The repo now standardizes these searchable event families:

- `api.request`
- `runtime.llm_call.completed`
- `runtime.llm_call.failed`
- `runtime.agent_loop.completed`
- `runtime.agent_loop.failed`
- `claude.session.*`
- `claude.task.*`
- `claude.prompt.submitted`
- `mcp.tool_call.*`
- `desktop.lifecycle.*`
- `ops.guard.*`
- `ops.triage.*`
- `ops.deploy.*`
- `auth.*`
- `openfang-runtime` (agent loop monitoring)

---

## 📝 **Configuration Reference**

### Full Config Options

```toml
[sentry]
# Required: Your Sentry project DSN
dsn = "https://abc123@o456.ingest.sentry.io/789"

# Environment tag (appears in Sentry UI)
environment = "production"          # Default: "production"

# Sample rate for performance traces (0.0-1.0)
traces_sample_rate = 1.0           # Default: 1.0 (100%)

# Include prompts/completions in events
include_prompts = false            # Default: false (privacy-safe)

# Enable error tracking
error_tracking = true              # Default: true

# Enable performance monitoring
performance_monitoring = true      # Default: true

# Custom tags (appear on all events)
[sentry.tags]
region = "us-west-2"
cluster = "prod-01"
version = "v2.1.0"
team = "ai-platform"
```

### Environment Variables (Alternative)

```bash
# Override config via env vars
export SENTRY_DSN="https://..."
export SENTRY_ENVIRONMENT="staging"
export SENTRY_SAMPLE_RATE="0.5"
```

---

## 🎯 **Use Cases**

### 1. **Production Monitoring**
- Real-time error alerts
- Performance degradation detection
- Cost anomaly alerts
- Agent health tracking

### 2. **Debugging**
- Full stack traces with line numbers
- Breadcrumb trail showing what led to error
- Request context (API calls, agent messages)
- Reproducible error conditions

### 3. **Performance Optimization**
- Identify slow agent loops
- Find bottlenecks in tool execution
- Database query performance
- LLM API latency tracking

### 4. **Cost Control**
- Alert on expensive agent loops (>$10)
- Track which agents consume most budget
- Identify model inefficiencies
- Prevent runaway costs

### 5. **Compliance & Audit**
- Error rate trending
- Incident response tracking
- Deployment impact analysis
- Service level monitoring

---

## 🚨 **Alert Examples**

### Cost Spike Alert
```
🚨 High cost agent loop: $12.45 for agent 91c7030f
Agent: researcher
Model: gpt-4-32k
Tokens: 28,432 input, 3,567 output
Time: 2026-02-27 18:15:23 UTC
```

### Error Alert
```
❌ Failed to connect to IMAP server
Error: Connection refused (os error 61)
Agent: email-test-bot
Channel: email
Stack trace: [full trace...]
```

### Performance Alert
```
⚠️ Slow transaction detected
Endpoint: POST /api/agents/{id}/message
Duration: 45.2s (p95: 2.1s)
Agent: gpt-4-research-bot
Model: gpt-4-turbo
```

---

## 💡 **Best Practices**

### Production Setup
1. **Set environment:** `production`
2. **Sample traces:** 10-20% (`traces_sample_rate = 0.1`)
3. **Disable PII:** `include_prompts = false`
4. **Enable both:** `error_tracking = true`, `performance_monitoring = true`
5. **Add tags:** Region, cluster, version

### Staging Setup
1. **Set environment:** `staging`
2. **Full sampling:** `traces_sample_rate = 1.0`
3. **Enable prompts:** `include_prompts = true` (for debugging)
4. **Alert thresholds:** Lower than production

### Development Setup
1. **Disable Sentry:** `dsn = null`
2. Or use local Sentry instance
3. Full sampling for testing

---

## 📦 **Integration Status**

| Component | Sentry Integration | Status |
|-----------|-------------------|--------|
| Kernel | ✅ Full | Error + performance + cost alerts |
| API Server | ✅ Full | Request tracking + errors |
| Runtime | ✅ Full | Agent loop monitoring |
| Channels | ✅ Automatic | Via tracing logs |
| Memory | ✅ Automatic | Via tracing logs |
| Tools | ✅ Automatic | Via tracing logs |
| RLM | ✅ Automatic | Via tracing logs |

---

## 🔍 **Example Sentry Query**

### Find High-Cost Agents
```
environment:production
AND message:"High cost agent loop"
AND cost:>10
```

### Find Specific Agent Errors
```
tags.agent_id:91c7030f-9100-44d5-b4d6-63df41d40494
AND level:error
```

### Performance Issues
```
transaction.op:"agent.loop"
AND transaction.duration:>30s
```

---

## 🎉 **Summary**

**OpenFang + Sentry = Complete Observability**

✅ **Error tracking** - Every exception captured
✅ **Performance monitoring** - Full distributed tracing
✅ **Cost alerts** - Prevent runaway spending
✅ **Agent context** - Know exactly which agent failed
✅ **Privacy-safe** - PII exclusion built-in
✅ **Production-ready** - Used in live deployments

**Already configured in your codebase:** Just add your Sentry DSN to config!

---

**Current Status:** Sentry integration is **active and production-ready** ✨

**From logs:**
```
INFO openfang_kernel::kernel: Sentry AI Monitoring initialized
  environment=production
  sample_rate=1
```

Your agents are being monitored right now! 🎯
