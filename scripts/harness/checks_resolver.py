#!/usr/bin/env python3
"""Risk-tier and policy helpers for harness workflows."""

from __future__ import annotations

import fnmatch
import json
from pathlib import Path, PurePosixPath
from typing import Any, Dict, Iterable, List, Tuple


RISK_ORDER = ["critical", "high", "medium", "low"]

# Legacy/semantic check aliases mapped to concrete CI job names.
CHECK_ALIAS_MAP = {
    "build": "ci-check",
    "test": "ci-test",
    "clippy": "ci-clippy",
    "fmt": "ci-fmt",
    "format": "ci-fmt",
    "security-audit": "ci-audit",
    "audit": "ci-audit",
    "secrets": "ci-secrets",
    "infra-preflight": "infra-preflight",
    "agent-evals-live-pr": "agent-evals-live-pr",
    "sentry-live-validate": "sentry-live-validate",
    "integration-tests": "ci-test",
    "install-smoke": "ci-install-smoke",
    "code-review": "pr-review-harness",
    "docs-lint": "pr-review-harness",
}


def normalize_path(path: str) -> str:
    normalized = path.strip().replace("\\", "/")
    while normalized.startswith("./"):
        normalized = normalized[2:]
    if normalized.startswith("/"):
        normalized = normalized[1:]
    return normalized


def path_matches(path: str, pattern: str) -> bool:
    p = normalize_path(path)
    pat = normalize_path(pattern)
    if not pat:
        return False
    if fnmatch.fnmatch(p, pat):
        return True
    try:
        return PurePosixPath(p).match(pat)
    except Exception:
        return False


def any_path_matches(paths: Iterable[str], patterns: Iterable[str]) -> bool:
    normalized_paths = [normalize_path(p) for p in paths]
    normalized_patterns = [normalize_path(pat) for pat in patterns]
    return any(path_matches(path, pattern) for path in normalized_paths for pattern in normalized_patterns)


def load_contract(contract_path: str) -> Dict[str, Any]:
    with open(contract_path, "r", encoding="utf-8") as f:
        return json.load(f)


def _extract_risk_tier_rules(contract_or_rules: Dict[str, Any]) -> Dict[str, List[str]]:
    # Legacy contract style: {"riskTierRules": {"high": ["..."]}}
    if isinstance(contract_or_rules.get("riskTierRules"), dict):
        out: Dict[str, List[str]] = {}
        for tier, patterns in contract_or_rules["riskTierRules"].items():
            if isinstance(patterns, list):
                out[str(tier)] = [str(p) for p in patterns]
        return out

    # Current contract style: {"riskTiers": {"high": {"paths": ["..."]}}}
    if isinstance(contract_or_rules.get("riskTiers"), dict):
        out = {}
        for tier, cfg in contract_or_rules["riskTiers"].items():
            if not isinstance(cfg, dict):
                continue
            paths = cfg.get("paths", [])
            if isinstance(paths, list):
                out[str(tier)] = [str(p) for p in paths]
        return out

    # Direct rules map.
    out = {}
    for tier, patterns in contract_or_rules.items():
        if isinstance(patterns, list):
            out[str(tier)] = [str(p) for p in patterns]
    return out


def compute_risk_tier(changed_files: List[str], contract_or_rules: Dict[str, Any]) -> str:
    if not changed_files:
        return "low"

    risk_tier_rules = _extract_risk_tier_rules(contract_or_rules)

    for tier in RISK_ORDER:
        patterns = risk_tier_rules.get(tier, [])
        if any_path_matches(changed_files, patterns):
            return tier

    for tier, patterns in risk_tier_rules.items():
        if tier not in RISK_ORDER and any_path_matches(changed_files, patterns):
            return tier

    return "low"


def _normalize_required_checks(checks: Iterable[str]) -> List[str]:
    normalized: List[str] = []
    for check in checks:
        raw = str(check).strip()
        if not raw:
            continue
        key = raw.lower()
        mapped = CHECK_ALIAS_MAP.get(key, raw)
        normalized.append(mapped)

    deduped: List[str] = []
    seen = set()
    for check in normalized:
        if check in seen:
            continue
        seen.add(check)
        deduped.append(check)
    return deduped


def compute_required_checks(contract: Dict[str, Any], risk_tier: str) -> List[str]:
    checks: List[str] = []

    # Legacy contract style.
    merge_policy = contract.get("mergePolicy", {})
    if isinstance(merge_policy, dict):
        tier_policy = merge_policy.get(risk_tier, {})
        if isinstance(tier_policy, dict):
            raw_checks = tier_policy.get("requiredChecks", [])
            if isinstance(raw_checks, list):
                checks.extend(str(check) for check in raw_checks)

    # Current contract style.
    risk_tiers = contract.get("riskTiers", {})
    if isinstance(risk_tiers, dict):
        tier_cfg = risk_tiers.get(risk_tier, {})
        if isinstance(tier_cfg, dict):
            raw_checks = tier_cfg.get("requiredChecks", [])
            if isinstance(raw_checks, list):
                checks.extend(str(check) for check in raw_checks)

    normalized = _normalize_required_checks(checks)

    # Ensure PR harness check is emitted when enabled.
    pr_harness = contract.get("prReviewHarness", {})
    if isinstance(pr_harness, dict) and bool(pr_harness.get("enabled", False)):
        required_name = str(pr_harness.get("requiredCheckName", "pr-review-harness")).strip() or "pr-review-harness"
        if required_name not in normalized:
            normalized.append(required_name)

    return normalized


def _as_docs_rule_list(docs_drift_rules: Any) -> List[Dict[str, Any]]:
    if isinstance(docs_drift_rules, list):
        return [rule for rule in docs_drift_rules if isinstance(rule, dict)]

    if isinstance(docs_drift_rules, dict):
        # Current contract object form.
        require_docs_update = docs_drift_rules.get("requireDocsUpdate", [])
        docs_files = docs_drift_rules.get("docsFiles", [])
        rules: List[Dict[str, Any]] = []
        if isinstance(require_docs_update, list) and isinstance(docs_files, list):
            rules.append(
                {
                    "name": "default-docs-drift",
                    "whenTouched": [str(path) for path in require_docs_update],
                    "requireAny": [str(path) for path in docs_files],
                }
            )
        nested = docs_drift_rules.get("rules", [])
        if isinstance(nested, list):
            for rule in nested:
                if isinstance(rule, dict):
                    rules.append(rule)
        return rules

    return []


def evaluate_docs_drift(changed_files: List[str], docs_drift_rules: Any) -> List[str]:
    violations: List[str] = []
    rules = _as_docs_rule_list(docs_drift_rules)

    for rule in rules:
        name = str(rule.get("name", "unnamed-rule"))
        touched_patterns = rule.get("whenTouched", [])
        required_any = rule.get("requireAny", [])

        if not touched_patterns or not required_any:
            continue

        touched = any_path_matches(changed_files, touched_patterns)
        doc_updated = any_path_matches(changed_files, required_any)

        if touched and not doc_updated:
            violations.append(
                f"docs drift rule '{name}' violated: changes touched {touched_patterns} but none of {required_any} were updated"
            )

    return violations


def requires_browser_evidence(changed_files: List[str], contract: Dict[str, Any]) -> bool:
    evidence_policy = contract.get("evidencePolicy", {})
    if isinstance(evidence_policy, dict):
        paths = evidence_policy.get("uiImpactPaths", [])
        if isinstance(paths, list) and paths:
            return any_path_matches(changed_files, paths)

    browser_reqs = contract.get("browserEvidenceRequirements", {})
    paths = []
    if isinstance(browser_reqs, dict):
        paths = browser_reqs.get("uiChangePaths", [])
    return any_path_matches(changed_files, paths)


def get_evidence_policy(contract: Dict[str, Any]) -> Dict[str, Any]:
    policy: Dict[str, Any] = {
        "required_flows": [],
        "required_assertions": [],
        "ui_impact_paths": [],
        "min_screenshots": 2,
        "min_videos": 1,
    }

    evidence_policy = contract.get("evidencePolicy", {})
    if isinstance(evidence_policy, dict):
        required_flows = evidence_policy.get("requiredFlows", [])
        required_assertions = evidence_policy.get("requiredAssertions", [])
        ui_impact_paths = evidence_policy.get("uiImpactPaths", [])
        if isinstance(required_flows, list):
            policy["required_flows"] = [str(item) for item in required_flows]
        if isinstance(required_assertions, list):
            policy["required_assertions"] = [str(item) for item in required_assertions]
        if isinstance(ui_impact_paths, list):
            policy["ui_impact_paths"] = [str(item) for item in ui_impact_paths]

    browser_reqs = contract.get("browserEvidenceRequirements", {})
    if isinstance(browser_reqs, dict):
        ui_paths = browser_reqs.get("uiChangePaths", [])
        if isinstance(ui_paths, list) and ui_paths:
            policy["ui_impact_paths"] = [str(item) for item in ui_paths]
        min_shots = browser_reqs.get("requiredScreenshots")
        if isinstance(min_shots, int) and min_shots > 0:
            policy["min_screenshots"] = min_shots
        # Legacy contract has no explicit videos key; keep default 1.

    pr_harness = contract.get("prReviewHarness", {})
    if isinstance(pr_harness, dict):
        min_shots = pr_harness.get("minScreenshots")
        min_videos = pr_harness.get("minVideos")
        if isinstance(min_shots, int) and min_shots > 0:
            policy["min_screenshots"] = min_shots
        if isinstance(min_videos, int) and min_videos > 0:
            policy["min_videos"] = min_videos
        required_assertions = pr_harness.get("requiredEvidenceAssertions")
        if isinstance(required_assertions, list):
            policy["required_assertions"] = [str(item) for item in required_assertions]

    return policy


def get_rollout_settings(contract: Dict[str, Any]) -> Tuple[str, Dict[str, Any]]:
    rollout = contract.get("rolloutPolicy", {})
    current = str(rollout.get("currentPhase", "phase-0"))
    phase_settings = rollout.get("phases", {}).get(current, {})
    return current, phase_settings


def read_changed_files(changed_files_path: str) -> List[str]:
    path = Path(changed_files_path)
    if not path.exists():
        return []

    files = []
    for raw in path.read_text(encoding="utf-8").splitlines():
        normalized = normalize_path(raw)
        if normalized:
            files.append(normalized)
    return files
