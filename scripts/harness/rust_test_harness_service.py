#!/usr/bin/env python3
"""HTTP Rust test harness with Docker isolation and Sentry reporting.

Endpoints:
  GET  /health
  POST /v1/rust-harness/run
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import io
import json
import os
import shutil
import subprocess
import tarfile
import tempfile
import traceback
import uuid
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Dict, List, Tuple

from sentry_client import send_sentry_event


DEFAULT_CHECKS = [
    {"name": "build", "command": "cargo build --workspace --all-targets"},
    {"name": "test", "command": "cargo test --workspace --all-targets"},
]

COPY_IGNORE_PATTERNS = (
    ".git",
    "target",
    ".venv",
    "venv",
    "__pycache__",
    "node_modules",
    ".pytest_cache",
)


class HarnessRequestError(Exception):
    def __init__(self, status_code: int, message: str) -> None:
        super().__init__(message)
        self.status_code = status_code
        self.message = message


@dataclass
class HarnessConfig:
    host: str
    port: int
    docker_image: str
    docker_cpus: str
    docker_memory: str
    docker_pids_limit: int
    default_timeout_secs: int
    max_request_bytes: int
    max_output_tail_bytes: int
    max_inline_source_bytes: int
    results_dir: Path
    allowed_roots: List[Path]
    sentry_dsn_env: str
    sentry_environment: str


def _now_utc() -> str:
    return dt.datetime.now(tz=dt.timezone.utc).isoformat()


def _is_relative_to(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def _tail_text(path: Path, max_bytes: int) -> str:
    if not path.exists():
        return ""
    size = path.stat().st_size
    with path.open("rb") as handle:
        if size > max_bytes:
            handle.seek(size - max_bytes)
            data = handle.read()
            return "...<truncated>\n" + data.decode("utf-8", errors="replace")
        return handle.read().decode("utf-8", errors="replace")


def _json_bytes(payload: Dict[str, Any]) -> bytes:
    return (json.dumps(payload, sort_keys=True) + "\n").encode("utf-8")


def _parse_int(name: str, value: str, default: int) -> int:
    if not value.strip():
        return default
    try:
        parsed = int(value)
    except Exception as exc:  # pragma: no cover - defensive
        raise ValueError(f"{name} must be an integer") from exc
    if parsed <= 0:
        raise ValueError(f"{name} must be > 0")
    return parsed


def _parse_allowed_roots(raw: str, fallback: Path) -> List[Path]:
    if not raw.strip():
        return [fallback.resolve()]
    roots: List[Path] = []
    for token in raw.split(","):
        tok = token.strip()
        if not tok:
            continue
        roots.append(Path(tok).expanduser().resolve())
    return roots or [fallback.resolve()]


def load_config() -> HarnessConfig:
    cwd = Path.cwd().resolve()
    port = _parse_int("RUST_HARNESS_PORT", os.getenv("RUST_HARNESS_PORT", "7788"), 7788)
    timeout_secs = _parse_int(
        "RUST_HARNESS_DEFAULT_TIMEOUT_SECS",
        os.getenv("RUST_HARNESS_DEFAULT_TIMEOUT_SECS", "900"),
        900,
    )
    max_request_bytes = _parse_int(
        "RUST_HARNESS_MAX_REQUEST_BYTES",
        os.getenv("RUST_HARNESS_MAX_REQUEST_BYTES", str(5 * 1024 * 1024)),
        5 * 1024 * 1024,
    )
    max_output_tail_bytes = _parse_int(
        "RUST_HARNESS_MAX_OUTPUT_TAIL_BYTES",
        os.getenv("RUST_HARNESS_MAX_OUTPUT_TAIL_BYTES", str(16 * 1024)),
        16 * 1024,
    )
    max_inline_source_bytes = _parse_int(
        "RUST_HARNESS_MAX_INLINE_SOURCE_BYTES",
        os.getenv("RUST_HARNESS_MAX_INLINE_SOURCE_BYTES", str(20 * 1024 * 1024)),
        20 * 1024 * 1024,
    )
    results_dir = Path(
        os.getenv(
            "RUST_HARNESS_RESULTS_DIR",
            str(cwd / "artifacts" / "rust-harness-runs"),
        )
    ).expanduser()
    allowed_roots = _parse_allowed_roots(
        os.getenv("RUST_HARNESS_ALLOWED_ROOTS", str(cwd)),
        cwd,
    )
    return HarnessConfig(
        host=os.getenv("RUST_HARNESS_HOST", "127.0.0.1"),
        port=port,
        docker_image=os.getenv("RUST_HARNESS_DOCKER_IMAGE", "rust:1.86-bookworm"),
        docker_cpus=os.getenv("RUST_HARNESS_DOCKER_CPUS", "2.0"),
        docker_memory=os.getenv("RUST_HARNESS_DOCKER_MEMORY", "4g"),
        docker_pids_limit=_parse_int(
            "RUST_HARNESS_DOCKER_PIDS_LIMIT",
            os.getenv("RUST_HARNESS_DOCKER_PIDS_LIMIT", "256"),
            256,
        ),
        default_timeout_secs=timeout_secs,
        max_request_bytes=max_request_bytes,
        max_output_tail_bytes=max_output_tail_bytes,
        max_inline_source_bytes=max_inline_source_bytes,
        results_dir=results_dir,
        allowed_roots=allowed_roots,
        sentry_dsn_env=os.getenv("RUST_HARNESS_SENTRY_DSN_ENV", "SENTRY_DSN"),
        sentry_environment=os.getenv("RUST_HARNESS_SENTRY_ENVIRONMENT", "production"),
    )


def check_docker_available() -> Tuple[bool, str]:
    try:
        proc = subprocess.run(
            ["docker", "version", "--format", "{{.Server.Version}}"],
            capture_output=True,
            text=True,
            timeout=8,
            check=False,
        )
    except Exception as exc:
        return False, f"docker_unavailable: {exc}"
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or f"exit_code={proc.returncode}"
        return False, f"docker_error: {detail}"
    version = proc.stdout.strip() or "unknown"
    return True, version


def _read_json_request(handler: BaseHTTPRequestHandler, max_bytes: int) -> Dict[str, Any]:
    raw_len = handler.headers.get("Content-Length", "0").strip() or "0"
    try:
        content_len = int(raw_len)
    except Exception as exc:
        raise HarnessRequestError(400, "invalid Content-Length header") from exc
    if content_len <= 0:
        raise HarnessRequestError(400, "request body is required")
    if content_len > max_bytes:
        raise HarnessRequestError(413, f"request too large (max {max_bytes} bytes)")
    body = handler.rfile.read(content_len)
    try:
        payload = json.loads(body.decode("utf-8"))
    except Exception as exc:
        raise HarnessRequestError(400, f"invalid JSON body: {exc}") from exc
    if not isinstance(payload, dict):
        raise HarnessRequestError(400, "JSON body must be an object")
    return payload


def _validate_env(env_payload: Any) -> Dict[str, str]:
    if env_payload is None:
        return {}
    if not isinstance(env_payload, dict):
        raise HarnessRequestError(400, "env must be an object")
    env: Dict[str, str] = {}
    for key, value in env_payload.items():
        if not isinstance(key, str) or not key:
            raise HarnessRequestError(400, "env keys must be non-empty strings")
        if not key.replace("_", "").isalnum() or key.upper() != key:
            raise HarnessRequestError(400, f"invalid env key: {key}")
        env[key] = str(value)
    return env


def _normalize_checks(payload: Any) -> List[Dict[str, Any]]:
    if payload is None:
        return [dict(item) for item in DEFAULT_CHECKS]
    if not isinstance(payload, list) or not payload:
        raise HarnessRequestError(400, "checks must be a non-empty array")
    checks: List[Dict[str, Any]] = []
    for idx, raw in enumerate(payload):
        if not isinstance(raw, dict):
            raise HarnessRequestError(400, f"checks[{idx}] must be an object")
        command = str(raw.get("command", "")).strip()
        if not command:
            raise HarnessRequestError(400, f"checks[{idx}].command is required")
        name = str(raw.get("name", f"check_{idx + 1}")).strip() or f"check_{idx + 1}"
        timeout_raw = raw.get("timeout_secs")
        timeout_secs = None
        if timeout_raw is not None:
            try:
                timeout_secs = int(timeout_raw)
            except Exception as exc:
                raise HarnessRequestError(400, f"checks[{idx}].timeout_secs must be an integer") from exc
            if timeout_secs <= 0:
                raise HarnessRequestError(400, f"checks[{idx}].timeout_secs must be > 0")
        checks.append({"name": name, "command": command, "timeout_secs": timeout_secs})
    return checks


def _validate_source_path(source_path: str, config: HarnessConfig) -> Path:
    resolved = Path(source_path).expanduser().resolve()
    if not resolved.exists():
        raise HarnessRequestError(400, f"source_path does not exist: {resolved}")
    if not resolved.is_dir():
        raise HarnessRequestError(400, f"source_path must be a directory: {resolved}")
    if not any(_is_relative_to(resolved, root) for root in config.allowed_roots):
        allowed = ", ".join(str(root) for root in config.allowed_roots)
        raise HarnessRequestError(403, f"source_path outside allowed roots: {allowed}")
    return resolved


def _copy_source_tree(src: Path, dest: Path) -> None:
    shutil.copytree(
        src,
        dest,
        ignore=shutil.ignore_patterns(*COPY_IGNORE_PATTERNS),
    )


def _extract_tarball(source_tar_gz_b64: str, dest: Path, config: HarnessConfig) -> None:
    try:
        raw = base64.b64decode(source_tar_gz_b64.encode("utf-8"), validate=True)
    except Exception as exc:
        raise HarnessRequestError(400, f"source_tar_gz_base64 is not valid base64: {exc}") from exc
    if len(raw) > config.max_inline_source_bytes:
        raise HarnessRequestError(
            413,
            f"source_tar_gz_base64 decoded payload too large (max {config.max_inline_source_bytes} bytes)",
        )

    try:
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
            for member in tar.getmembers():
                member_path = Path(member.name)
                if member_path.is_absolute() or ".." in member_path.parts:
                    raise HarnessRequestError(400, "tarball contains path traversal entries")
                if member.issym() or member.islnk():
                    raise HarnessRequestError(400, "tarball symlinks are not allowed")
            tar.extractall(dest)
    except HarnessRequestError:
        raise
    except Exception as exc:
        raise HarnessRequestError(400, f"failed to extract source tarball: {exc}") from exc


def _prepare_source(payload: Dict[str, Any], project_dir: Path, config: HarnessConfig) -> Dict[str, Any]:
    source_path_raw = payload.get("source_path")
    source_tar_b64 = payload.get("source_tar_gz_base64")
    if bool(source_path_raw) == bool(source_tar_b64):
        raise HarnessRequestError(400, "provide exactly one of source_path or source_tar_gz_base64")

    project_dir.parent.mkdir(parents=True, exist_ok=True)
    if source_path_raw:
        src = _validate_source_path(str(source_path_raw), config)
        _copy_source_tree(src, project_dir)
        source_meta = {"kind": "source_path", "source_path": str(src)}
    else:
        _extract_tarball(str(source_tar_b64), project_dir, config)
        source_meta = {"kind": "inline_tarball"}

    cargo_toml = project_dir / "Cargo.toml"
    if not cargo_toml.exists():
        raise HarnessRequestError(400, "source does not contain Cargo.toml at project root")
    return source_meta


def _docker_command(
    config: HarnessConfig,
    project_dir: Path,
    env_vars: Dict[str, str],
    command: str,
) -> List[str]:
    cmd = [
        "docker",
        "run",
        "--rm",
        "--network",
        "none",
        "--cpus",
        config.docker_cpus,
        "--memory",
        config.docker_memory,
        "--pids-limit",
        str(config.docker_pids_limit),
        "--security-opt",
        "no-new-privileges",
        "--cap-drop",
        "ALL",
        "-v",
        f"{project_dir}:/workspace",
        "-w",
        "/workspace",
    ]
    for key, value in env_vars.items():
        cmd.extend(["-e", f"{key}={value}"])
    cmd.extend([config.docker_image, "bash", "-lc", command])
    return cmd


def _run_one_check(
    config: HarnessConfig,
    project_dir: Path,
    env_vars: Dict[str, str],
    check: Dict[str, Any],
    timeout_secs: int,
) -> Dict[str, Any]:
    started_at = dt.datetime.now(tz=dt.timezone.utc)
    stdout_path = project_dir.parent / f"{check['name']}.stdout.log"
    stderr_path = project_dir.parent / f"{check['name']}.stderr.log"
    cmd = _docker_command(config, project_dir, env_vars, check["command"])

    with stdout_path.open("wb") as out, stderr_path.open("wb") as err:
        proc = subprocess.Popen(cmd, stdout=out, stderr=err)  # noqa: S603
        timeout_hit = False
        try:
            proc.wait(timeout=timeout_secs)
        except subprocess.TimeoutExpired:
            timeout_hit = True
            proc.kill()
            proc.wait(timeout=10)

    duration_ms = int((dt.datetime.now(tz=dt.timezone.utc) - started_at).total_seconds() * 1000)
    exit_code = -1 if timeout_hit else int(proc.returncode)
    status = "timeout" if timeout_hit else ("passed" if exit_code == 0 else "failed")
    return {
        "name": check["name"],
        "command": check["command"],
        "status": status,
        "exit_code": exit_code,
        "timeout_secs": timeout_secs,
        "duration_ms": duration_ms,
        "stdout_tail": _tail_text(stdout_path, config.max_output_tail_bytes),
        "stderr_tail": _tail_text(stderr_path, config.max_output_tail_bytes),
    }


def _emit_sentry(run_report: Dict[str, Any], config: HarnessConfig) -> Dict[str, Any]:
    dsn = os.getenv(config.sentry_dsn_env, "").strip()
    if not dsn:
        return {
            "enabled": False,
            "sent": False,
            "detail": f"missing env {config.sentry_dsn_env}",
            "http_status": None,
        }

    level = "info" if run_report["status"] == "passed" else "error"
    checks_for_event = [
        {
            "name": check["name"],
            "status": check["status"],
            "exit_code": check["exit_code"],
            "duration_ms": check["duration_ms"],
        }
        for check in run_report.get("checks", [])
    ]
    event = {
        "event_id": uuid.uuid4().hex,
        "timestamp": _now_utc(),
        "platform": "python",
        "logger": "rust-test-harness",
        "level": level,
        "message": f"rust_harness_run {run_report['status']} ({run_report['summary']['passed']}/{run_report['summary']['total']} checks passed)",
        "environment": config.sentry_environment,
        "tags": {
            "component": "rust-test-harness",
            "run_id": run_report["run_id"],
            "status": run_report["status"],
            "checks_total": str(run_report["summary"]["total"]),
            "checks_failed": str(run_report["summary"]["failed"]),
            "docker_image": run_report["isolation"]["docker_image"],
        },
        "extra": {
            "summary": run_report["summary"],
            "checks": checks_for_event,
            "isolation": run_report["isolation"],
        },
    }
    sent, detail, http_status = send_sentry_event(
        dsn,
        event,
        client_name="openfang-rust-test-harness/1.0",
    )
    return {
        "enabled": True,
        "sent": sent,
        "detail": detail,
        "http_status": http_status,
    }


def run_harness(payload: Dict[str, Any], config: HarnessConfig) -> Dict[str, Any]:
    checks = _normalize_checks(payload.get("checks"))
    env_vars = _validate_env(payload.get("env"))
    continue_on_failure = bool(payload.get("continue_on_failure", False))

    timeout_default = payload.get("timeout_secs", config.default_timeout_secs)
    try:
        timeout_default = int(timeout_default)
    except Exception as exc:
        raise HarnessRequestError(400, "timeout_secs must be an integer") from exc
    if timeout_default <= 0:
        raise HarnessRequestError(400, "timeout_secs must be > 0")

    run_id = uuid.uuid4().hex
    started_at = dt.datetime.now(tz=dt.timezone.utc)

    with tempfile.TemporaryDirectory(prefix=f"rust-harness-{run_id[:8]}-") as td:
        temp_root = Path(td)
        project_dir = temp_root / "project"
        source = _prepare_source(payload, project_dir, config)
        results: List[Dict[str, Any]] = []

        for check in checks:
            timeout_secs = check["timeout_secs"] if check["timeout_secs"] else timeout_default
            result = _run_one_check(config, project_dir, env_vars, check, timeout_secs)
            results.append(result)
            if result["status"] != "passed" and not continue_on_failure:
                break

    passed = sum(1 for item in results if item["status"] == "passed")
    failed = sum(1 for item in results if item["status"] != "passed")
    finished_at = dt.datetime.now(tz=dt.timezone.utc)
    duration_ms = int((finished_at - started_at).total_seconds() * 1000)
    status = "passed" if failed == 0 else "failed"

    report: Dict[str, Any] = {
        "run_id": run_id,
        "status": status,
        "started_at": started_at.isoformat(),
        "finished_at": finished_at.isoformat(),
        "duration_ms": duration_ms,
        "source": source,
        "checks": results,
        "summary": {
            "total": len(results),
            "passed": passed,
            "failed": failed,
        },
        "isolation": {
            "kind": "docker",
            "docker_image": config.docker_image,
            "network": "none",
            "cpus": config.docker_cpus,
            "memory": config.docker_memory,
            "pids_limit": config.docker_pids_limit,
            "security_opt": "no-new-privileges",
            "cap_drop": ["ALL"],
        },
    }
    report["sentry"] = _emit_sentry(report, config)

    config.results_dir.mkdir(parents=True, exist_ok=True)
    report_path = config.results_dir / f"{run_id}.json"
    report_path.write_bytes(_json_bytes(report))
    report["artifact_path"] = str(report_path.resolve())
    return report


class HarnessHandler(BaseHTTPRequestHandler):
    server_version = "RustHarnessHTTP/1.0"
    config: HarnessConfig

    def _write_json(self, status_code: int, payload: Dict[str, Any]) -> None:
        data = _json_bytes(payload)
        self.send_response(status_code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def _handle_error(self, exc: Exception) -> None:
        if isinstance(exc, HarnessRequestError):
            self._write_json(
                exc.status_code,
                {"status": "error", "error": exc.message},
            )
            return
        trace = traceback.format_exc(limit=4)
        self._write_json(
            500,
            {"status": "error", "error": str(exc), "trace": trace},
        )

    def do_GET(self) -> None:  # noqa: N802
        try:
            if self.path != "/health":
                raise HarnessRequestError(404, "not found")
            docker_ok, docker_detail = check_docker_available()
            status = 200 if docker_ok else 503
            self._write_json(
                status,
                {
                    "status": "ok" if docker_ok else "degraded",
                    "service": "rust-test-harness",
                    "time": _now_utc(),
                    "docker_available": docker_ok,
                    "docker_detail": docker_detail,
                    "config": {
                        "docker_image": self.config.docker_image,
                        "default_timeout_secs": self.config.default_timeout_secs,
                        "results_dir": str(self.config.results_dir),
                    },
                },
            )
        except Exception as exc:  # pragma: no cover - defensive
            self._handle_error(exc)

    def do_POST(self) -> None:  # noqa: N802
        try:
            if self.path != "/v1/rust-harness/run":
                raise HarnessRequestError(404, "not found")
            payload = _read_json_request(self, self.config.max_request_bytes)
            docker_ok, docker_detail = check_docker_available()
            if not docker_ok:
                raise HarnessRequestError(503, f"docker unavailable: {docker_detail}")
            report = run_harness(payload, self.config)
            status = 200 if report["status"] == "passed" else 422
            self._write_json(status, report)
        except Exception as exc:
            self._handle_error(exc)

    def log_message(self, fmt: str, *args: Any) -> None:  # noqa: A003
        return


def run_server(config: HarnessConfig) -> None:
    HarnessHandler.config = config
    httpd = ThreadingHTTPServer((config.host, config.port), HarnessHandler)
    print(
        json.dumps(
            {
                "service": "rust-test-harness",
                "listen": f"http://{config.host}:{config.port}",
                "docker_image": config.docker_image,
                "results_dir": str(config.results_dir.resolve()),
                "allowed_roots": [str(path) for path in config.allowed_roots],
            },
            sort_keys=True,
        )
    )
    httpd.serve_forever()


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Rust Docker test harness HTTP service")
    parser.add_argument("--host", default="", help="Override host")
    parser.add_argument("--port", default=0, type=int, help="Override port")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    config = load_config()
    if args.host:
        config.host = args.host
    if args.port:
        config.port = args.port
    run_server(config)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

