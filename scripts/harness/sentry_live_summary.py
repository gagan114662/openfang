#!/usr/bin/env python3
"""Fetch a trustworthy live Sentry summary for the configured OpenFang project."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, Dict, Iterable, List, Mapping, Optional, Tuple

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    import tomli as tomllib  # type: ignore


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Fetch a live OpenFang Sentry summary")
    parser.add_argument(
        "--config",
        default=str(Path.home() / ".openfang" / "config.toml"),
        help="Path to OpenFang config TOML",
    )
    parser.add_argument("--org", help="Sentry org slug (overrides config)")
    parser.add_argument("--project", help="Sentry project slug (overrides config)")
    parser.add_argument("--base-url", help="Sentry base URL (overrides config)")
    parser.add_argument("--environment", help="Sentry environment filter (overrides config)")
    parser.add_argument("--stats-period", default="24h", help="Sentry stats period, e.g. 24h or 7d")
    parser.add_argument("--limit", type=int, default=10, help="Maximum rows to return in top lists")
    parser.add_argument(
        "--feed-limit",
        type=int,
        default=25,
        help="Issue rows to mirror from the first Sentry issue-feed page",
    )
    parser.add_argument("--format", choices=("json", "text"), default="json", help="Output format")
    parser.add_argument("--out", help="Optional output file path")
    return parser.parse_args()


def load_config(path: str) -> Dict[str, Any]:
    return tomllib.loads(Path(path).read_text(encoding="utf-8"))


def resolve_token(sentry_cfg: Dict[str, Any]) -> str:
    env_name = str(sentry_cfg.get("auth_token_env") or "").strip()
    if env_name:
        token = os.getenv(env_name, "").strip()
        if token:
            return token
    return str(sentry_cfg.get("auth_token") or "").strip()


def sentry_get_json_with_headers(
    base_url: str, path: str, token: str, params: Dict[str, Any]
) -> Tuple[Any, Mapping[str, str]]:
    query = urllib.parse.urlencode({k: v for k, v in params.items() if v is not None}, doseq=True)
    url = f"{base_url.rstrip('/')}{path}"
    if query:
        url = f"{url}?{query}"
    req = urllib.request.Request(
        url,
        headers={
            "Authorization": f"Bearer {token}",
            "Accept": "application/json",
        },
        method="GET",
    )
    with urllib.request.urlopen(req, timeout=30) as response:
        return json.loads(response.read().decode("utf-8")), dict(response.headers.items())


def sentry_get_json(base_url: str, path: str, token: str, params: Dict[str, Any]) -> Any:
    payload, _headers = sentry_get_json_with_headers(base_url, path, token, params)
    return payload


def parse_next_cursor(link_header: str) -> Optional[str]:
    if not link_header:
        return None
    for part in link_header.split(","):
        if 'rel="next"' not in part or 'results="true"' not in part:
            continue
        match = re.search(r'[?&]cursor=([^&>]+)', part)
        if match:
            return urllib.parse.unquote(match.group(1))
    return None


def sentry_get_all_pages(
    base_url: str,
    path: str,
    token: str,
    params: Dict[str, Any],
    *,
    page_limit: int = 20,
) -> List[Dict[str, Any]]:
    rows: List[Dict[str, Any]] = []
    page_params = dict(params)
    for _ in range(page_limit):
        payload, headers = sentry_get_json_with_headers(base_url, path, token, page_params)
        if not isinstance(payload, list):
            break
        rows.extend(item for item in payload if isinstance(item, dict))
        next_cursor = parse_next_cursor(headers.get("Link", ""))
        if not next_cursor:
            break
        page_params["cursor"] = next_cursor
    return rows


def issue_window_count(issue: Dict[str, Any], stats_period: str) -> int:
    stats = (issue.get("stats") or {}).get(stats_period) or []
    total = 0
    for row in stats:
        if not isinstance(row, list) or len(row) < 2:
            continue
        try:
            total += int(row[1] or 0)
        except (TypeError, ValueError):
            continue
    return total


def build_query(*clauses: str, environment: Optional[str] = None) -> str:
    parts = [c.strip() for c in clauses if c and c.strip()]
    if environment:
        parts.append(f"environment:{environment}")
    return " ".join(parts)


def to_int(value: Any) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


def to_float(value: Any) -> Optional[float]:
    try:
        return float(value)
    except (TypeError, ValueError):
        return None


def summarize_issue(issue: Dict[str, Any], stats_period: str) -> Dict[str, Any]:
    return {
        "short_id": str(issue.get("shortId") or ""),
        "title": str(issue.get("title") or ""),
        "status": str(issue.get("status") or ""),
        "level": str(issue.get("level") or ""),
        "last_seen": str(issue.get("lastSeen") or ""),
        "count_total": to_int(issue.get("count")),
        "count_window": issue_window_count(issue, stats_period),
    }


def sort_top(rows: Iterable[Dict[str, Any]], key: str, limit: int) -> List[Dict[str, Any]]:
    return sorted(rows, key=lambda row: row.get(key, 0), reverse=True)[:limit]


def stats_period_window(
    stats_period: str, *, now: Optional[dt.datetime] = None
) -> Tuple[dt.datetime, dt.datetime]:
    match = re.fullmatch(r"(\d+)([hdw])", stats_period.strip().lower())
    if not match:
        raise ValueError(f"Unsupported stats period: {stats_period}")
    amount = int(match.group(1))
    unit = match.group(2)
    multipliers = {
        "h": dt.timedelta(hours=amount),
        "d": dt.timedelta(days=amount),
        "w": dt.timedelta(weeks=amount),
    }
    end = (now or dt.datetime.now(tz=dt.timezone.utc)).astimezone(dt.timezone.utc)
    start = end - multipliers[unit]
    return start, end


def render_text(summary: Dict[str, Any]) -> str:
    lines = [
        f"Sentry summary ({summary['stats_period']})",
        f"- org/project: {summary['org']} / {summary['project']}",
        f"- environment: {summary.get('environment') or 'all'}",
        f"- window_utc: {summary['window']['start']} -> {summary['window']['end']}",
        f"- errors: {summary['errors']['count_24h']}",
        f"- visible groups on feed page: {summary['issues']['visible_groups_feed_page']}",
        f"- visible unresolved groups on feed page: {summary['issues']['visible_unresolved_groups_feed_page']}",
        f"- groups seen in window: {summary['issues']['groups_seen_24h']}",
        f"- unresolved groups seen in window: {summary['issues']['unresolved_groups_seen_24h']}",
        f"- issue events seen in window: {summary['issues']['events_seen_24h']}",
    ]

    tx = summary["transactions"]
    if tx["count_24h"] is None:
        lines.append("- transactions: none returned")
    else:
        lines.append(
            f"- transactions: {tx['count_24h']} (p95={tx['p95_ms']:.1f} ms)"
            if tx["p95_ms"] is not None
            else f"- transactions: {tx['count_24h']}"
        )

    lines.append("- top issue groups by issue-event count:")
    for row in summary["issues"]["top_groups"]:
        lines.append(
            f"  - {row['events_seen_24h']} — {row['title']} "
            f"[{row['status']}/{row['level']}] ({row['short_id']})"
        )

    lines.append("- top transactions by count:")
    for row in summary["transactions"]["top_transactions"]:
        p95 = row.get("p95_ms")
        p95_part = f", p95={p95:.1f} ms" if isinstance(p95, float) else ""
        lines.append(f"  - {row['count_24h']} — {row['transaction']}{p95_part}")

    return "\n".join(lines)


def main() -> int:
    args = parse_args()
    cfg = load_config(args.config)
    sentry_cfg = cfg.get("sentry") or {}

    token = resolve_token(sentry_cfg)
    if not token:
        raise SystemExit("Missing Sentry auth token in config or auth_token_env")

    org = args.org or str(sentry_cfg.get("org_slug") or "").strip()
    project = args.project or str(sentry_cfg.get("project_slug") or "").strip()
    base_url = args.base_url or str(sentry_cfg.get("api_base_url") or "https://sentry.io").strip()
    environment = args.environment or str(sentry_cfg.get("environment") or "").strip() or None

    if not org or not project:
        raise SystemExit("Missing Sentry org/project configuration")

    window_start, window_end = stats_period_window(args.stats_period)

    project_info = sentry_get_json(
        base_url,
        f"/api/0/projects/{org}/{project}/",
        token,
        {},
    )
    project_id = str(project_info.get("id") or "")
    if not project_id:
        raise SystemExit("Failed to resolve Sentry project id")

    feed_page_raw = sentry_get_json(
        base_url,
        f"/api/0/projects/{org}/{project}/issues/",
        token,
        {
            "statsPeriod": args.stats_period,
            "limit": args.feed_limit,
            "query": build_query(environment=environment),
        },
    )
    feed_page_issues = feed_page_raw if isinstance(feed_page_raw, list) else []
    feed_page_seen = [
        summarize_issue(issue, args.stats_period)
        for issue in feed_page_issues
        if issue_window_count(issue, args.stats_period) > 0
    ]
    feed_page_unresolved = [issue for issue in feed_page_seen if issue["status"] == "unresolved"]

    issues_all = sentry_get_all_pages(
        base_url,
        f"/api/0/projects/{org}/{project}/issues/",
        token,
        {
            "statsPeriod": args.stats_period,
            "limit": 100,
            "query": build_query(environment=environment),
        },
    )
    issues_seen = [
        summarize_issue(issue, args.stats_period)
        for issue in issues_all
        if issue_window_count(issue, args.stats_period) > 0
    ]
    unresolved_seen = [issue for issue in issues_seen if issue["status"] == "unresolved"]
    issue_events_seen = sum(issue["count_window"] for issue in issues_seen)
    for issue in issues_seen:
        issue["events_seen_24h"] = issue.pop("count_window")
    for issue in unresolved_seen:
        if "count_window" in issue:
            issue["events_seen_24h"] = issue.pop("count_window")

    errors_raw = sentry_get_json(
        base_url,
        f"/api/0/organizations/{org}/events/",
        token,
        {
            "project": project_id,
            "statsPeriod": args.stats_period,
            "query": build_query("event.type:error", environment=environment),
            "field": ["count()"],
        },
    )
    error_rows = errors_raw.get("data") or []
    error_count = to_int(error_rows[0].get("count()")) if error_rows else 0

    tx_summary_raw = sentry_get_json(
        base_url,
        f"/api/0/organizations/{org}/events/",
        token,
        {
            "project": project_id,
            "statsPeriod": args.stats_period,
            "query": build_query("event.type:transaction", environment=environment),
            "field": ["count()", "p95()"],
        },
    )
    tx_rows = tx_summary_raw.get("data") or []
    tx_count = to_int(tx_rows[0].get("count()")) if tx_rows else None
    tx_p95 = to_float(tx_rows[0].get("p95()")) if tx_rows else None

    top_tx_raw = sentry_get_json(
        base_url,
        f"/api/0/organizations/{org}/events/",
        token,
        {
            "project": project_id,
            "statsPeriod": args.stats_period,
            "query": build_query("event.type:transaction", environment=environment),
            "field": ["transaction", "count()", "p95()"],
            "sort": "-count",
            "per_page": args.limit,
        },
    )
    top_tx_rows = []
    for row in top_tx_raw.get("data") or []:
        top_tx_rows.append(
            {
                "transaction": str(row.get("transaction") or ""),
                "count_24h": to_int(row.get("count()")),
                "p95_ms": to_float(row.get("p95()")),
            }
        )

    summary = {
        "generated_at": dt.datetime.now(tz=dt.timezone.utc).isoformat(),
        "stats_period": args.stats_period,
        "window": {
            "start": window_start.isoformat(),
            "end": window_end.isoformat(),
        },
        "org": org,
        "project": project,
        "project_id": project_id,
        "environment": environment,
        "errors": {
            "count_24h": error_count,
        },
        "issues": {
            "feed_page_limit": args.feed_limit,
            "visible_groups_feed_page": len(feed_page_seen),
            "visible_unresolved_groups_feed_page": len(feed_page_unresolved),
            "groups_seen_24h": len(issues_seen),
            "unresolved_groups_seen_24h": len(unresolved_seen),
            "events_seen_24h": issue_events_seen,
            "top_groups": sort_top(issues_seen, "events_seen_24h", args.limit),
            "top_unresolved_groups": sort_top(unresolved_seen, "events_seen_24h", args.limit),
        },
        "transactions": {
            "count_24h": tx_count,
            "p95_ms": tx_p95,
            "top_transactions": top_tx_rows,
        },
    }

    rendered = (
        json.dumps(summary, indent=2, sort_keys=True)
        if args.format == "json"
        else render_text(summary)
    )

    if args.out:
        out_path = Path(args.out)
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(rendered + "\n", encoding="utf-8")

    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
