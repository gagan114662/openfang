#!/usr/bin/env python3
"""Unattended vacation guard for the live OpenFang deployment."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import subprocess
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Tuple


DEFAULT_GUARD_DIR = Path.home() / ".openfang" / "artifacts" / "vacation-guard"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Check live OpenFang health and poller ownership")
    parser.add_argument("--api-base", default="http://127.0.0.1:50051", help="Local OpenFang API base URL")
    parser.add_argument("--heartbeat-path", default="/api/status", help="Cheap endpoint that should emit api.request logs")
    parser.add_argument(
        "--guard-report-path",
        default="/api/ops/guard/report",
        help="Endpoint that accepts canonical ops.guard.* reports",
    )
    parser.add_argument(
        "--host-role",
        default=os.getenv("OPENFANG_HOST_ROLE", "primary"),
        help="Host role reported into Sentry ops logs",
    )
    parser.add_argument(
        "--primary-host",
        default=os.getenv("OPENFANG_PRIMARY_HOST", "gpu"),
        help="Intended primary unattended runtime host label",
    )
    parser.add_argument(
        "--telegram-owner",
        default=os.getenv("OPENFANG_TELEGRAM_OWNER", "gpu"),
        help="Host label that should own Telegram polling",
    )
    parser.add_argument(
        "--primary-sentry-browser-host",
        default=os.getenv("OPENFANG_PRIMARY_SENTRY_BROWSER_HOST", "gpu"),
        help="Host label for the primary Sentry browser profile",
    )
    parser.add_argument(
        "--fallback-sentry-browser-host",
        default=os.getenv("OPENFANG_FALLBACK_SENTRY_BROWSER_HOST", "mac"),
        help="Host label for the fallback Sentry browser profile",
    )
    parser.add_argument(
        "--remote-host",
        default=os.getenv("REMOTE_HOST", "gagan-arora@192.168.40.234"),
        help="Remote SSH target that hosts the GPU machine",
    )
    parser.add_argument("--ssh-timeout-secs", type=int, default=8, help="SSH connect timeout")
    parser.add_argument(
        "--out",
        default=str(DEFAULT_GUARD_DIR / "latest.json"),
        help="Latest report path",
    )
    parser.add_argument(
        "--history-dir",
        default=str(DEFAULT_GUARD_DIR / "history"),
        help="Directory for timestamped report history",
    )
    parser.add_argument(
        "--local-log-path",
        default=str(Path.home() / ".openfang" / "daemon.log"),
        help="Local daemon log path",
    )
    parser.add_argument("--tail-lines", type=int, default=30, help="Log tail lines to include on failure")
    parser.add_argument(
        "--remote-openfang-service",
        default="openfang.service",
        help="Remote primary daemon service name",
    )
    parser.add_argument(
        "--remote-control-plane-service",
        default="openfang_control_plane.service",
        help="Remote control-plane service name",
    )
    parser.add_argument(
        "--enforce-single-poller",
        action="store_true",
        help="Stop the remote control plane when it is active",
    )
    return parser.parse_args()


def _write_json(path: Path, payload: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _tail(path: Path, max_lines: int) -> List[str]:
    if max_lines <= 0 or not path.exists():
        return []
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []
    return lines[-max_lines:]


def _http_json(url: str, timeout: int = 15) -> Tuple[int, Dict[str, str], Any]:
    req = urllib.request.Request(url, method="GET", headers={"Accept": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            raw = response.read().decode("utf-8", errors="replace")
            headers = {k.lower(): v for k, v in response.headers.items()}
            try:
                body: Any = json.loads(raw) if raw.strip() else {}
            except json.JSONDecodeError:
                body = raw
            return int(response.status), headers, body
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        headers = {k.lower(): v for k, v in (exc.headers.items() if exc.headers else [])}
        try:
            body = json.loads(raw) if raw.strip() else {}
        except json.JSONDecodeError:
            body = raw
        return int(exc.code), headers, body
    except Exception as exc:
        return 0, {}, {"error": str(exc)}


def _post_json(url: str, payload: Dict[str, Any], timeout: int = 15) -> Tuple[int, Dict[str, str], Any]:
    body = json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        method="POST",
        data=body,
        headers={"Accept": "application/json", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            raw = response.read().decode("utf-8", errors="replace")
            headers = {k.lower(): v for k, v in response.headers.items()}
            try:
                parsed: Any = json.loads(raw) if raw.strip() else {}
            except json.JSONDecodeError:
                parsed = raw
            return int(response.status), headers, parsed
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        headers = {k.lower(): v for k, v in (exc.headers.items() if exc.headers else [])}
        try:
            parsed = json.loads(raw) if raw.strip() else {}
        except json.JSONDecodeError:
            parsed = raw
        return int(exc.code), headers, parsed
    except Exception as exc:
        return 0, {}, {"error": str(exc)}


def _post_guard_report(
    api_base: str,
    preferred_path: str,
    payload: Dict[str, Any],
    timeout: int = 15,
) -> Tuple[int, Dict[str, str], Any, str, List[Dict[str, Any]]]:
    paths: List[str] = []
    for candidate in (preferred_path, "/api/ops/guard/report", "/ops/guard/report"):
        normalized = candidate if candidate.startswith("/") else f"/{candidate}"
        if normalized not in paths:
            paths.append(normalized)

    last_status = 0
    last_headers: Dict[str, str] = {}
    last_body: Any = {}
    last_path = paths[0] if paths else preferred_path
    tried: List[Dict[str, Any]] = []
    for idx, path in enumerate(paths):
        status, headers, body = _post_json(f"{api_base.rstrip('/')}{path}", payload, timeout=timeout)
        tried.append({"path": path, "status_code": status})
        last_status, last_headers, last_body, last_path = status, headers, body, path
        if 200 <= status < 300:
            break
        # Only fall back for route-not-found style failures.
        if status not in {0, 404, 405}:
            break
        if idx == len(paths) - 1:
            break
    return last_status, last_headers, last_body, last_path, tried


def _run_cmd(cmd: List[str], timeout: int = 15) -> Tuple[int, str, str]:
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        return 124, exc.stdout or "", exc.stderr or "command timed out"
    except Exception as exc:
        return 1, "", str(exc)
    return proc.returncode, proc.stdout, proc.stderr


def _local_pid() -> str:
    rc, stdout, _ = _run_cmd(
        ["bash", "-lc", "pgrep -f 'target/debug/openfang start|./target/debug/openfang start|openfang start' | head -n 1"],
        timeout=5,
    )
    if rc not in (0, 1):
        return ""
    return stdout.strip()


def _ssh(remote_host: str, ssh_timeout_secs: int, command: str) -> Tuple[int, str, str]:
    if remote_host in {"", "local", "localhost", "127.0.0.1", "self"}:
        return _run_cmd(["bash", "-lc", command], timeout=max(ssh_timeout_secs + 4, 10))
    return _run_cmd(
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            f"ConnectTimeout={ssh_timeout_secs}",
            remote_host,
            command,
        ],
        timeout=max(ssh_timeout_secs + 4, 10),
    )


def _remote_service_state(remote_host: str, ssh_timeout_secs: int, service: str) -> Dict[str, Any]:
    rc, stdout, stderr = _ssh(
        remote_host,
        ssh_timeout_secs,
        f"systemctl --user is-active {service} 2>/dev/null || true",
    )
    state = stdout.strip() if stdout.strip() else "unknown"
    return {"service": service, "state": state, "rc": rc, "stderr": stderr.strip()}


def _remote_telegram_socket_count(remote_host: str, ssh_timeout_secs: int) -> Dict[str, Any]:
    rc, stdout, stderr = _ssh(
        remote_host,
        ssh_timeout_secs,
        r"ss -tpn | grep -Ec '149\.154|91\.108' || true",
    )
    count = -1
    try:
        count = int(stdout.strip())
    except ValueError:
        pass
    return {"count": count, "rc": rc, "stderr": stderr.strip()}


def _stop_remote_service(remote_host: str, ssh_timeout_secs: int, service: str) -> Dict[str, Any]:
    rc, stdout, stderr = _ssh(
        remote_host,
        ssh_timeout_secs,
        f"systemctl --user stop {service}",
    )
    return {"service": service, "rc": rc, "stdout": stdout.strip(), "stderr": stderr.strip()}


def _restart_remote_service(remote_host: str, ssh_timeout_secs: int, service: str) -> Dict[str, Any]:
    rc, stdout, stderr = _ssh(
        remote_host,
        ssh_timeout_secs,
        f"systemctl --user restart {service}",
    )
    return {"service": service, "rc": rc, "stdout": stdout.strip(), "stderr": stderr.strip()}


def _artifact_paths(out_arg: str, history_arg: str, stamp: str) -> Tuple[Path, Path]:
    out_path = Path(out_arg)
    history_dir = Path(history_arg)
    if not out_path.is_absolute():
        out_path = Path.cwd() / out_path
    if not history_dir.is_absolute():
        history_dir = Path.cwd() / history_dir
    return out_path, history_dir / f"{stamp}.json"


def main() -> int:
    args = parse_args()
    now = dt.datetime.now(tz=dt.timezone.utc)
    stamp = now.strftime("%Y%m%dT%H%M%SZ")
    out_path, history_path = _artifact_paths(args.out, args.history_dir, stamp)

    report: Dict[str, Any] = {
        "generated_at": now.isoformat(),
        "status": "fail",
        "errors": [],
        "actions": [],
        "local": {},
        "remote": {},
        "sentry": {},
    }

    pid = _local_pid()
    heartbeat_url = f"{args.api_base.rstrip('/')}{args.heartbeat_path}"
    health_url = f"{args.api_base.rstrip('/')}/api/health"

    health_status, _, health_body = _http_json(health_url)
    heartbeat_status, heartbeat_headers, heartbeat_body = _http_json(heartbeat_url)
    request_id = str(heartbeat_headers.get("x-request-id", "")).strip()

    agent_count: Optional[int] = None
    if isinstance(heartbeat_body, dict):
        raw_count = heartbeat_body.get("agent_count")
        if isinstance(raw_count, int):
            agent_count = raw_count

    report["local"] = {
        "pid": pid or None,
        "health_url": health_url,
        "health_status_code": health_status,
        "health_body": health_body,
        "heartbeat_url": heartbeat_url,
        "heartbeat_status_code": heartbeat_status,
        "heartbeat_request_id": request_id or None,
        "agent_count": agent_count,
    }
    report["sentry"] = {
        "expected_query": f"event.kind:api.request request.id:{request_id}" if request_id else None,
        "notes": [
            "heartbeat endpoint emits canonical api.request logs",
            "Telegram-originated activity should be searched under runtime.* event kinds",
            "runtime user content query shape: payload.input.user_message:*<marker>*",
        ],
    }

    if not pid:
        report["errors"].append("local daemon process not found")
    if health_status != 200:
        report["errors"].append(f"local health check failed status={health_status}")
    if heartbeat_status != 200:
        report["errors"].append(f"local heartbeat request failed status={heartbeat_status}")

    openfang_state = _remote_service_state(args.remote_host, args.ssh_timeout_secs, args.remote_openfang_service)
    control_plane_state = _remote_service_state(
        args.remote_host, args.ssh_timeout_secs, args.remote_control_plane_service
    )
    socket_count = _remote_telegram_socket_count(args.remote_host, args.ssh_timeout_secs)

    report["remote"] = {
        "host": args.remote_host,
        "openfang_service": openfang_state,
        "control_plane_service": control_plane_state,
        "telegram_socket_count": socket_count,
    }

    if openfang_state["state"] not in {"active", "activating"}:
        report["errors"].append(
            f"remote {args.remote_openfang_service} is not active (state={openfang_state['state']})"
        )
        report["actions"].append(
            {
                "type": "restart_remote_openfang",
                "result": _restart_remote_service(
                    args.remote_host,
                    args.ssh_timeout_secs,
                    args.remote_openfang_service,
                ),
            }
        )

    if control_plane_state["state"] == "active":
        if args.enforce_single_poller:
            stop_result = _stop_remote_service(
                args.remote_host,
                args.ssh_timeout_secs,
                args.remote_control_plane_service,
            )
            report["actions"].append(
                {
                    "type": "stop_remote_control_plane",
                    "result": stop_result,
                }
            )
            control_plane_state = _remote_service_state(
                args.remote_host, args.ssh_timeout_secs, args.remote_control_plane_service
            )
            report["remote"]["control_plane_service"] = control_plane_state
            if control_plane_state["state"] != "inactive":
                report["errors"].append(
                    f"remote {args.remote_control_plane_service} remained {control_plane_state['state']}"
                )
        else:
            report["errors"].append(f"remote {args.remote_control_plane_service} is active")

    if socket_count["count"] > 0 and control_plane_state["state"] == "active":
        report["errors"].append(
            f"remote Telegram sockets still detected while {args.remote_control_plane_service} is active"
        )

    if report["errors"]:
        report["status"] = "fail"
        report["local"]["daemon_log_tail"] = _tail(Path(args.local_log_path).expanduser(), args.tail_lines)
    else:
        report["status"] = "pass"

    host_name = os.uname().nodename if hasattr(os, "uname") else os.getenv("HOSTNAME", "unknown")
    guard_kind = "ops.guard.heartbeat" if report["status"] == "pass" else "ops.guard.check_failed"
    guard_status, _, guard_body, guard_path_used, guard_paths_tried = _post_guard_report(
        args.api_base,
        args.guard_report_path,
        {
            "kind": guard_kind,
            "host_role": args.host_role,
            "host_name": host_name,
            "service_name": args.remote_openfang_service,
            "service_state": openfang_state["state"],
            "daemon_pid": int(pid) if pid.isdigit() else None,
            "request_id": request_id or None,
            "guard_interval_secs": 15,
            "outcome": report["status"],
            "failure_reason": "; ".join(report["errors"]) if report["errors"] else None,
            "primary_host": args.primary_host,
            "telegram_owner": args.telegram_owner,
            "primary_sentry_browser_host": args.primary_sentry_browser_host,
            "fallback_sentry_browser_host": args.fallback_sentry_browser_host,
            "blockers": report["errors"],
            "payload": report,
        },
    )
    report["sentry"]["guard_report_status_code"] = guard_status
    report["sentry"]["guard_report_body"] = guard_body
    report["sentry"]["guard_report_path_used"] = guard_path_used
    report["sentry"]["guard_report_paths_tried"] = guard_paths_tried
    if guard_status not in (200, 202):
        report["errors"].append(
            f"guard report endpoint failed status={guard_status} attempted={','.join(p['path'] for p in guard_paths_tried)}"
        )
        report["status"] = "fail"
        if "daemon_log_tail" not in report["local"]:
            report["local"]["daemon_log_tail"] = _tail(Path(args.local_log_path).expanduser(), args.tail_lines)

    for action in report["actions"]:
        result = action.get("result", {})
        action_rc = result.get("rc")
        action_status, _, _, action_path_used, action_paths_tried = _post_guard_report(
            args.api_base,
            args.guard_report_path,
            {
                "kind": "ops.guard.remediated" if action_rc == 0 else "ops.guard.failed",
                "host_role": args.host_role,
                "host_name": host_name,
                "service_name": result.get("service", args.remote_openfang_service),
                "service_state": openfang_state["state"],
                "daemon_pid": int(pid) if pid.isdigit() else None,
                "request_id": request_id or None,
                "guard_interval_secs": 15,
                "outcome": "pass" if action_rc == 0 else "fail",
                "failure_reason": result.get("stderr") or None,
                "remediation_action": action.get("type"),
                "remediation_result": "success" if action_rc == 0 else "failed",
                "primary_host": args.primary_host,
                "telegram_owner": args.telegram_owner,
                "primary_sentry_browser_host": args.primary_sentry_browser_host,
                "fallback_sentry_browser_host": args.fallback_sentry_browser_host,
                "blockers": report["errors"],
                "payload": action,
            },
        )
        action["sentry_report"] = {
            "status_code": action_status,
            "path_used": action_path_used,
            "paths_tried": action_paths_tried,
        }

    _write_json(out_path, report)
    _write_json(history_path, report)
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
