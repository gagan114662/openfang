#!/usr/bin/env python3
"""Probe Paperclip APIs and emit one canonical visibility event to Sentry."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import tomllib
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path
from typing import Any

from sentry_client import send_sentry_event


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Probe Paperclip and emit a canonical Sentry event")
    parser.add_argument(
        "--paperclip-base",
        default=os.getenv("PAPERCLIP_API_URL", "http://localhost:3100"),
        help="Paperclip base URL without /api suffix",
    )
    parser.add_argument(
        "--company-id",
        default=os.getenv("PAPERCLIP_COMPANY_ID", ""),
        help="Paperclip company ID for company-scoped endpoints",
    )
    parser.add_argument(
        "--api-key-env",
        default="PAPERCLIP_API_KEY",
        help="Environment variable holding the Paperclip bearer token",
    )
    parser.add_argument(
        "--emit",
        default="true",
        choices=["true", "false"],
        help="Whether to emit to Sentry",
    )
    parser.add_argument(
        "--sentry-dsn-env",
        default="SENTRY_DSN",
        help="Environment variable holding the Sentry DSN",
    )
    parser.add_argument(
        "--openfang-config",
        default=str(Path.home() / ".openfang" / "config.toml"),
        help="OpenFang config path used as Sentry DSN fallback",
    )
    parser.add_argument(
        "--out",
        default="artifacts/paperclip/paperclip-probe-latest.json",
        help="Artifact path for probe results",
    )
    return parser.parse_args()


def now_utc() -> str:
    return dt.datetime.now(tz=dt.timezone.utc).isoformat()


def _request_json(
    url: str,
    *,
    bearer_token: str | None = None,
    timeout: int = 20,
) -> tuple[bool, dict[str, Any]]:
    headers = {"Accept": "application/json"}
    if bearer_token:
        headers["Authorization"] = f"Bearer {bearer_token}"
    req = urllib.request.Request(url, headers=headers, method="GET")
    started = dt.datetime.now(tz=dt.timezone.utc)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            body = resp.read().decode("utf-8", errors="replace")
            payload: Any = {}
            try:
                payload = json.loads(body) if body.strip() else {}
            except json.JSONDecodeError:
                payload = {"raw": body[:4000]}
            duration_ms = int((dt.datetime.now(tz=dt.timezone.utc) - started).total_seconds() * 1000)
            return True, {
                "url": url,
                "status": int(resp.status),
                "duration_ms": duration_ms,
                "payload": payload,
            }
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        duration_ms = int((dt.datetime.now(tz=dt.timezone.utc) - started).total_seconds() * 1000)
        return False, {
            "url": url,
            "status": int(exc.code),
            "duration_ms": duration_ms,
            "error": body[:4000] or f"http_error={exc.code}",
        }
    except urllib.error.URLError as exc:
        duration_ms = int((dt.datetime.now(tz=dt.timezone.utc) - started).total_seconds() * 1000)
        return False, {
            "url": url,
            "status": 0,
            "duration_ms": duration_ms,
            "error": f"url_error={exc.reason}",
        }


def _count(payload: Any) -> int | None:
    if isinstance(payload, list):
        return len(payload)
    if isinstance(payload, dict):
        for key in ("items", "data", "results", "events", "issues"):
            value = payload.get(key)
            if isinstance(value, list):
                return len(value)
    return None


def _write_json(path: str, payload: dict[str, Any]) -> None:
    out = Path(path)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def build_event(report: dict[str, Any]) -> dict[str, Any]:
    kind = "paperclip.probe.completed" if report["status"] == "ok" else "paperclip.probe.failed"
    return {
        "event_id": uuid.uuid4().hex,
        "timestamp": now_utc(),
        "level": "info" if report["status"] == "ok" else "error",
        "message": report["summary"],
        "transaction": "paperclip.probe.cycle",
        "tags": {
            "event.kind": kind,
            "integration": "paperclip",
            "service": "openfang",
            "paperclip.host": report["paperclip_base"],
            "paperclip.company_id": report["company_id"] or "none",
        },
        "extra": {
            "status": report["status"],
            "company_id": report["company_id"],
            "checked_endpoints": report["checked_endpoints"],
            "counts": report["counts"],
            "failures": report["failures"],
            "request_ids": report["request_ids"],
            "generated_at": report["generated_at"],
        },
    }


def read_openfang_sentry_dsn(config_path: str) -> str:
    path = Path(config_path)
    if not path.exists():
        return ""
    try:
        payload = tomllib.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return ""
    sentry = payload.get("sentry")
    if isinstance(sentry, dict):
        dsn = sentry.get("dsn")
        if isinstance(dsn, str):
            return dsn.strip()
    return ""


def main() -> int:
    args = parse_args()
    base = args.paperclip_base.rstrip("/")
    api_base = f"{base}/api"
    token = os.getenv(args.api_key_env, "").strip() or None
    request_id = uuid.uuid4().hex

    checks: list[tuple[str, str, bool]] = [
        ("health", f"{api_base}/health", False),
    ]
    if args.company_id:
        checks.extend(
            [
                ("issues", f"{api_base}/companies/{args.company_id}/issues?status=todo,in_progress,blocked", True),
                ("activity", f"{api_base}/companies/{args.company_id}/activity", True),
                ("heartbeats", f"{api_base}/companies/{args.company_id}/heartbeat-runs?limit=10", True),
                ("approvals", f"{api_base}/companies/{args.company_id}/approvals?status=pending", True),
                ("scheduler_heartbeats", f"{api_base}/instance/scheduler-heartbeats", True),
            ]
        )

    report: dict[str, Any] = {
        "generated_at": now_utc(),
        "paperclip_base": base,
        "company_id": args.company_id or None,
        "request_ids": {"probe_id": request_id},
        "checked_endpoints": {},
        "counts": {},
        "failures": {},
        "status": "ok",
        "summary": "Paperclip probe completed successfully.",
    }

    if args.company_id and not token:
        report["status"] = "error"
        report["summary"] = f"Paperclip probe blocked: missing {args.api_key_env}."
        report["failures"]["auth"] = {"error": f"missing env var {args.api_key_env}"}
        _write_json(args.out, report)
        if args.emit == "true":
            dsn = os.getenv(args.sentry_dsn_env, "").strip()
            if dsn:
                send_sentry_event(dsn, build_event(report))
        return 1

    for name, url, requires_auth in checks:
        ok, result = _request_json(url, bearer_token=token if requires_auth else None)
        report["checked_endpoints"][name] = {
            "ok": ok,
            "status": result.get("status"),
            "duration_ms": result.get("duration_ms"),
            "url": url,
        }
        if ok:
            count = _count(result.get("payload"))
            if count is not None:
                report["counts"][name] = count
        else:
            report["status"] = "error"
            report["failures"][name] = {
                "status": result.get("status"),
                "error": result.get("error"),
            }

    if report["status"] != "ok":
        failed_names = ", ".join(sorted(report["failures"].keys()))
        report["summary"] = f"Paperclip probe found failures in: {failed_names}."
    else:
        counts = ", ".join(f"{k}={v}" for k, v in sorted(report["counts"].items()))
        report["summary"] = f"Paperclip probe completed successfully. {counts}".strip()

    _write_json(args.out, report)

    if args.emit == "true":
        dsn = os.getenv(args.sentry_dsn_env, "").strip() or read_openfang_sentry_dsn(args.openfang_config)
        if dsn:
            sent, detail, status = send_sentry_event(dsn, build_event(report))
            report["sentry"] = {"sent": sent, "detail": detail, "status": status}
            _write_json(args.out, report)
        else:
            report["sentry"] = {
                "sent": False,
                "detail": f"missing {args.sentry_dsn_env} and no sentry.dsn in {args.openfang_config}",
                "status": None,
            }
            _write_json(args.out, report)

    return 0 if report["status"] == "ok" else 1


if __name__ == "__main__":
    raise SystemExit(main())
