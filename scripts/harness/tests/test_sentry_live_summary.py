#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
from pathlib import Path


MODULE_PATH = Path(__file__).resolve().parents[1] / "sentry_live_summary.py"
SPEC = importlib.util.spec_from_file_location("sentry_live_summary", MODULE_PATH)
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC is not None and SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def test_issue_window_count_sums_rows() -> None:
    issue = {"stats": {"24h": [[1, 2], [2, 0], [3, 5]]}}
    assert MODULE.issue_window_count(issue, "24h") == 7


def test_issue_window_count_ignores_bad_rows() -> None:
    issue = {"stats": {"24h": [[1], "bad", [2, "3"]]}}
    assert MODULE.issue_window_count(issue, "24h") == 3


def test_build_query_appends_environment() -> None:
    query = MODULE.build_query("event.type:transaction", "foo:bar", environment="production")
    assert query == "event.type:transaction foo:bar environment:production"


def test_summarize_issue_uses_window_count() -> None:
    issue = {
        "shortId": "OPENFANG-1",
        "title": "api.request",
        "status": "unresolved",
        "level": "info",
        "lastSeen": "2026-03-18T12:45:54Z",
        "count": "574",
        "stats": {"24h": [[1, 4], [2, 6]]},
    }
    summary = MODULE.summarize_issue(issue, "24h")
    assert summary["count_total"] == 574
    assert summary["count_window"] == 10
    assert summary["short_id"] == "OPENFANG-1"
