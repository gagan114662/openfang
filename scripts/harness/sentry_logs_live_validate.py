#!/usr/bin/env python3
"""Send live OpenFang traffic and verify queryable Sentry Logs rows."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Validate OpenFang canonical Sentry Logs with live traffic"
    )
    parser.add_argument("--org", required=True, help="Sentry organization slug")
    parser.add_argument("--project", required=True, help="Sentry project slug")
    parser.add_argument("--api-base", default="http://127.0.0.1:50051", help="OpenFang daemon base URL")
    parser.add_argument("--sentry-base-url", default="https://sentry.io", help="Sentry base URL")
    parser.add_argument("--token-env", default="SENTRY_AUTH_TOKEN", help="Env var holding Sentry auth token")
    parser.add_argument("--success-agent-id", default="", help="Explicit agent id for success probe")
    parser.add_argument("--failure-agent-id", default="", help="Explicit agent id for failure probe")
    parser.add_argument("--stats-period", default="30m", help="Sentry query window")
    parser.add_argument("--poll-seconds", type=int, default=90, help="Max wait for Sentry ingest")
    parser.add_argument("--poll-interval-seconds", type=int, default=5, help="Poll interval")
    parser.add_argument("--marker-prefix", default="SENTRY_LIVE_VALIDATE", help="Message marker prefix")
    parser.add_argument(
        "--out",
        default="artifacts/sentry-logs-validation.json",
        help="Output artifact path",
    )
    return parser.parse_args()


def _write_json(path: str, payload: Dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _http_json(
    method: str,
    url: str,
    *,
    timeout: int = 30,
    headers: Optional[Dict[str, str]] = None,
    payload: Optional[Dict[str, Any]] = None,
) -> Tuple[int, Dict[str, str], Any]:
    body = None
    req_headers = {"Accept": "application/json"}
    if headers:
        req_headers.update(headers)
    if payload is not None:
        body = json.dumps(payload).encode("utf-8")
        req_headers["Content-Type"] = "application/json"

    req = urllib.request.Request(url, method=method.upper(), headers=req_headers, data=body)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            raw = response.read().decode("utf-8")
            status = int(response.status)
            hdrs = {k.lower(): v for k, v in response.headers.items()}
            try:
                return status, hdrs, json.loads(raw) if raw.strip() else {}
            except json.JSONDecodeError:
                return status, hdrs, raw
    except urllib.error.HTTPError as exc:
        body_text = exc.read().decode("utf-8", errors="replace")
        hdrs = {k.lower(): v for k, v in (exc.headers.items() if exc.headers else [])}
        try:
            parsed = json.loads(body_text) if body_text.strip() else {}
        except json.JSONDecodeError:
            parsed = body_text
        return int(exc.code), hdrs, parsed


def _sentry_query_logs(
    base_url: str,
    org: str,
    token: str,
    query: str,
    stats_period: str,
) -> Tuple[int, Any]:
    params: List[Tuple[str, str]] = [
        ("dataset", "logs"),
        ("query", query),
        ("statsPeriod", stats_period),
        ("sort", "-timestamp"),
        ("per_page", "50"),
        ("field", "timestamp"),
        ("field", "event.kind"),
        ("field", "request.id"),
        ("field", "message"),
        ("field", "http.path"),
        ("field", "http.status_code"),
        ("field", "agent.id"),
        ("field", "agent.name"),
        ("field", "outcome"),
    ]
    endpoint = (
        f"{base_url.rstrip('/')}/api/0/organizations/{org}/events/?"
        f"{urllib.parse.urlencode(params, doseq=True)}"
    )
    status, _, data = _http_json(
        "GET",
        endpoint,
        headers={"Authorization": f"Bearer {token}"},
        timeout=30,
    )
    return status, data


def _wait_for_logs(
    *,
    base_url: str,
    org: str,
    token: str,
    query: str,
    stats_period: str,
    timeout_seconds: int,
    poll_interval_seconds: int,
) -> Dict[str, Any]:
    started = time.time()
    last_status = 0
    last_data: Any = {}
    while time.time() - started <= timeout_seconds:
        status, data = _sentry_query_logs(base_url, org, token, query, stats_period)
        last_status = status
        last_data = data
        if status == 200 and isinstance(data, list) and len(data) > 0:
            return {"ok": True, "status_code": status, "count": len(data), "sample": data[0]}
        if status in (401, 403):
            return {"ok": False, "status_code": status, "error": data}
        time.sleep(max(1, poll_interval_seconds))
    return {"ok": False, "status_code": last_status, "error": last_data}


def _extract_token(token_env: str) -> str:
    return os.environ.get(token_env, "").strip()


def _sentry_scopes(base_url: str, token: str) -> List[str]:
    status, _, data = _http_json(
        "GET",
        f"{base_url.rstrip('/')}/api/0/",
        headers={"Authorization": f"Bearer {token}"},
        timeout=15,
    )
    if status != 200 or not isinstance(data, dict):
        return []
    auth = data.get("auth") or {}
    scopes = auth.get("scopes") or []
    if not isinstance(scopes, list):
        return []
    return [str(s) for s in scopes]


def _daemon_agents(api_base: str) -> List[Dict[str, Any]]:
    status, _, data = _http_json("GET", f"{api_base.rstrip('/')}/api/agents", timeout=20)
    if status != 200 or not isinstance(data, list):
        return []
    return [row for row in data if isinstance(row, dict)]


def _choose_agent(
    agents: List[Dict[str, Any]],
    preferred_id: str,
    provider_hint: Optional[str] = None,
    exclude_id: str = "",
) -> Optional[str]:
    if preferred_id:
        return preferred_id
    if provider_hint:
        for row in agents:
            if str(row.get("id", "")) == exclude_id:
                continue
            if str(row.get("model_provider", "")) == provider_hint:
                return str(row.get("id", ""))
    for row in agents:
        aid = str(row.get("id", ""))
        if aid and aid != exclude_id:
            return aid
    return None


def _send_probe_message(
    *,
    api_base: str,
    agent_id: str,
    marker: str,
) -> Dict[str, Any]:
    sent_at = dt.datetime.now(tz=dt.timezone.utc).isoformat()
    status, headers, body = _http_json(
        "POST",
        f"{api_base.rstrip('/')}/api/agents/{agent_id}/message",
        payload={"message": marker},
        timeout=120,
    )
    request_id = str(headers.get("x-request-id", "")).strip()
    if isinstance(body, dict):
        response_text = body.get("response")
        error_text = body.get("error")
    else:
        response_text = None
        error_text = str(body)
    return {
        "agent_id": agent_id,
        "sent_at": sent_at,
        "marker": marker,
        "http_status": status,
        "request_id": request_id,
        "response": response_text,
        "error": error_text,
    }


def _sentry_logs_link(org: str, query: str) -> str:
    encoded = urllib.parse.quote(query, safe="")
    return f"https://{org}.sentry.io/explore/logs/?query={encoded}"


def main() -> int:
    args = parse_args()
    token = _extract_token(args.token_env)
    timestamp = dt.datetime.now(tz=dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")

    report: Dict[str, Any] = {
        "status": "started",
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "org": args.org,
        "project": args.project,
        "sentry_base_url": args.sentry_base_url,
        "api_base": args.api_base,
        "token_env": args.token_env,
        "checks": {},
        "errors": [],
    }

    if not token:
        report["status"] = "error"
        report["errors"].append(f"missing token in env var: {args.token_env}")
        _write_json(args.out, report)
        return 1

    scopes = _sentry_scopes(args.sentry_base_url, token)
    report["token_scopes"] = scopes
    required_scopes = {"event:read", "project:read"}
    missing_scopes = sorted(required_scopes - set(scopes))
    if missing_scopes:
        report["status"] = "error"
        report["errors"].append(
            "token missing required scope(s): "
            + ", ".join(missing_scopes)
            + " (create a Sentry token with event:read + project:read)"
        )
        _write_json(args.out, report)
        return 1

    health_status, _, health_body = _http_json("GET", f"{args.api_base.rstrip('/')}/api/health", timeout=10)
    report["daemon_health"] = {"status_code": health_status, "body": health_body}
    if health_status != 200:
        report["status"] = "error"
        report["errors"].append("OpenFang daemon is not healthy")
        _write_json(args.out, report)
        return 1

    agents = _daemon_agents(args.api_base)
    report["agent_count"] = len(agents)
    if not agents:
        report["status"] = "error"
        report["errors"].append("no agents available from /api/agents")
        _write_json(args.out, report)
        return 1

    success_agent = _choose_agent(
        agents,
        preferred_id=args.success_agent_id,
        provider_hint="claude-code",
    )
    if not success_agent:
        report["status"] = "error"
        report["errors"].append("could not select success agent")
        _write_json(args.out, report)
        return 1

    failure_agent = _choose_agent(
        agents,
        preferred_id=args.failure_agent_id,
        provider_hint="codex-cli",
        exclude_id=success_agent,
    )

    probes: List[Dict[str, Any]] = []
    success_marker = f"{args.marker_prefix}_{timestamp}_success"
    success_probe = _send_probe_message(
        api_base=args.api_base,
        agent_id=success_agent,
        marker=success_marker,
    )
    probes.append(success_probe)

    if failure_agent:
        failure_marker = f"{args.marker_prefix}_{timestamp}_failure"
        probes.append(
            _send_probe_message(
                api_base=args.api_base,
                agent_id=failure_agent,
                marker=failure_marker,
            )
        )

    report["probes"] = probes
    checks: Dict[str, Any] = {}

    for probe in probes:
        req_id = probe.get("request_id", "")
        marker = probe.get("marker", "")
        label = "failure" if marker.endswith("_failure") else "success"
        if req_id:
            api_query = f"event.kind:api.request request.id:{req_id}"
            checks[f"api_request_{label}"] = {
                "query": api_query,
                "link": _sentry_logs_link(args.org, api_query),
                **_wait_for_logs(
                    base_url=args.sentry_base_url,
                    org=args.org,
                    token=token,
                    query=api_query,
                    stats_period=args.stats_period,
                    timeout_seconds=args.poll_seconds,
                    poll_interval_seconds=args.poll_interval_seconds,
                ),
            }
        runtime_query = (
            f"(event.kind:runtime.agent_loop.completed OR event.kind:runtime.agent_loop.failed) "
            f"payload.input.user_message:*{marker}*"
        )
        checks[f"runtime_loop_{label}"] = {
            "query": runtime_query,
            "link": _sentry_logs_link(args.org, runtime_query),
            **_wait_for_logs(
                base_url=args.sentry_base_url,
                org=args.org,
                token=token,
                query=runtime_query,
                stats_period=args.stats_period,
                timeout_seconds=args.poll_seconds,
                poll_interval_seconds=args.poll_interval_seconds,
            ),
        }

        llm_query = (
            "(event.kind:runtime.llm_call.completed OR event.kind:runtime.llm_call.failed) "
            f"(payload.request.messages.0.content:*{marker}* "
            f"OR payload.request.messages.0.content.0.text:*{marker}* "
            f"OR payload.request.messages.1.content:*{marker}* "
            f"OR payload.request.messages.1.content.0.text:*{marker}*)"
        )
        checks[f"runtime_llm_{label}"] = {
            "query": llm_query,
            "link": _sentry_logs_link(args.org, llm_query),
            **_wait_for_logs(
                base_url=args.sentry_base_url,
                org=args.org,
                token=token,
                query=llm_query,
                stats_period=args.stats_period,
                timeout_seconds=args.poll_seconds,
                poll_interval_seconds=args.poll_interval_seconds,
            ),
        }

    report["checks"] = checks
    required_keys = [
        key for key in checks if key.startswith("api_request_") or key.startswith("runtime_loop_")
    ]
    optional_keys = [key for key in checks if key.startswith("runtime_llm_")]
    required_ok = all(bool(checks[key].get("ok")) for key in required_keys)
    optional_ok = all(bool(checks[key].get("ok")) for key in optional_keys)

    report["required_checks"] = required_keys
    report["optional_checks"] = optional_keys
    report["status"] = "success" if required_ok and optional_ok else "partial"
    if not required_ok:
        report["errors"].append("required Sentry log checks did not return rows in time window")
    elif not optional_ok:
        report["errors"].append("optional runtime.llm_call checks did not return rows in time window")

    _write_json(args.out, report)
    return 0 if required_ok else 1


if __name__ == "__main__":
    sys.exit(main())
