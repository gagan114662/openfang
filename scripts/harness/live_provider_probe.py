#!/usr/bin/env python3
"""Lightweight live-provider probes for OpenFang PR gating."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Tuple


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run live provider probes for OpenFang")
    parser.add_argument("--contract", default=".harness/policy.contract.json", help="Policy contract path")
    parser.add_argument("--head-sha", required=True, help="Head SHA under evaluation")
    parser.add_argument("--risk-tier", required=True, choices=["critical", "high", "medium", "low"], help="Risk tier")
    parser.add_argument(
        "--out",
        default="artifacts/agent-evals/live-provider-report.json",
        help="Output report path",
    )
    parser.add_argument("--timeout-secs", type=int, default=15, help="HTTP timeout seconds")
    parser.add_argument("--attempts-override", type=int, default=0, help="Retry attempts override")
    return parser.parse_args()


def _read_json(path: Path) -> Dict[str, Any]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict):
        raise ValueError(f"expected object in {path}")
    return payload


def _write_json(path: Path, payload: Dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _classify_http(code: int) -> str:
    if code == 429:
        return "rate_limit"
    if code in {401, 403}:
        return "auth"
    if code >= 500:
        return "outage"
    if 400 <= code < 500:
        return "other"
    return "ok"


def _headers_for_provider(name: str, key: str) -> Dict[str, str]:
    if name == "openai":
        return {"Authorization": f"Bearer {key}"}
    if name == "anthropic":
        return {"x-api-key": key, "anthropic-version": "2023-06-01"}
    return {}


def _url_for_provider(name: str, base_url: str, key: str) -> str:
    return base_url


def _run_http_probe(name: str, url: str, key: str, timeout_secs: int) -> Tuple[str, int | None, str]:
    headers = _headers_for_provider(name, key)
    req = urllib.request.Request(_url_for_provider(name, url, key), headers=headers, method="GET")
    try:
        with urllib.request.urlopen(req, timeout=timeout_secs) as resp:
            status = int(resp.status)
            cls = _classify_http(status)
            return cls, status, f"http_status={status}"
    except urllib.error.HTTPError as exc:
        cls = _classify_http(int(exc.code))
        return cls, int(exc.code), f"http_error={exc.code}"
    except urllib.error.URLError as exc:
        return "network", None, f"url_error={exc.reason}"
    except Exception as exc:  # pragma: no cover
        return "other", None, f"unexpected_error={exc}"


def _load_gate_cfg(contract: Dict[str, Any]) -> Dict[str, Any]:
    eval_policy = contract.get("agentEvalPolicy", {})
    if not isinstance(eval_policy, dict):
        return {}
    gate = eval_policy.get("liveProviderGate", {})
    return gate if isinstance(gate, dict) else {}


def _default_catalog() -> Dict[str, Dict[str, str]]:
    return {
        "openai": {"env": "OPENAI_API_KEY", "url": "https://api.openai.com/v1/models", "method": "GET"},
        "anthropic": {"env": "ANTHROPIC_API_KEY", "url": "https://api.anthropic.com/v1/models", "method": "GET"},
    }


def main() -> int:
    args = parse_args()
    contract = _read_json(Path(args.contract))
    gate_cfg = _load_gate_cfg(contract)

    enabled = bool(gate_cfg.get("enabled", True))
    blocking_tiers = gate_cfg.get("blockingRiskTiers", ["critical", "high"])
    blocking_tiers = [str(item) for item in blocking_tiers] if isinstance(blocking_tiers, list) else ["critical", "high"]
    blocking_applies = args.risk_tier in set(blocking_tiers)
    min_success = int(gate_cfg.get("minSuccessfulProviders", 1) or 1)
    fail_if_no_secrets = bool(gate_cfg.get("failIfNoProviderSecrets", True))

    retry_cfg = gate_cfg.get("retries", {})
    retry_cfg = retry_cfg if isinstance(retry_cfg, dict) else {}
    attempts = int(retry_cfg.get("attempts", 3) or 3)
    if args.attempts_override > 0:
        attempts = args.attempts_override
    attempts = max(1, attempts)
    backoff = retry_cfg.get("backoffSeconds", [5, 20, 60])
    backoff = [int(v) for v in backoff if isinstance(v, int) and v >= 0]
    if not backoff:
        backoff = [5, 20, 60]

    catalog = gate_cfg.get("providerCatalog", {})
    if not isinstance(catalog, dict) or not catalog:
        catalog = _default_catalog()

    providers: List[Dict[str, Any]] = []
    failure_classes: Dict[str, int] = {}
    detected = 0
    successful = 0
    errors: List[str] = []

    for name, raw_cfg in sorted(catalog.items()):
        cfg = raw_cfg if isinstance(raw_cfg, dict) else {}
        env_name = str(cfg.get("env", "")).strip()
        url = str(cfg.get("url", "")).strip()
        method = str(cfg.get("method", "GET")).upper()
        key = os.getenv(env_name, "").strip() if env_name else ""

        base = {
            "name": str(name),
            "enabled": bool(key),
            "status": "skipped",
            "http_status": None,
            "classification": "missing_secret",
            "detail": f"missing env var {env_name}" if env_name else "missing env var mapping",
            "attempts_used": 0,
            "method": method,
        }

        if not key:
            providers.append(base)
            continue
        if not url:
            base["status"] = "fail"
            base["classification"] = "other"
            base["detail"] = "missing probe URL"
            providers.append(base)
            failure_classes["other"] = failure_classes.get("other", 0) + 1
            errors.append(f"{name}: missing probe URL")
            detected += 1
            continue
        if method != "GET":
            base["status"] = "fail"
            base["classification"] = "other"
            base["detail"] = f"unsupported method={method}"
            providers.append(base)
            failure_classes["other"] = failure_classes.get("other", 0) + 1
            errors.append(f"{name}: unsupported method {method}")
            detected += 1
            continue

        detected += 1
        final_class = "other"
        final_code: int | None = None
        final_detail = "probe did not execute"
        used_attempts = 0
        is_success = False

        for attempt in range(1, attempts + 1):
            cls, code, detail = _run_http_probe(str(name), url, key, args.timeout_secs)
            used_attempts = attempt
            final_class = cls
            final_code = code
            final_detail = detail
            if cls == "ok":
                is_success = True
                break
            if cls not in {"rate_limit", "outage", "network"}:
                break
            if attempt < attempts:
                sleep_for = backoff[min(attempt - 1, len(backoff) - 1)]
                time.sleep(max(0, sleep_for))

        base["attempts_used"] = used_attempts
        base["http_status"] = final_code
        base["classification"] = final_class
        base["detail"] = final_detail
        if is_success:
            base["status"] = "pass"
            successful += 1
        else:
            base["status"] = "fail"
            failure_classes[final_class] = failure_classes.get(final_class, 0) + 1
            errors.append(f"{name}: {final_detail}")
        providers.append(base)

    if not enabled:
        status = "advisory"
    elif detected == 0 and fail_if_no_secrets:
        status = "fail" if blocking_applies else "advisory"
        errors.append("no provider API secrets detected for live probe")
    elif successful >= min_success:
        status = "pass"
    else:
        status = "fail" if blocking_applies else "advisory"

    report: Dict[str, Any] = {
        "head_sha": args.head_sha,
        "risk_tier": args.risk_tier,
        "status": status,
        "enabled": enabled,
        "blocking_applies": blocking_applies,
        "min_successful_providers": min_success,
        "detected_providers": detected,
        "successful_providers": successful,
        "providers": providers,
        "external_failure_classes": failure_classes,
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "errors": errors,
    }

    _write_json(Path(args.out), report)
    print(json.dumps(report, indent=2, sort_keys=True))

    if blocking_applies and status == "fail":
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
