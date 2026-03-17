#!/usr/bin/env python3
"""Seed native OpenFang unattended schedules from config/unattended_workloads.toml."""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any, Dict, List, Optional, Set


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Seed unattended OpenFang workloads")
    parser.add_argument("--api-base", default="http://127.0.0.1:50051", help="OpenFang API base URL")
    parser.add_argument(
        "--registry",
        default="config/unattended_workloads.toml",
        help="Path to unattended workload registry TOML",
    )
    parser.add_argument(
        "--ops-agent-name",
        default="ops-coder",
        help="Name of the dedicated unattended ops agent",
    )
    parser.add_argument(
        "--ops-agent-manifest",
        default="config/agents/ops-coder.toml",
        help="Path to the repo-tracked ops agent manifest",
    )
    return parser.parse_args()


def http_json(method: str, url: str, payload: Optional[Dict[str, Any]] = None) -> Any:
    body = None
    headers = {"Accept": "application/json"}
    if payload is not None:
        body = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, method=method, data=body, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=20) as response:
            raw = response.read().decode("utf-8", errors="replace")
            return json.loads(raw) if raw.strip() else {}
    except urllib.error.HTTPError as exc:
        raw = exc.read().decode("utf-8", errors="replace")
        try:
            detail = json.loads(raw) if raw.strip() else {}
        except json.JSONDecodeError:
            detail = raw
        raise RuntimeError(f"{method} {url} failed: {exc.code} {detail}") from exc


def load_registry(path: Path) -> List[Dict[str, Any]]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    workloads = data.get("workloads")
    if not isinstance(workloads, list):
        raise RuntimeError(f"registry at {path} is missing [[workloads]] entries")
    return workloads


def load_manifest(path: Path) -> Dict[str, Any]:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def list_agents(api_base: str) -> List[Dict[str, Any]]:
    payload = http_json("GET", f"{api_base.rstrip('/')}/api/agents")
    if isinstance(payload, list):
        return payload
    if isinstance(payload, dict):
        for key in ("agents", "items", "data"):
            value = payload.get(key)
            if isinstance(value, list):
                return value
    raise RuntimeError("unexpected /api/agents response shape")


def get_agent(api_base: str, agent_id: str) -> Dict[str, Any]:
    payload = http_json("GET", f"{api_base.rstrip('/')}/api/agents/{agent_id}")
    if not isinstance(payload, dict):
        raise RuntimeError(f"unexpected /api/agents/{agent_id} response shape")
    return payload


def get_agent_budget(api_base: str, agent_id: str) -> Dict[str, Any]:
    payload = http_json("GET", f"{api_base.rstrip('/')}/api/budget/agents/{agent_id}")
    if not isinstance(payload, dict):
        raise RuntimeError(f"unexpected /api/budget/agents/{agent_id} response shape")
    return payload


def list_schedules(api_base: str) -> List[Dict[str, Any]]:
    payload = http_json("GET", f"{api_base.rstrip('/')}/api/schedules")
    if isinstance(payload, dict):
        schedules = payload.get("schedules")
        if isinstance(schedules, list):
            return schedules
    if isinstance(payload, list):
        return payload
    raise RuntimeError("unexpected /api/schedules response shape")


def find_agent(agents: List[Dict[str, Any]], name: str) -> Optional[Dict[str, Any]]:
    for agent in agents:
        if str(agent.get("name", "")).lower() == name.lower():
            return agent
    return None


def required_tool_set(manifest: Dict[str, Any]) -> Set[str]:
    capabilities = manifest.get("capabilities") or {}
    tools = capabilities.get("tools") or []
    if not isinstance(tools, list):
        return set()
    return {str(tool) for tool in tools}


def agent_matches_manifest(
    api_base: str,
    agent: Dict[str, Any],
    manifest: Dict[str, Any],
) -> bool:
    agent_id = str(agent.get("id") or "")
    if not agent_id:
        return False

    detail = get_agent(api_base, agent_id)
    budget = get_agent_budget(api_base, agent_id)

    required_model = manifest.get("model") or {}
    required_resources = manifest.get("resources") or {}
    required_tools = required_tool_set(manifest)

    actual_model = detail.get("model") or {}
    actual_tools = set((detail.get("capabilities") or {}).get("tools") or [])
    actual_hourly_limit = ((budget.get("hourly") or {}).get("limit"))
    actual_daily_limit = ((budget.get("daily") or {}).get("limit"))
    actual_monthly_limit = ((budget.get("monthly") or {}).get("limit"))

    return (
        str(detail.get("name") or "") == str(manifest.get("name") or "")
        and str(actual_model.get("provider") or "") == str(required_model.get("provider") or "")
        and str(actual_model.get("model") or "") == str(required_model.get("model") or "")
        and required_tools.issubset(actual_tools)
        and float(actual_hourly_limit or 0.0) == float(required_resources.get("max_cost_per_hour_usd") or 0.0)
        and float(actual_daily_limit or 0.0) == float(required_resources.get("max_cost_per_day_usd") or 0.0)
        and float(actual_monthly_limit or 0.0) == float(required_resources.get("max_cost_per_month_usd") or 0.0)
    )


def kill_agent(api_base: str, agent_id: str) -> None:
    http_json("DELETE", f"{api_base.rstrip('/')}/api/agents/{agent_id}")


def spawn_ops_agent(api_base: str, manifest_path: Path) -> Dict[str, Any]:
    manifest_toml = manifest_path.read_text(encoding="utf-8")
    http_json(
        "POST",
        f"{api_base.rstrip('/')}/api/agents",
        {"manifest_toml": manifest_toml},
    )
    refreshed = list_agents(api_base)
    created = find_agent(refreshed, tomllib.loads(manifest_toml).get("name", "ops-coder"))
    if created is None:
        raise RuntimeError("agent spawn reported success but the ops agent was not found afterwards")
    return created


def ensure_ops_agent(
    api_base: str,
    agents: List[Dict[str, Any]],
    ops_agent_name: str,
    manifest_path: Path,
    manifest: Dict[str, Any],
) -> Dict[str, Any]:
    existing = find_agent(agents, ops_agent_name)
    if existing is None:
        return spawn_ops_agent(api_base, manifest_path)

    if agent_matches_manifest(api_base, existing, manifest):
        return existing

    agent_id = str(existing.get("id") or "")
    if not agent_id:
        raise RuntimeError(f"ops agent '{ops_agent_name}' exists without an id")
    kill_agent(api_base, agent_id)
    return spawn_ops_agent(api_base, manifest_path)


def upsert_schedule(api_base: str, schedules: List[Dict[str, Any]], workload: Dict[str, Any], agent_id: str) -> Dict[str, Any]:
    payload = {
        "name": workload["id"],
        "cron": workload["schedule"],
        "agent_id": agent_id,
        "message": workload["message"],
        "enabled": True,
    }
    existing = next((s for s in schedules if str(s.get("name")) == workload["id"]), None)
    if existing is None:
        created = http_json("POST", f"{api_base.rstrip('/')}/api/schedules", payload)
        return {"action": "created", "schedule": created}
    updated = http_json(
        "PUT",
        f"{api_base.rstrip('/')}/api/schedules/{existing['id']}",
        payload,
    )
    return {"action": "updated", "schedule": updated, "id": existing["id"]}


def main() -> int:
    args = parse_args()
    registry_path = Path(args.registry)
    if not registry_path.is_absolute():
        registry_path = Path.cwd() / registry_path
    manifest_path = Path(args.ops_agent_manifest)
    if not manifest_path.is_absolute():
        manifest_path = Path.cwd() / manifest_path
    workloads = load_registry(registry_path)
    manifest = load_manifest(manifest_path)
    agents = list_agents(args.api_base)
    ops_agent = ensure_ops_agent(
        args.api_base,
        agents,
        args.ops_agent_name,
        manifest_path,
        manifest,
    )
    schedules = list_schedules(args.api_base)

    results = []
    for workload in workloads:
        agent_name = workload.get("agent_name", args.ops_agent_name)
        if agent_name != args.ops_agent_name:
            target = find_agent(agents, agent_name)
            if target is None:
                raise RuntimeError(f"agent '{agent_name}' not found for workload '{workload['id']}'")
        else:
            target = ops_agent
        results.append(upsert_schedule(args.api_base, schedules, workload, str(target["id"])))
        schedules = list_schedules(args.api_base)

    print(
        json.dumps(
            {
                "ops_agent": {"id": ops_agent["id"], "name": ops_agent["name"]},
                "ops_agent_manifest": str(manifest_path),
                "results": results,
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as exc:
        print(json.dumps({"error": str(exc)}), file=sys.stderr)
        raise SystemExit(1)
