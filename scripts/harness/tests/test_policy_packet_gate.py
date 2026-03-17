#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
RISK_GATE = REPO_ROOT / "scripts/harness/risk_policy_gate.py"
PR_PACKET = REPO_ROOT / "scripts/harness/pr_packet.py"


class PolicyPacketGateTests(unittest.TestCase):
    def test_checks_resolver_aliases_include_infra_and_live(self) -> None:
        import sys

        sys.path.insert(0, str(REPO_ROOT / "scripts/harness"))
        import checks_resolver  # type: ignore

        self.assertEqual(checks_resolver.CHECK_ALIAS_MAP["infra-preflight"], "infra-preflight")
        self.assertEqual(checks_resolver.CHECK_ALIAS_MAP["agent-evals-live-pr"], "agent-evals-live-pr")

    def test_risk_policy_gate_fails_when_infra_or_live_fail(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            changed = tmp / "changed.txt"
            infra = tmp / "infra.json"
            live = tmp / "live.json"
            report = tmp / "report.json"
            review = tmp / "review.json"
            claude = tmp / "claude.json"

            contract.write_text(
                json.dumps(
                    {
                        "rolloutPolicy": {
                            "currentPhase": "phase-0",
                            "phases": {
                                "phase-0": {
                                    "enforceMergeBlock": True,
                                    "enableRemediation": False
                                }
                            },
                        },
                        "riskTiers": {
                            "critical": {
                                "paths": ["crates/openfang-runtime/**"],
                                "requiredChecks": ["infra-preflight", "agent-evals-live-pr"],
                            },
                            "high": {"paths": [], "requiredChecks": []},
                            "medium": {"paths": [], "requiredChecks": []},
                            "low": {"paths": [], "requiredChecks": []},
                        },
                        "reviewPolicy": {"provider": "greptile", "checkRunName": "greptile-review"},
                        "reviewProviders": {
                            "mode": "alongside",
                            "primary": "greptile",
                            "providers": {"greptile": {"enabled": False}, "claude": {"enabled": False}},
                        },
                        "agentEvalPolicy": {
                            "infraPreflight": {"enabled": True},
                            "liveProviderGate": {"enabled": True, "blockingRiskTiers": ["critical", "high"]},
                        },
                    }
                ),
                encoding="utf-8",
            )
            changed.write_text("crates/openfang-runtime/src/agent_loop.rs\n", encoding="utf-8")
            infra.write_text(json.dumps({"status": "fail", "workflow": "risk-policy-gate"}), encoding="utf-8")
            live.write_text(
                json.dumps(
                    {
                        "status": "fail",
                        "risk_tier": "critical",
                        "blocking_applies": True,
                        "successful_providers": 0,
                        "detected_providers": 1,
                    }
                ),
                encoding="utf-8",
            )
            claude.write_text(json.dumps({"provider": "claude", "status": "missing", "findings": []}), encoding="utf-8")

            env = os.environ.copy()
            env["GITHUB_TOKEN"] = ""
            proc = subprocess.run(
                [
                    "python3",
                    str(RISK_GATE),
                    "--pr",
                    "1",
                    "--head-sha",
                    "deadbeefcafebabe",
                    "--changed-files",
                    str(changed),
                    "--contract",
                    str(contract),
                    "--repo",
                    "owner/repo",
                    "--review-findings",
                    str(review),
                    "--claude-findings",
                    str(claude),
                    "--infra-preflight-report",
                    str(infra),
                    "--live-provider-report",
                    str(live),
                    "--report-out",
                    str(report),
                ],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                env=env,
                check=False,
            )
            self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            payload = json.loads(report.read_text(encoding="utf-8"))
            self.assertEqual(payload["decision"], "fail")
            self.assertEqual(payload["infra_preflight_state"]["status"], "fail")
            self.assertEqual(payload["live_provider_state"]["status"], "fail")

    def test_risk_policy_gate_fails_when_claude_required_and_missing(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            changed = tmp / "changed.txt"
            report = tmp / "report.json"
            review = tmp / "review.json"
            claude = tmp / "claude.json"

            contract.write_text(
                json.dumps(
                    {
                        "rolloutPolicy": {
                            "currentPhase": "phase-0",
                            "phases": {
                                "phase-0": {
                                    "enforceMergeBlock": True,
                                    "enableRemediation": False,
                                    "enforceReviewState": False,
                                }
                            },
                        },
                        "riskTiers": {
                            "low": {
                                "paths": ["**"],
                                "requiredChecks": ["risk-policy-gate"],
                            }
                        },
                        "reviewProviders": {
                            "mode": "alongside",
                            "primary": "greptile",
                            "providers": {
                                "greptile": {"enabled": False, "enforcement": "advisory"},
                                "claude": {"enabled": True, "enforcement": "required"},
                            },
                        },
                    }
                ),
                encoding="utf-8",
            )
            changed.write_text("README.md\n", encoding="utf-8")
            claude.write_text(
                json.dumps(
                    {
                        "provider": "claude",
                        "head_sha": "deadbeefcafebabe",
                        "status": "missing",
                        "findings": [],
                    }
                ),
                encoding="utf-8",
            )

            env = os.environ.copy()
            env["GITHUB_TOKEN"] = ""
            proc = subprocess.run(
                [
                    "python3",
                    str(RISK_GATE),
                    "--pr",
                    "1",
                    "--head-sha",
                    "deadbeefcafebabe",
                    "--changed-files",
                    str(changed),
                    "--contract",
                    str(contract),
                    "--repo",
                    "owner/repo",
                    "--review-findings",
                    str(review),
                    "--claude-findings",
                    str(claude),
                    "--report-out",
                    str(report),
                ],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                env=env,
                check=False,
            )
            self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            payload = json.loads(report.read_text(encoding="utf-8"))
            self.assertEqual(payload["decision"], "fail")
            self.assertEqual(payload["review_states"]["claude"]["status"], "missing")
            self.assertTrue(
                any("claude review state is 'missing'" in reason for reason in payload.get("reasons", []))
            )

    def test_pr_packet_includes_infra_and_live_criteria(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            acceptance = tmp / "acceptance.json"
            contract = tmp / "contract.json"
            changed = tmp / "changed.txt"
            risk = tmp / "risk.json"
            eval_results = tmp / "eval.json"
            evidence = tmp / "evidence.json"
            out_dir = tmp / "out"

            acceptance.write_text(
                json.dumps(
                    {
                        "core": [
                            {"id": "diff_scoped_coherent", "title": "Diff scoped"},
                            {"id": "evidence_package_exists", "title": "Evidence package"},
                            {"id": "minimum_screenshots", "title": "Screens"},
                            {"id": "minimum_videos", "title": "Videos"},
                            {"id": "no_harness_policy_violations", "title": "Policy pass"},
                            {"id": "agent_blocking_evals_pass", "title": "Eval pass"},
                            {"id": "infra_preflight_pass", "title": "Infra pass"},
                        ],
                        "path_rules": [
                            {
                                "name": "live",
                                "when_touched": ["crates/openfang-runtime/**"],
                                "criteria": ["live_provider_gate_pass"],
                            }
                        ],
                        "verifier_map": {
                            "diff_scoped_coherent": "verify_diff_scope",
                            "evidence_package_exists": "verify_evidence_package",
                            "minimum_screenshots": "verify_minimum_screenshots",
                            "minimum_videos": "verify_minimum_videos",
                            "no_harness_policy_violations": "verify_policy_state",
                            "agent_blocking_evals_pass": "verify_agent_blocking_evals_pass",
                            "infra_preflight_pass": "verify_infra_preflight_pass",
                            "live_provider_gate_pass": "verify_live_provider_gate_pass",
                        },
                    }
                ),
                encoding="utf-8",
            )
            contract.write_text(
                json.dumps(
                    {
                        "prReviewHarness": {"requiredCheckName": "pr-review-harness", "maxFiles": 200, "minScreenshots": 2, "minVideos": 1},
                        "agentEvalPolicy": {"blockingCheckName": "agent-evals-pr", "liveProviderGate": {"checkName": "agent-evals-live-pr", "blockingRiskTiers": ["critical", "high"]}},
                        "reviewProviders": {"providers": {}},
                        "reviewPolicy": {"checkRunName": "greptile-review"},
                    }
                ),
                encoding="utf-8",
            )
            changed.write_text("crates/openfang-runtime/src/agent_loop.rs\n", encoding="utf-8")
            risk.write_text(
                json.dumps(
                    {
                        "risk_tier": "critical",
                        "required_checks": [],
                        "decision": "pass",
                        "review_primary": "greptile",
                        "review_states": {},
                        "infra_preflight_state": {"status": "pass", "attempts_used": 1, "transient_failures": 0},
                        "live_provider_state": {"status": "pass", "successful_providers": 1, "detected_providers": 1},
                    }
                ),
                encoding="utf-8",
            )
            eval_results.write_text(
                json.dumps({"profile": "blocking", "summary": {"failed": 0, "total": 1, "all_blocking_passed": True}}),
                encoding="utf-8",
            )
            evidence.write_text(
                json.dumps(
                    {
                        "artifacts": [
                            {"kind": "screenshot", "path": "a.png", "sha256": "a", "size_bytes": 1},
                            {"kind": "screenshot", "path": "b.png", "sha256": "b", "size_bytes": 1},
                            {"kind": "video", "path": "c.mp4", "sha256": "c", "size_bytes": 1},
                        ],
                        "assertions": [],
                    }
                ),
                encoding="utf-8",
            )

            proc = subprocess.run(
                [
                    "python3",
                    str(PR_PACKET),
                    "--acceptance-model",
                    str(acceptance),
                    "--contract",
                    str(contract),
                    "--changed-files",
                    str(changed),
                    "--risk-report",
                    str(risk),
                    "--eval-results",
                    str(eval_results),
                    "--evidence-manifest",
                    str(evidence),
                    "--head-sha",
                    "deadbeefcafebabe",
                    "--out-dir",
                    str(out_dir),
                ],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=False,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            checklist = json.loads((out_dir / "acceptance-checklist.json").read_text(encoding="utf-8"))
            by_id = {item["id"]: item for item in checklist["criteria"]}
            self.assertTrue(by_id["infra_preflight_pass"]["passed"])
            self.assertTrue(by_id["live_provider_gate_pass"]["passed"])


if __name__ == "__main__":
    unittest.main()
