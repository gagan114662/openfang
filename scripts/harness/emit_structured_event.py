#!/usr/bin/env python3
"""Emit a structured OpenFang telemetry event when the local daemon is available."""

from __future__ import annotations

import argparse
import json
import os
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Emit a structured OpenFang event")
    parser.add_argument("--event-kind", required=True, help="Canonical event.kind value")
    parser.add_argument("--body", default="", help="Human-readable body")
    parser.add_argument("--level", default="info", help="Event level")
    parser.add_argument("--attributes-json", default="{}", help="JSON object with extra attributes")
    parser.add_argument("--out", help="Optional JSONL file to append a copy to")
    return parser.parse_args()


def daemon_base_url() -> str:
    explicit = os.environ.get("OPENFANG_API_BASE", "").strip()
    if explicit:
        return explicit.rstrip("/")

    daemon_info = Path.home() / ".openfang" / "daemon.json"
    if daemon_info.exists():
        try:
            data = json.loads(daemon_info.read_text(encoding="utf-8"))
            listen_addr = str(data.get("listen_addr") or "").strip()
            if listen_addr:
                if listen_addr.startswith(("http://", "https://")):
                    return listen_addr.rstrip("/")
                return f"http://{listen_addr}"
        except Exception:
            pass
    return ""


def append_copy(out_path: str, payload: Dict[str, Any]) -> None:
    target = Path(out_path)
    target.parent.mkdir(parents=True, exist_ok=True)
    with target.open("a", encoding="utf-8") as handle:
        handle.write(json.dumps(payload, ensure_ascii=True) + "\n")


def main() -> int:
    args = parse_args()
    try:
        extra = json.loads(args.attributes_json)
    except Exception as exc:
        raise SystemExit(f"Invalid --attributes-json payload: {exc}")
    if not isinstance(extra, dict):
        raise SystemExit("--attributes-json must decode to an object")

    payload: Dict[str, Any] = {
        "body": args.body or args.event_kind,
        "level": args.level,
        "attributes": {
            "event.kind": args.event_kind,
            **extra,
        },
    }

    if args.out:
        append_copy(args.out, payload)

    base_url = daemon_base_url()
    if not base_url:
        return 0

    req = urllib.request.Request(
        f"{base_url}/api/telemetry/structured",
        data=json.dumps(payload).encode("utf-8"),
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    try:
        with urllib.request.urlopen(req, timeout=5):
            return 0
    except (urllib.error.URLError, TimeoutError):
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
