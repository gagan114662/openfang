# Google Home Bridge

Connect Google Home automations to OpenFang using a small HTTP bridge that forwards events into OpenFang webhook trigger endpoints.

This uses:
- `POST /hooks/wake` (event injection)
- `POST /hooks/agent` (direct agent turn)

## 1) Enable webhook triggers in OpenFang

In your OpenFang config (`~/.openfang/config.toml`), add:

```toml
[webhook_triggers]
enabled = true
token_env = "OPENFANG_WEBHOOK_TOKEN"
max_payload_bytes = 65536
rate_limit_per_minute = 30
```

Set the token environment variable (must be 32+ chars):

```bash
export OPENFANG_WEBHOOK_TOKEN='replace-with-32-plus-char-secret-token'
```

Start OpenFang:

```bash
openfang start
```

## 2) Start the Google Home bridge

Run:

```bash
cd /path/to/open_fang
export OPENFANG_BASE_URL='http://127.0.0.1:4200'
export OPENFANG_WEBHOOK_TOKEN='replace-with-same-token-as-above'
export GOOGLE_HOME_BRIDGE_TOKEN='replace-with-another-secret'
export GOOGLE_HOME_BRIDGE_PORT='8787'
python3 scripts/google_home_bridge.py
```

Optional defaults:

```bash
export GOOGLE_HOME_DEFAULT_AGENT='researcher'
export GOOGLE_HOME_DEFAULT_DELIVER='false'
export GOOGLE_HOME_DEFAULT_TIMEOUT_SECS='120'
```

Health check:

```bash
curl -s http://127.0.0.1:8787/healthz
```

## 3) Expose the bridge publicly

Google Home automations need a reachable HTTPS endpoint. Expose the local bridge with your preferred tunnel/reverse proxy (Cloudflare Tunnel, ngrok, etc.).

Example public URL:

```text
https://your-bridge.example.com
```

## 4) Call patterns

All calls should include bridge auth either as:
- `Authorization: Bearer <GOOGLE_HOME_BRIDGE_TOKEN>`, or
- `X-Bridge-Token: <GOOGLE_HOME_BRIDGE_TOKEN>`, or
- query param `?token=<GOOGLE_HOME_BRIDGE_TOKEN>`

### Wake event

```bash
curl -X POST 'https://your-bridge.example.com/google-home/wake' \
  -H 'Authorization: Bearer YOUR_BRIDGE_TOKEN' \
  -H 'Content-Type: application/json' \
  -d '{"text":"doorbell detected motion","mode":"now"}'
```

### Agent turn

```bash
curl -X POST 'https://your-bridge.example.com/google-home/agent' \
  -H 'Authorization: Bearer YOUR_BRIDGE_TOKEN' \
  -H 'Content-Type: application/json' \
  -d '{
    "message":"Turn on office lights and summarize energy usage",
    "agent":"smart-home",
    "deliver":false,
    "timeout_secs":120
  }'
```

### Generic endpoint

```bash
curl -X POST 'https://your-bridge.example.com/google-home' \
  -H 'Authorization: Bearer YOUR_BRIDGE_TOKEN' \
  -H 'Content-Type: application/json' \
  -d '{"action":"wake","text":"night mode start"}'
```

## 5) Google Home automation mapping

Use your Google Home automation path (or intermediary webhook action) to hit:
- `POST /google-home/wake` for event-style triggers
- `POST /google-home/agent` for direct command execution

Suggested mapping:
- Voice phrase: "Start morning ops"
- Bridge request: `POST /google-home/wake` with `{"text":"morning_ops_start","mode":"now"}`
- OpenFang trigger listens for `webhook.wake` events and runs the assigned workflow/agent.

## Security notes

- Keep both secrets private:
  - `OPENFANG_WEBHOOK_TOKEN`
  - `GOOGLE_HOME_BRIDGE_TOKEN`
- Use HTTPS for all public bridge traffic.
- Restrict source IPs in your reverse proxy if possible.
- Rotate tokens periodically.
