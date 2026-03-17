#!/usr/bin/env python3
"""Continuous Sentry -> Codex -> Claude browser verify -> PR merge loop."""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import time
import tomllib
from pathlib import Path
from typing import Any

DEFAULT_AUTONOMY_REPO_ROOT = Path.home() / ".openfang" / "worktrees" / "autonomy-main"
DEFAULT_AUTONOMY_RUNTIME_DIR = Path.home() / ".openfang" / "autonomy"


def default_source_repo() -> Path:
    return Path(__file__).resolve().parents[1]


def resolve_default_repo_root() -> Path:
    env_repo_root = os.getenv("OPENFANG_AUTONOMY_REPO_ROOT", "").strip()
    if env_repo_root:
        return Path(env_repo_root).expanduser().resolve()
    if DEFAULT_AUTONOMY_REPO_ROOT.exists():
        return DEFAULT_AUTONOMY_REPO_ROOT.resolve()
    return default_source_repo()


def resolve_repo_script(repo: Path, relative_path: str) -> Path:
    candidate = (repo / relative_path).resolve()
    if candidate.exists():
        return candidate
    fallback = (default_source_repo() / relative_path).resolve()
    return fallback


def resolve_runtime_dir() -> Path:
    env_runtime_dir = os.getenv("OPENFANG_AUTONOMY_RUNTIME_DIR", "").strip()
    if env_runtime_dir:
        return Path(env_runtime_dir).expanduser().resolve()
    return DEFAULT_AUTONOMY_RUNTIME_DIR.resolve()


def resolve_runtime_path(runtime_dir: Path, raw_path: str) -> Path:
    path = Path(raw_path).expanduser()
    if path.is_absolute():
        return path.resolve()
    return (runtime_dir / path).resolve()


def resolve_paperclip_findings_path(runtime_dir: Path, raw_path: str) -> Path:
    env_path = os.getenv("OPENFANG_AUTONOMY_PAPERCLIP_FINDINGS_PATH", "").strip()
    if env_path:
        return Path(env_path).expanduser().resolve()
    configured = Path(raw_path).expanduser()
    if configured.is_absolute():
        return configured.resolve()
    legacy_source_path = (default_source_repo() / configured).resolve()
    if legacy_source_path.exists():
        return legacy_source_path
    return (runtime_dir / configured.name).resolve()


def load_configured_sentry_auth_token() -> str:
    if os.getenv("SENTRY_AUTH_TOKEN", "").strip():
        return os.environ["SENTRY_AUTH_TOKEN"].strip()
    config_candidates = [
        Path.home() / ".openfang" / "config.toml",
        Path.home() / ".openfang-local-computer-use" / "config.toml",
    ]
    for path in config_candidates:
        if not path.exists():
            continue
        try:
            payload = tomllib.loads(path.read_text(encoding="utf-8"))
        except Exception:
            continue
        sentry_cfg = payload.get("sentry")
        if isinstance(sentry_cfg, dict):
            token = str(sentry_cfg.get("auth_token") or "").strip()
            if token:
                os.environ["SENTRY_AUTH_TOKEN"] = token
                return token
    return ""


def load_codex_auth_b64() -> str:
    for env_name in ("CODEX_AUTH_JSON_B64_PRIMARY", "CODEX_AUTH_JSON_B64"):
        value = os.getenv(env_name, "").strip()
        if value:
            return value
    auth_path = Path.home() / ".codex" / "auth.json"
    if not auth_path.exists():
        return ""
    try:
        encoded = base64.b64encode(auth_path.read_bytes()).decode("ascii")
    except Exception:
        return ""
    if encoded:
        os.environ["CODEX_AUTH_JSON_B64_PRIMARY"] = encoded
    return encoded


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Run the unattended OpenFang remediation loop")
    parser.add_argument("--repo-root", default=None, help="OpenFang repo root (defaults to dedicated autonomy worktree)")
    parser.add_argument(
        "--runtime-dir",
        default=None,
        help="Directory for loop state/findings/guard artifacts (defaults to ~/.openfang/autonomy)",
    )
    parser.add_argument("--api-base", default="http://127.0.0.1:50051", help="Local OpenFang API base")
    parser.add_argument("--dashboard-base", default="http://127.0.0.1:4200", help="Dashboard URL for browser verification")
    parser.add_argument("--sleep-secs", type=int, default=1800, help="Loop interval for continuous mode")
    parser.add_argument("--sentry-org", default=os.getenv("SENTRY_ORG", os.getenv("OPENFANG_SENTRY_ORG", "")))
    parser.add_argument(
        "--sentry-project",
        default=os.getenv("SENTRY_PROJECT", os.getenv("OPENFANG_SENTRY_PROJECT", "")),
    )
    parser.add_argument("--sentry-query", default="is:unresolved level:error")
    parser.add_argument("--sentry-limit", type=int, default=20)
    parser.add_argument("--state-path", default="loop-state.json")
    parser.add_argument("--findings-path", default="sentry-findings.json")
    parser.add_argument(
        "--paperclip-findings-path",
        default="artifacts/autonomy/paperclip-findings.json",
        help="Staged Paperclip findings merged into the remediation queue",
    )
    parser.add_argument("--guard-report-path", default="vacation-guard-latest.json")
    parser.add_argument("--guard-history-dir", default="vacation-guard-history")
    parser.add_argument("--kill-switch", default=str(Path.home() / ".openfang" / "autonomy.lock"))
    parser.add_argument("--max-per-cycle", type=int, default=5)
    parser.add_argument("--max-per-day", type=int, default=10)
    parser.add_argument("--max-diff-lines", type=int, default=500)
    parser.add_argument("--once", action="store_true", help="Run one cycle and exit")
    parser.add_argument("--dry-run", action="store_true", help="Do not mutate git/GitHub state")
    parser.add_argument("--skip-browser-verify", action="store_true")
    parser.add_argument("--skip-vacation-guard", action="store_true")
    parser.add_argument("--base-branch", default=os.getenv("OPENFANG_AUTONOMY_BASE_BRANCH", "main"))
    parser.add_argument(
        "--validation-cmd",
        action="append",
        default=[],
        help="Validation command(s) to run after Codex applies changes",
    )
    parser.add_argument(
        "--codex-apply-cmd",
        default=(
            "codex exec --json --full-auto "
            "'Read .sentry-findings-context.md and fix the listed issue with the smallest safe change. "
            "Only edit files under crates/. Run cargo build --workspace --lib after the fix and summarize the result.'"
        ),
        help="Shell command used by remediation_runner via codex_failover_runner",
    )
    parser.add_argument(
        "--claude-browser-cmd",
        default="claude -p --output-format json --chrome",
        help="Claude browser command prefix used for visual verification",
    )
    return parser.parse_args()


def now_utc() -> dt.datetime:
    return dt.datetime.now(tz=dt.timezone.utc)


def emit(kind: str, level: str = "info", **fields: Any) -> None:
    payload = {
        "ts": now_utc().isoformat(),
        "level": level,
        "event.kind": kind,
    }
    payload.update(fields)
    print(json.dumps(payload, sort_keys=True), flush=True)


def run(
    cmd: list[str],
    *,
    cwd: Path,
    check: bool = False,
    timeout: int | None = None,
    env: dict[str, str] | None = None,
) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        cmd,
        cwd=str(cwd),
        text=True,
        capture_output=True,
        check=False,
        timeout=timeout,
        env=env,
    )
    if check and proc.returncode != 0:
        raise RuntimeError(f"command failed ({proc.returncode}): {' '.join(cmd)}\n{proc.stderr or proc.stdout}")
    return proc


def shell(command: str, *, cwd: Path, check: bool = False, timeout: int | None = None) -> subprocess.CompletedProcess[str]:
    proc = subprocess.run(
        command,
        cwd=str(cwd),
        text=True,
        capture_output=True,
        check=False,
        timeout=timeout,
        shell=True,
    )
    if check and proc.returncode != 0:
        raise RuntimeError(f"shell failed ({proc.returncode}): {command}\n{proc.stderr or proc.stdout}")
    return proc


def load_json(path: Path, default: dict[str, Any]) -> dict[str, Any]:
    if not path.exists():
        return default
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(raw, dict):
            return raw
    except Exception:
        pass
    return default


def write_json(path: Path, payload: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def default_state() -> dict[str, Any]:
    return {
        "processed_issues": {},
        "prs_today": 0,
        "prs_day": now_utc().date().isoformat(),
        "last_loop": None,
        "consecutive_failures": 0,
    }


def reset_daily_counter(state: dict[str, Any]) -> None:
    today = now_utc().date().isoformat()
    if state.get("prs_day") != today:
        state["prs_day"] = today
        state["prs_today"] = 0


def git_output(repo: Path, *args: str) -> str:
    proc = run(["git", *args], cwd=repo, check=True)
    return proc.stdout.strip()


def sanitize_slug(value: str) -> str:
    slug = re.sub(r"[^a-z0-9_-]+", "-", value.lower()).strip("-")
    return slug or "finding"


def branch_name_for(finding: dict[str, Any]) -> str:
    source = finding.get("source") or {}
    short_id = str(source.get("short_id") or finding.get("id") or "finding")
    return f"codex/sentry-fix-{sanitize_slug(short_id)}"


def build_context(findings: list[dict[str, Any]]) -> str:
    lines = [
        "You are running the unattended OpenFang remediation loop.",
        "Fix only the actionable finding(s) listed below with minimal, safe edits.",
        "Hard constraints:",
        "- Do not edit .github/, scripts/, config/, or .claude/ in this remediation.",
        "- Keep the diff under the configured safety threshold.",
        "- Do not broaden scope beyond the listed issue(s).",
        "",
        "Actionable findings:",
    ]
    for finding in findings:
        source = finding.get("source") or {}
        lines.append(
            "- [{severity}] {summary} ({path}:{line}) [{short_id}]".format(
                severity=finding.get("severity", "medium"),
                summary=finding.get("summary", "Sentry issue"),
                path=finding.get("path") or "unknown",
                line=finding.get("line") or 1,
                short_id=source.get("short_id") or finding.get("id") or "finding",
            )
        )
    lines.extend(
        [
            "",
            "Return only implemented code changes and brief verification notes.",
        ]
    )
    return "\n".join(lines) + "\n"


def collect_findings(args: argparse.Namespace, repo: Path, findings_path: Path) -> dict[str, Any]:
    token = load_configured_sentry_auth_token()
    env = os.environ.copy()
    if token:
        env["SENTRY_AUTH_TOKEN"] = token
    cmd = [
        sys.executable,
        str(resolve_repo_script(repo, "scripts/harness/sentry_findings.py")),
        "--org",
        args.sentry_org,
        "--project",
        args.sentry_project,
        "--query",
        args.sentry_query,
        "--limit",
        str(args.sentry_limit),
        "--out",
        str(findings_path),
    ]
    if not args.sentry_org or not args.sentry_project:
        raise RuntimeError("missing --sentry-org or --sentry-project")
    proc = run(cmd, cwd=repo, env=env)
    payload = load_json(findings_path, {"status": "error", "findings": [], "errors": ["missing findings payload"]})
    if proc.returncode != 0 or payload.get("status") != "success":
        raise RuntimeError("; ".join(payload.get("errors") or [proc.stderr.strip() or proc.stdout.strip() or "unknown sentry_findings failure"]))
    return payload


def collect_staged_paperclip_findings(repo: Path, findings_path: Path) -> dict[str, Any]:
    payload = load_json(
        findings_path,
        {"status": "missing", "provider": "paperclip", "findings": [], "errors": []},
    )
    if payload.get("status") != "success":
        return {"status": "missing", "provider": "paperclip", "findings": [], "errors": []}
    findings = payload.get("findings", [])
    if not isinstance(findings, list):
        findings = []
    return {
        "status": "success",
        "provider": "paperclip",
        "generated_at": payload.get("generated_at"),
        "findings": findings,
        "errors": payload.get("errors", []),
    }


def run_vacation_guard(args: argparse.Namespace, repo: Path, report_path: Path, history_dir: Path) -> None:
    cmd = [
        sys.executable,
        str(resolve_repo_script(repo, "scripts/harness/vacation_guard.py")),
        "--api-base",
        args.api_base,
        "--out",
        str(report_path),
        "--history-dir",
        str(history_dir),
        "--enforce-single-poller",
    ]
    proc = run(cmd, cwd=repo)
    emit(
        "autonomy.guard.completed",
        "info" if proc.returncode == 0 else "error",
        status_code=proc.returncode,
        report_path=str(report_path),
    )


def ensure_clean_worktree(repo: Path) -> None:
    proc = run(["git", "status", "--porcelain"], cwd=repo, check=True)
    if proc.stdout.strip():
        raise RuntimeError("working tree is dirty; autonomy loop requires a clean checkout")


def write_subset_findings(runtime_dir: Path, finding: dict[str, Any]) -> Path:
    payload = {
        "provider": "sentry",
        "status": "success",
        "findings": [finding],
        "errors": [],
    }
    runtime_dir.mkdir(parents=True, exist_ok=True)
    fd, path_str = tempfile.mkstemp(prefix="autonomy-finding-", suffix=".json", dir=str(runtime_dir))
    os.close(fd)
    path = Path(path_str)
    write_json(path, payload)
    return path


def remediation_result_path(runtime_dir: Path, branch: str) -> Path:
    return runtime_dir / f"{sanitize_slug(branch)}-remediation-result.json"


def failover_result_path(runtime_dir: Path, branch: str) -> Path:
    return runtime_dir / f"{sanitize_slug(branch)}-failover-result.json"


def git_diff_line_count(repo: Path) -> int:
    proc = run(["git", "diff", "--shortstat"], cwd=repo, check=True)
    text = proc.stdout.strip()
    match = re.search(r"(\d+)\s+insertions?\(\+\).*(\d+)\s+deletions?\(-\)", text)
    if match:
        return int(match.group(1)) + int(match.group(2))
    match = re.search(r"(\d+)\s+insertions?\(\+\)", text)
    if match:
        return int(match.group(1))
    match = re.search(r"(\d+)\s+deletions?\(-\)", text)
    if match:
        return int(match.group(1))
    return 0


def sync_base_ref(repo: Path, base_branch: str) -> str:
    remote_ref = f"origin/{base_branch}"
    run(["git", "fetch", "origin", base_branch], cwd=repo, check=True)
    run(["git", "checkout", "--detach", remote_ref], cwd=repo, check=True)
    return remote_ref


def create_pr(repo: Path, branch: str, base_branch: str, finding: dict[str, Any]) -> str:
    title = finding.get("summary") or "Sentry remediation"
    short_id = (finding.get("source") or {}).get("short_id") or finding.get("id") or branch
    body = "\n".join(
        [
            f"Automated Sentry remediation for `{short_id}`.",
            "",
            f"- Summary: {title}",
            f"- Path: `{finding.get('path') or 'unknown'}`",
            f"- Line: `{finding.get('line') or 1}`",
        ]
    )
    proc = run(
        ["gh", "pr", "create", "--base", base_branch, "--head", branch, "--title", title, "--body", body],
        cwd=repo,
        check=True,
    )
    return proc.stdout.strip().splitlines()[-1].strip()


def verify_dashboard(args: argparse.Namespace, repo: Path, branch: str) -> dict[str, Any]:
    prompt = (
        f"Open {args.dashboard_base}/ and verify the OpenFang dashboard on branch {branch}. "
        "Check: 1. the dashboard loads, 2. agents page renders, 3. health page is green or clearly reports blockers. "
        "Take screenshots if needed. Return strict JSON {\"pass\": true|false, \"issues\": [..], \"notes\": \"...\"}."
    )
    cmd = shlex.split(args.claude_browser_cmd) + [prompt]
    proc = run(cmd, cwd=repo, timeout=300)
    if proc.returncode != 0:
        return {"pass": False, "issues": [proc.stderr.strip() or proc.stdout.strip() or "claude browser verification failed"]}
    raw = proc.stdout.strip()
    try:
        return json.loads(raw)
    except json.JSONDecodeError:
        for line in reversed(raw.splitlines()):
            line = line.strip()
            if not line:
                continue
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                continue
    return {"pass": False, "issues": ["claude browser verification returned non-JSON output"], "notes": raw[-500:]}


def wait_for_checks(repo: Path, pr_url: str) -> None:
    run(["gh", "pr", "checks", pr_url, "--watch"], cwd=repo, check=True, timeout=1800)


def merge_pr(repo: Path, pr_url: str) -> None:
    run(["gh", "pr", "merge", pr_url, "--squash", "--auto"], cwd=repo, check=True, timeout=300)


def process_finding(
    args: argparse.Namespace,
    repo: Path,
    runtime_dir: Path,
    state: dict[str, Any],
    finding: dict[str, Any],
) -> dict[str, Any]:
    short_id = str((finding.get("source") or {}).get("short_id") or finding.get("id") or "finding")
    branch = branch_name_for(finding)
    result: dict[str, Any] = {
        "issue": short_id,
        "branch": branch,
        "status": "skipped",
        "pr_url": "",
        "verification": None,
    }
    if short_id in state["processed_issues"]:
        result["status"] = "already_processed"
        return result

    if state["prs_today"] >= args.max_per_day:
        result["status"] = "daily_limit_reached"
        return result

    ensure_clean_worktree(repo)
    remote_base_ref = sync_base_ref(repo, args.base_branch)
    run(["git", "checkout", "-B", branch], cwd=repo, check=True)

    context_path = repo / ".sentry-findings-context.md"
    subset_path = write_subset_findings(runtime_dir, finding)
    result_path = remediation_result_path(runtime_dir, branch)
    failover_path = failover_result_path(runtime_dir, branch)
    attempt_log = runtime_dir / "attempt-log.json"
    context_path.write_text(build_context([finding]), encoding="utf-8")

    validation_cmds = args.validation_cmd or [
        "cargo build --workspace --lib",
        "cargo test --workspace",
        "cargo clippy --workspace --all-targets -- -D warnings",
    ]

    try:
        if args.dry_run:
            emit(
                "autonomy.finding.dry_run",
                issue=short_id,
                branch=branch,
                summary=finding.get("summary"),
            )
            result["status"] = "dry_run"
            return result

        cmd = [
            sys.executable,
            str(resolve_repo_script(repo, "scripts/harness/codex_failover_runner.py")),
            "--findings",
            str(subset_path),
            "--head-sha",
            git_output(repo, "rev-parse", "HEAD"),
            "--apply-cmd",
            args.codex_apply_cmd,
            "--attempt-log",
            str(attempt_log),
            "--result-out",
            str(result_path),
            "--failover-out",
            str(failover_path),
        ]
        for item in validation_cmds:
            cmd.extend(["--validation-cmd", item])
        env = os.environ.copy()
        codex_auth_b64 = load_codex_auth_b64()
        if codex_auth_b64:
            env["CODEX_AUTH_JSON_B64_PRIMARY"] = codex_auth_b64
        run(cmd, cwd=repo, check=True, timeout=7200, env=env)

        diff_lines = git_diff_line_count(repo)
        if diff_lines > args.max_diff_lines:
            raise RuntimeError(f"diff too large for auto-merge gate: {diff_lines} > {args.max_diff_lines}")

        commit_msg = f"fix: remediate Sentry issue {short_id}"
        run(["git", "add", "-A"], cwd=repo, check=True)
        run(["git", "commit", "-m", commit_msg], cwd=repo, check=True)
        run(["git", "push", "-u", "origin", branch], cwd=repo, check=True, timeout=900)
        pr_url = create_pr(repo, branch, args.base_branch, finding)
        result["pr_url"] = pr_url
        result["status"] = "pr_opened"
        state["prs_today"] += 1

        wait_for_checks(repo, pr_url)
        if not args.skip_browser_verify:
            verification = verify_dashboard(args, repo, branch)
            result["verification"] = verification
            if not verification.get("pass"):
                raise RuntimeError(f"claude browser verification failed: {verification.get('issues')}")

        merge_pr(repo, pr_url)
        result["status"] = "merged"
        return result
    finally:
        for path in (context_path, subset_path):
            try:
                if path.exists():
                    path.unlink()
            except OSError:
                pass
        try:
            run(["git", "reset", "--hard", "HEAD"], cwd=repo)
            run(["git", "clean", "-fd"], cwd=repo)
            run(["git", "checkout", "--detach", remote_base_ref], cwd=repo)
        except Exception:
            pass


def loop_once(args: argparse.Namespace, repo: Path, runtime_dir: Path, state_path: Path) -> int:
    state = load_json(state_path, default_state())
    reset_daily_counter(state)
    state["last_loop"] = now_utc().isoformat()
    write_json(state_path, state)
    load_configured_sentry_auth_token()

    if Path(args.kill_switch).exists():
        emit("autonomy.loop.paused", reason="kill_switch_present", path=args.kill_switch)
        return 0

    if not args.skip_vacation_guard:
        run_vacation_guard(
            args,
            repo,
            resolve_runtime_path(runtime_dir, args.guard_report_path),
            resolve_runtime_path(runtime_dir, args.guard_history_dir),
        )

    paperclip_payload = collect_staged_paperclip_findings(repo, resolve_paperclip_findings_path(runtime_dir, args.paperclip_findings_path))
    sentry_error = None
    try:
        findings_payload = collect_findings(args, repo, resolve_runtime_path(runtime_dir, args.findings_path))
    except Exception as exc:
        sentry_error = str(exc)
        findings_payload = {
            "status": "error",
            "provider": "sentry",
            "findings": [],
            "errors": [sentry_error],
        }
        emit("autonomy.findings.sentry_failed", "error", error=sentry_error)

    combined_findings = list(findings_payload.get("findings", []))
    combined_findings.extend(paperclip_payload.get("findings", []))
    actionable = [item for item in combined_findings if item.get("actionable")]
    actionable = actionable[: args.max_per_cycle]

    emit(
        "autonomy.findings.collected",
        count=len(combined_findings),
        actionable_count=len(actionable),
        query=args.sentry_query,
        sentry_count=len(findings_payload.get("findings", [])),
        paperclip_count=len(paperclip_payload.get("findings", [])),
        sentry_error=sentry_error or "",
    )

    if not actionable:
        if sentry_error and not paperclip_payload.get("findings"):
            raise RuntimeError(sentry_error)
        state["consecutive_failures"] = 0
        write_json(state_path, state)
        return 0

    failures = 0
    for finding in actionable:
        short_id = str((finding.get("source") or {}).get("short_id") or finding.get("id") or "finding")
        try:
            outcome = process_finding(args, repo, runtime_dir, state, finding)
            state["processed_issues"][short_id] = {
                "status": outcome["status"],
                "branch": outcome["branch"],
                "pr_url": outcome.get("pr_url", ""),
                "updated_at": now_utc().isoformat(),
            }
            emit(
                "autonomy.finding.completed",
                issue=short_id,
                status=outcome["status"],
                branch=outcome["branch"],
                pr_url=outcome.get("pr_url", ""),
            )
        except Exception as exc:
            failures += 1
            state["processed_issues"][short_id] = {
                "status": "failed",
                "error": str(exc),
                "updated_at": now_utc().isoformat(),
            }
            emit("autonomy.finding.failed", "error", issue=short_id, error=str(exc))
        finally:
            write_json(state_path, state)

    state["consecutive_failures"] = failures if failures else 0
    write_json(state_path, state)
    return 1 if failures else 0


def main() -> int:
    args = parse_args()
    repo = Path(args.repo_root).expanduser().resolve() if args.repo_root else resolve_default_repo_root()
    runtime_dir = Path(args.runtime_dir).expanduser().resolve() if args.runtime_dir else resolve_runtime_dir()
    runtime_dir.mkdir(parents=True, exist_ok=True)
    state_path = resolve_runtime_path(runtime_dir, args.state_path)
    emit(
        "autonomy.loop.start",
        repo_root=str(repo),
        runtime_dir=str(runtime_dir),
        dry_run=args.dry_run,
        once=args.once,
        sleep_secs=args.sleep_secs,
    )

    while True:
        try:
            code = loop_once(args, repo, runtime_dir, state_path)
        except Exception as exc:
            emit("autonomy.loop.failed", "error", error=str(exc))
            code = 1
        if args.once:
            return code
        time.sleep(args.sleep_secs)


if __name__ == "__main__":
    raise SystemExit(main())
