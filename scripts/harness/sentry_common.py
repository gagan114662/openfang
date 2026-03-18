#!/usr/bin/env python3
"""Shared Sentry config resolution for OpenFang harness scripts."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Any, Dict

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore


def load_sentry_config(path: str | None = None) -> Dict[str, Any]:
    config_path = Path(path or (Path.home() / ".openfang" / "config.toml"))
    if not config_path.exists():
        return {}
    try:
        payload = tomllib.loads(config_path.read_text(encoding="utf-8"))
    except Exception:
        return {}
    sentry = payload.get("sentry")
    return sentry if isinstance(sentry, dict) else {}


def resolve_sentry_token(sentry_cfg: Dict[str, Any], default_env: str = "SENTRY_AUTH_TOKEN") -> str:
    for env_name in [
        str(sentry_cfg.get("auth_token_env") or "").strip(),
        os.getenv("OPENFANG_SENTRY_AUTH_TOKEN_ENV", "").strip(),
        default_env,
    ]:
        if not env_name:
            continue
        token = os.getenv(env_name, "").strip()
        if token:
            return token
    return str(sentry_cfg.get("auth_token") or "").strip()


def resolve_sentry_value(
    explicit: str | None,
    sentry_cfg: Dict[str, Any],
    cfg_key: str,
    env_names: list[str],
    default: str = "",
) -> str:
    if explicit:
        return explicit.strip()
    for env_name in env_names:
        value = os.getenv(env_name, "").strip()
        if value:
            return value
    return str(sentry_cfg.get(cfg_key) or default).strip()
