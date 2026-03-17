# OpenFang Control Plane (Elixir)

Remote-first control plane for OpenFang:
- Telegram commands (`/agents`, `/run`, `/status`, `/stop`, `/logs`, `/help`)
- OpenFang WS v1 streaming (`/ws`, subprotocol `of-ws.v1.json`)
- SQLite job store (Ecto)
- Wide-event JSON logs to stdout (systemd/journald friendly)
- Optional Sentry crash reporting

This project is designed to run on a remote Linux host (e.g. `192.168.40.234`)
in a Docker container managed by systemd, so your Mac stays mostly idle.

## Env Vars

Required:
- `TELEGRAM_BOT_TOKEN`
- `TELEGRAM_ALLOWED_USERS` (comma-separated Telegram user IDs; empty = allow all)
- `OPENFANG_HTTP_BASE` (default: `http://127.0.0.1:50051`)
- `OPENFANG_WS_BASE` (default: `ws://127.0.0.1:50051`)

Optional:
- `OPENFANG_API_KEY` (if OpenFang is configured with an API key)
- `SENTRY_DSN`
- `OFCP_DB_PATH` (default: `./data/ofcp.sqlite3`)
- `OFCP_LOG_LEVEL` (default: `info`)

## How It Works

Each `/run` starts a Job row in SQLite, opens a WS connection to OpenFang `/ws`,
starts `agent.run`, and edits a single Telegram message as deltas arrive.

## Local dev (optional)

If you have Elixir locally:
```bash
mix deps.get
mix ecto.create
mix ecto.migrate
mix run --no-halt
```

