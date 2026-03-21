# OpenFang Monitoring Stack

Grafana + Loki + Promtail overlay for querying OpenFang structured JSON logs.

## Architecture

```
openfang (4200) ──► JSON logs ──► /data/logs/ (shared volume)
promtail         ──► scrapes /data/logs/   ──► pushes to loki:3100
grafana  (3000)  ──► queries loki:3100     ──► renders dashboard
```

## Quick Start

```bash
# Start everything (add --build on first run)
bash scripts/docker/monitoring-up.sh -d --build

# Or manually:
docker compose -f docker-compose.yml -f docker-compose.monitoring.yml up -d --build
```

## Service URLs

| Service   | URL                     | Credentials        |
|-----------|-------------------------|---------------------|
| OpenFang  | http://localhost:4200    | —                   |
| Grafana   | http://localhost:3000    | admin / openfang    |
| Loki      | http://localhost:3100    | —                   |

Anonymous Grafana access is enabled with Viewer role.

## Dashboard Panels

The pre-provisioned **OpenFang Agent Monitor** dashboard includes:

| Panel            | Description                                          |
|------------------|------------------------------------------------------|
| Active Agents    | Count of unique agents seen in the selected range    |
| Agent Log Volume | Time series of log lines per agent                   |
| Errors           | Count of ERROR-level logs (turns red when > 0)       |
| Recent Logs      | Live log stream with JSON detail expansion           |
| Warnings & Errors| Table filtered to WARN and ERROR entries             |

Use the `agent` dropdown at the top to filter by specific agents.

## Useful LogQL Queries

```logql
# All logs from a specific agent
{job="openfang", agent="my-agent"} | json

# Errors in the last hour
{job="openfang"} | json | level = "ERROR"

# Search by message content
{job="openfang"} | json | line_format "{{.message}}" |= "timeout"

# Count errors per agent
sum by (agent) (count_over_time({job="openfang"} | json | level = "ERROR" [1h]))
```

## Customization

### Grafana credentials

Set in `.env` or environment:
```bash
GF_ADMIN_USER=admin
GF_ADMIN_PASSWORD=your-password
```

### Log retention

Edit `monitoring/loki/loki-config.yml` — `reject_old_samples_max_age` (default: 7 days).

### Log directory

Edit `monitoring/openfang-monitoring.toml` — `json_dir` must match the Promtail scrape path in `monitoring/promtail/promtail-config.yml`.

## Cleanup

```bash
docker compose -f docker-compose.yml -f docker-compose.monitoring.yml down -v
```

The `-v` flag removes the `loki-data` and `grafana-data` volumes.
