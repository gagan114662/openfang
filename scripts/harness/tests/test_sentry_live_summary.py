#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import datetime as dt
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


def test_parse_next_cursor_reads_sentry_link_header() -> None:
    header = (
        '<https://sentry.io/api/0/projects/foolish/openfang-monitoring/issues/'
        '?cursor=0%3A100%3A1>; rel="previous"; results="false"; cursor="0:100:1", '
        '<https://sentry.io/api/0/projects/foolish/openfang-monitoring/issues/'
        '?cursor=0%3A200%3A0>; rel="next"; results="true"; cursor="0:200:0"'
    )
    assert MODULE.parse_next_cursor(header) == "0:200:0"


def test_stats_period_window_uses_exact_utc_range() -> None:
    now = dt.datetime(2026, 3, 18, 14, 30, tzinfo=dt.timezone.utc)
    start, end = MODULE.stats_period_window("24h", now=now)
    assert start == dt.datetime(2026, 3, 17, 14, 30, tzinfo=dt.timezone.utc)
    assert end == now


def test_render_text_includes_feed_page_counts() -> None:
    summary = {
        "stats_period": "24h",
        "org": "foolish",
        "project": "openfang-monitoring",
        "environment": "production",
        "window": {
            "start": "2026-03-17T14:30:00+00:00",
            "end": "2026-03-18T14:30:00+00:00",
        },
        "errors": {"count_24h": 1},
        "issues": {
            "visible_groups_feed_page": 25,
            "visible_unresolved_groups_feed_page": 25,
            "groups_seen_24h": 29,
            "unresolved_groups_seen_24h": 29,
            "events_seen_24h": 901,
            "top_groups": [],
        },
        "transactions": {
            "count_24h": 582,
            "p95_ms": 48928.3,
            "top_transactions": [],
        },
    }
    rendered = MODULE.render_text(summary)
    assert "- visible groups on feed page: 25" in rendered
    assert "- visible unresolved groups on feed page: 25" in rendered
