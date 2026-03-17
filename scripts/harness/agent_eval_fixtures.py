#!/usr/bin/env python3
"""Frozen deterministic fixtures for agent eval scenarios."""

from __future__ import annotations

import hashlib
import json
from typing import Any, Dict


BASE_FIXTURES: Dict[str, Dict[str, Any]] = {
    "routing_correctness": {
        "label": "routing-corpus",
        "content": "deterministic routing fixture: user-default and explicit target resolution",
    },
    "sentry_redaction_invariants": {
        "label": "sentry-redaction-corpus",
        "content": "deterministic redaction fixture: sanitize and never emit raw payload",
    },
    "provider_fallback_degradation": {
        "label": "fallback-corpus",
        "content": "deterministic fallback fixture: primary failover path remains available",
    },
    "concurrency_saturation": {
        "label": "concurrency-corpus",
        "content": "deterministic concurrency fixture: bounded queue pressure and retry budget",
    },
    "flaky_external_probe": {
        "label": "external-probe-corpus",
        "content": "deterministic external probe fixture: advisory-only probes never block",
    },
}


def fixture_for_class(failure_class: str, seed: int) -> Dict[str, Any]:
    base = BASE_FIXTURES.get(
        failure_class,
        {
            "label": "generic-corpus",
            "content": f"deterministic generic fixture for {failure_class}",
        },
    )
    payload = {
        "failure_class": failure_class,
        "seed": seed,
        "label": base["label"],
        "content": base["content"],
    }
    encoded = json.dumps(payload, sort_keys=True).encode("utf-8")
    payload["hash"] = hashlib.sha256(encoded).hexdigest()
    return payload


def fixture_contains(failure_class: str, seed: int, needle: str) -> bool:
    fixture = fixture_for_class(failure_class, seed)
    haystack = f"{fixture.get('label', '')} {fixture.get('content', '')} {fixture.get('hash', '')}".lower()
    return needle.lower() in haystack
