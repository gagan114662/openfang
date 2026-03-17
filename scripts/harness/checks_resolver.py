#!/usr/bin/env python3
"""Risk-tier and policy helpers for harness workflows."""

from __future__ import annotations

import fnmatch
import json
from pathlib import Path, PurePosixPath
from typing import Any, Dict, Iterable, List, Tuple


RISK_ORDER = ["critical", "high", "medium", "low"]


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


def compute_risk_tier(changed_files: List[str], risk_tier_rules: Dict[str, List[str]]) -> str:
    if not changed_files:
        return "low"

    for tier in RISK_ORDER:
        patterns = risk_tier_rules.get(tier, [])
        if any_path_matches(changed_files, patterns):
            return tier

    for tier, patterns in risk_tier_rules.items():
        if tier not in RISK_ORDER and any_path_matches(changed_files, patterns):
            return tier

    return "low"


def compute_required_checks(contract: Dict[str, Any], risk_tier: str) -> List[str]:
    merge_policy = contract.get("mergePolicy", {})
    tier_policy = merge_policy.get(risk_tier, {})
    checks = tier_policy.get("requiredChecks", [])
    if checks:
        return [str(check) for check in checks]

    # Backward-compatible fallback for older contract shape.
    risk_tiers = contract.get("riskTiers", {})
    legacy_checks = risk_tiers.get(risk_tier, {}).get("requiredChecks", [])
    return [str(check) for check in legacy_checks]


def evaluate_docs_drift(changed_files: List[str], docs_drift_rules: Any) -> List[str]:
    violations: List[str] = []

    # Backward-compatible fallback for older contract shape.
    if isinstance(docs_drift_rules, dict):
        touched_patterns = [str(item) for item in docs_drift_rules.get("requireDocsUpdate", [])]
        required_any = [str(item) for item in docs_drift_rules.get("docsFiles", [])]
        if touched_patterns and required_any:
            touched = any_path_matches(changed_files, touched_patterns)
            doc_updated = any_path_matches(changed_files, required_any)
            if touched and not doc_updated:
                violations.append(
                    "docs drift rule 'legacy-requireDocsUpdate' violated: "
                    f"changes touched {touched_patterns} but none of {required_any} were updated"
                )
        return violations

    if not isinstance(docs_drift_rules, list):
        return violations

    for rule in docs_drift_rules:
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
    paths = evidence_policy.get("uiImpactPaths", [])
    if not paths:
        # Backward-compatible fallback for older contract shape.
        paths = contract.get("browserEvidenceRequirements", {}).get("uiChangePaths", [])
    return any_path_matches(changed_files, paths)


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
