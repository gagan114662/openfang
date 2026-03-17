#!/usr/bin/env python3
"""Google Home webhook bridge for OpenFang.

This bridge accepts simple HTTP requests from a home-automation trigger
and forwards them to OpenFang webhook trigger endpoints:
  - POST /hooks/wake
  - POST /hooks/agent

Usage:
  OPENFANG_WEBHOOK_TOKEN=... GOOGLE_HOME_BRIDGE_TOKEN=... python3 scripts/google_home_bridge.py
"""

from __future__ import annotations

import hmac
import json
import os
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any


def _env_bool(name: str, default: bool) -> bool:
    value = os.getenv(name, "").strip().lower()
    if not value:
        return default
    return value in {"1", "true", "yes", "on", "enabled"}


def _parse_bool(value: Any, default: bool) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, (int, float)):
        return value != 0
    if isinstance(value, str):
        low = value.strip().lower()
        if low in {"1", "true", "yes", "on", "enabled"}:
            return True
        if low in {"0", "false", "no", "off", "disabled"}:
            return False
    return default


def _parse_int(value: Any, default: int) -> int:
    try:
        return int(value)
    except Exception:
        return default


@dataclass
class BridgeConfig:
    listen_host: str
    listen_port: int
    openfang_base_url: str
    openfang_webhook_token: str
    bridge_token: str
    require_auth: bool
    default_agent: str | None
    default_deliver: bool
    default_channel: str | None
    default_model: str | None
    default_timeout_secs: int

    @classmethod
    def from_env(cls) -> "BridgeConfig":
        openfang_token = os.getenv("OPENFANG_WEBHOOK_TOKEN", "").strip()
        if len(openfang_token) < 32:
            raise ValueError(
                "OPENFANG_WEBHOOK_TOKEN must be set and at least 32 chars "
                "(matches OpenFang webhook trigger requirements)"
            )

        require_auth = _env_bool("GOOGLE_HOME_BRIDGE_REQUIRE_AUTH", True)
        bridge_token = os.getenv("GOOGLE_HOME_BRIDGE_TOKEN", "").strip()
        if require_auth and not bridge_token:
            raise ValueError(
                "GOOGLE_HOME_BRIDGE_TOKEN must be set when GOOGLE_HOME_BRIDGE_REQUIRE_AUTH=true"
            )

        timeout = _parse_int(os.getenv("GOOGLE_HOME_DEFAULT_TIMEOUT_SECS", "120"), 120)
        timeout = max(10, min(600, timeout))

        return cls(
            listen_host=os.getenv("GOOGLE_HOME_BRIDGE_HOST", "0.0.0.0").strip() or "0.0.0.0",
            listen_port=_parse_int(os.getenv("GOOGLE_HOME_BRIDGE_PORT", "8787"), 8787),
            openfang_base_url=os.getenv("OPENFANG_BASE_URL", "http://127.0.0.1:4200").rstrip("/"),
            openfang_webhook_token=openfang_token,
            bridge_token=bridge_token,
            require_auth=require_auth,
            default_agent=os.getenv("GOOGLE_HOME_DEFAULT_AGENT", "").strip() or None,
            default_deliver=_env_bool("GOOGLE_HOME_DEFAULT_DELIVER", False),
            default_channel=os.getenv("GOOGLE_HOME_DEFAULT_CHANNEL", "").strip() or None,
            default_model=os.getenv("GOOGLE_HOME_DEFAULT_MODEL", "").strip() or None,
            default_timeout_secs=timeout,
        )


def _safe_json_load(raw: bytes) -> dict[str, Any]:
    if not raw:
        return {}
    try:
        value = json.loads(raw.decode("utf-8"))
    except Exception:
        return {}
    return value if isinstance(value, dict) else {}


def _extract_bearer_token(auth_header: str) -> str:
    if not auth_header:
        return ""
    if not auth_header.startswith("Bearer "):
        return ""
    return auth_header[7:].strip()


class GoogleHomeBridgeHandler(BaseHTTPRequestHandler):
    server_version = "OpenFangGoogleHomeBridge/1.0"

    def _cfg(self) -> BridgeConfig:
        return self.server.config  # type: ignore[attr-defined]

    def _respond(self, code: int, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, ensure_ascii=True).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_body(self) -> bytes:
        raw_len = self.headers.get("Content-Length", "0")
        try:
            length = int(raw_len)
        except Exception:
            length = 0
        if length <= 0:
            return b""
        if length > 65536:
            return b""
        return self.rfile.read(length)

    def _authorized(self, query: dict[str, list[str]]) -> bool:
        cfg = self._cfg()
        if not cfg.require_auth:
            return True

        provided = (
            _extract_bearer_token(self.headers.get("Authorization", ""))
            or self.headers.get("X-Bridge-Token", "").strip()
            or (query.get("token", [""])[0].strip() if query.get("token") else "")
        )
        return bool(provided and hmac.compare_digest(provided, cfg.bridge_token))

    def _forward_openfang(self, endpoint: str, payload: dict[str, Any]) -> tuple[int, dict[str, Any]]:
        cfg = self._cfg()
        url = f"{cfg.openfang_base_url}{endpoint}"
        body = json.dumps(payload).encode("utf-8")
        req = urllib.request.Request(
            url=url,
            data=body,
            method="POST",
            headers={
                "Content-Type": "application/json",
                "Authorization": f"Bearer {cfg.openfang_webhook_token}",
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=30) as resp:
                raw = resp.read()
                result = _safe_json_load(raw)
                if not result:
                    result = {"raw": raw.decode("utf-8", errors="replace")}
                return resp.status, result
        except urllib.error.HTTPError as err:
            raw = err.read() if hasattr(err, "read") else b""
            result = _safe_json_load(raw)
            if not result:
                result = {"error": raw.decode("utf-8", errors="replace") or str(err)}
            return int(getattr(err, "code", 500)), result
        except Exception as err:  # pragma: no cover
            return 502, {"error": f"bridge_forward_failed: {err}"}

    def _wake_from_request(self, query: dict[str, list[str]], body: dict[str, Any]) -> tuple[int, dict[str, Any]]:
        text = (
            (query.get("text", [""])[0].strip() if query.get("text") else "")
            or str(body.get("text", "")).strip()
            or str(body.get("command", "")).strip()
        )
        mode = (
            (query.get("mode", [""])[0].strip() if query.get("mode") else "")
            or str(body.get("mode", "")).strip()
            or "now"
        )
        if not text:
            return 400, {"error": "missing text for wake action"}
        return self._forward_openfang("/hooks/wake", {"text": text, "mode": mode})

    def _agent_from_request(
        self, query: dict[str, list[str]], body: dict[str, Any]
    ) -> tuple[int, dict[str, Any]]:
        cfg = self._cfg()
        message = (
            (query.get("message", [""])[0].strip() if query.get("message") else "")
            or str(body.get("message", "")).strip()
            or str(body.get("text", "")).strip()
            or str(body.get("command", "")).strip()
        )
        if not message:
            return 400, {"error": "missing message for agent action"}

        agent = (
            (query.get("agent", [""])[0].strip() if query.get("agent") else "")
            or str(body.get("agent", "")).strip()
            or cfg.default_agent
        )
        channel = (
            (query.get("channel", [""])[0].strip() if query.get("channel") else "")
            or str(body.get("channel", "")).strip()
            or cfg.default_channel
        )
        model = (
            (query.get("model", [""])[0].strip() if query.get("model") else "")
            or str(body.get("model", "")).strip()
            or cfg.default_model
        )
        timeout_secs = _parse_int(
            (query.get("timeout_secs", [""])[0] if query.get("timeout_secs") else body.get("timeout_secs")),
            cfg.default_timeout_secs,
        )
        timeout_secs = max(10, min(600, timeout_secs))
        deliver = _parse_bool(
            (query.get("deliver", [""])[0] if query.get("deliver") else body.get("deliver")),
            cfg.default_deliver,
        )

        payload: dict[str, Any] = {
            "message": message,
            "deliver": deliver,
            "timeout_secs": timeout_secs,
        }
        if agent:
            payload["agent"] = agent
        if channel:
            payload["channel"] = channel
        if model:
            payload["model"] = model

        return self._forward_openfang("/hooks/agent", payload)

    def do_GET(self) -> None:  # noqa: N802
        parsed = urllib.parse.urlparse(self.path)
        query = urllib.parse.parse_qs(parsed.query, keep_blank_values=False)

        if parsed.path in {"/health", "/healthz"}:
            self._respond(200, {"ok": True, "service": "google_home_bridge"})
            return

        if not self._authorized(query):
            self._respond(401, {"error": "unauthorized"})
            return

        if parsed.path == "/google-home/wake":
            code, payload = self._wake_from_request(query, {})
            self._respond(code, payload)
            return
        if parsed.path == "/google-home/agent":
            code, payload = self._agent_from_request(query, {})
            self._respond(code, payload)
            return

        self._respond(404, {"error": "not_found"})

    def do_POST(self) -> None:  # noqa: N802
        parsed = urllib.parse.urlparse(self.path)
        query = urllib.parse.parse_qs(parsed.query, keep_blank_values=False)
        body = _safe_json_load(self._read_body())

        if not self._authorized(query):
            self._respond(401, {"error": "unauthorized"})
            return

        if parsed.path == "/google-home/wake":
            code, payload = self._wake_from_request(query, body)
            self._respond(code, payload)
            return
        if parsed.path == "/google-home/agent":
            code, payload = self._agent_from_request(query, body)
            self._respond(code, payload)
            return
        if parsed.path == "/google-home":
            action = str(body.get("action", "")).strip().lower()
            if action == "wake":
                code, payload = self._wake_from_request(query, body)
                self._respond(code, payload)
                return
            if action == "agent":
                code, payload = self._agent_from_request(query, body)
                self._respond(code, payload)
                return
            self._respond(400, {"error": "unknown action; use wake or agent"})
            return

        self._respond(404, {"error": "not_found"})

    def log_message(self, fmt: str, *args: object) -> None:
        # Keep logs structured and minimal for daemon use.
        print(f"[google_home_bridge] {self.address_string()} {fmt % args}")


def main() -> int:
    cfg = BridgeConfig.from_env()
    server = ThreadingHTTPServer((cfg.listen_host, cfg.listen_port), GoogleHomeBridgeHandler)
    server.config = cfg  # type: ignore[attr-defined]
    print(
        json.dumps(
            {
                "service": "google_home_bridge",
                "listen": f"{cfg.listen_host}:{cfg.listen_port}",
                "openfang_base_url": cfg.openfang_base_url,
                "auth_required": cfg.require_auth,
            }
        )
    )
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
