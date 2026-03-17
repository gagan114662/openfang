#!/usr/bin/env python3

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
RUNNER = REPO_ROOT / "scripts/harness/agent_eval_runner.py"


class AgentEvalRunnerTests(unittest.TestCase):
    def _write_contract(self, path: Path) -> None:
        payload = {
            "agentEvalPolicy": {
                "enabled": True,
                "blockingThreshold": 1.0,
                "maxScenarioRuntimeSecs": 2,
                "maxRemediationFindingsPerRun": 5,
                "blockingProfile": {
                    "llm_mode": "mock_frozen",
                    "network_mode": "isolated",
                    "seed": 42,
                },
                "nightlyProfile": {
                    "llm_mode": "mixed_advisory",
                    "network_mode": "restricted",
                    "seed": 42,
                },
            }
        }
        path.write_text(json.dumps(payload), encoding="utf-8")

    def _run(
        self,
        *,
        scenarios: Path,
        contract: Path,
        out_dir: Path,
        seed: int = 42,
        enforce: str = "true",
        generated_scenarios: Path | None = None,
    ) -> subprocess.CompletedProcess[str]:
        cmd = [
            "python3",
            str(RUNNER),
            "--scenarios",
            str(scenarios),
            "--contract",
            str(contract),
            "--head-sha",
            "deadbeefcafebabe",
            "--profile",
            "blocking",
            "--seed",
            str(seed),
            "--out-dir",
            str(out_dir),
            "--enforce",
            enforce,
            "--repo-root",
            str(REPO_ROOT),
            "--max-scenario-runtime-secs",
            "2",
        ]
        if generated_scenarios is not None:
            cmd.extend(["--generated-scenarios", str(generated_scenarios)])
        return subprocess.run(cmd, capture_output=True, text=True, cwd=str(REPO_ROOT), check=False)

    def test_deterministic_replay_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            scenarios = tmp / "scenarios.json"
            out1 = tmp / "out1"
            out2 = tmp / "out2"
            self._write_contract(contract)
            scenarios.write_text(
                json.dumps(
                    {
                        "scenarios": [
                            {
                                "id": "fixture-a",
                                "name": "fixture",
                                "tier": "blocking",
                                "surface": "runtime",
                                "setup": {},
                                "stimulus": {"kind": "fixture_lookup", "fixture": "routing_correctness"},
                                "expected": {"kind": "fixture_contains", "contains": "deterministic"},
                                "judge": {"kind": "fixture_contains", "fixture": "routing_correctness", "contains": "deterministic"},
                                "failure_class": "routing_correctness",
                                "remediable": False,
                                "owner": "runtime",
                                "timeout_secs": 2,
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )

            p1 = self._run(scenarios=scenarios, contract=contract, out_dir=out1, seed=42)
            p2 = self._run(scenarios=scenarios, contract=contract, out_dir=out2, seed=42)
            self.assertEqual(p1.returncode, 0, p1.stderr)
            self.assertEqual(p2.returncode, 0, p2.stderr)

            r1 = json.loads((out1 / "eval-results.json").read_text(encoding="utf-8"))
            r2 = json.loads((out2 / "eval-results.json").read_text(encoding="utf-8"))
            hashes1 = [item["deterministic_hash"] for item in r1["results"]]
            hashes2 = [item["deterministic_hash"] for item in r2["results"]]
            self.assertEqual(hashes1, hashes2)

    def test_schema_shape_failure_exits_nonzero(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            scenarios = tmp / "scenarios.json"
            out_dir = tmp / "out"
            self._write_contract(contract)
            scenarios.write_text(json.dumps({"not_scenarios": []}), encoding="utf-8")
            proc = self._run(scenarios=scenarios, contract=contract, out_dir=out_dir)
            self.assertNotEqual(proc.returncode, 0)

    def test_timeout_generates_failure(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            scenarios = tmp / "scenarios.json"
            out_dir = tmp / "out"
            self._write_contract(contract)
            scenarios.write_text(
                json.dumps(
                    {
                        "scenarios": [
                            {
                                "id": "timeout-case",
                                "name": "timeout",
                                "tier": "blocking",
                                "surface": "harness",
                                "setup": {},
                                "stimulus": {"kind": "command"},
                                "expected": {"kind": "command_exit", "expected_exit": 0},
                                "judge": {"kind": "command_exit", "command": "python3 -c 'import time; time.sleep(3)'", "expected_exit": 0},
                                "failure_class": "retry_timeout_handling",
                                "remediable": True,
                                "owner": "harness",
                                "timeout_secs": 1,
                                "finding": {
                                    "path": "scripts/harness/agent_eval_runner.py",
                                    "line": 1,
                                    "summary": "timeout scenario failed"
                                }
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            proc = self._run(scenarios=scenarios, contract=contract, out_dir=out_dir, enforce="false")
            self.assertEqual(proc.returncode, 0, proc.stderr)
            results = json.loads((out_dir / "eval-results.json").read_text(encoding="utf-8"))
            self.assertEqual(results["summary"]["failed"], 1)
            self.assertIn("timeout", results["results"][0]["failure_reason"])

    def test_remediable_failures_create_findings(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            scenarios = tmp / "scenarios.json"
            out_dir = tmp / "out"
            self._write_contract(contract)
            scenarios.write_text(
                json.dumps(
                    {
                        "scenarios": [
                            {
                                "id": "missing-file",
                                "name": "missing",
                                "tier": "blocking",
                                "surface": "harness",
                                "setup": {},
                                "stimulus": {"kind": "filesystem", "path": "missing.txt"},
                                "expected": {"kind": "file_exists", "path": "missing.txt"},
                                "judge": {"kind": "file_exists", "path": "missing.txt"},
                                "failure_class": "tool_guardrails",
                                "remediable": True,
                                "owner": "harness",
                                "timeout_secs": 1,
                                "severity": "high",
                                "finding": {
                                    "path": "scripts/harness/agent_eval_runner.py",
                                    "line": 1,
                                    "summary": "required file missing"
                                }
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )

            proc = self._run(scenarios=scenarios, contract=contract, out_dir=out_dir, enforce="false")
            self.assertEqual(proc.returncode, 0, proc.stderr)
            findings = json.loads((out_dir / "eval-findings.json").read_text(encoding="utf-8"))
            self.assertEqual(findings["provider"], "eval")
            self.assertEqual(len(findings["findings"]), 1)
            self.assertTrue(findings["findings"][0]["actionable"])

    def test_generated_scenarios_merged_deterministically(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            scenarios = tmp / "scenarios.json"
            generated = tmp / "generated.json"
            out1 = tmp / "out1"
            out2 = tmp / "out2"
            self._write_contract(contract)
            scenarios.write_text(
                json.dumps(
                    {
                        "scenarios": [
                            {
                                "id": "z-base",
                                "name": "base z",
                                "tier": "blocking",
                                "surface": "runtime",
                                "setup": {},
                                "stimulus": {"kind": "filesystem", "path": "scripts/harness/agent_eval_runner.py"},
                                "expected": {"kind": "file_exists", "path": "scripts/harness/agent_eval_runner.py"},
                                "judge": {"kind": "file_exists", "path": "scripts/harness/agent_eval_runner.py"},
                                "failure_class": "routing_correctness",
                                "remediable": False,
                                "owner": "runtime",
                                "timeout_secs": 2
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )
            generated.write_text(
                json.dumps(
                    {
                        "scenarios": [
                            {
                                "id": "a-generated",
                                "name": "generated a",
                                "tier": "nightly",
                                "surface": "harness",
                                "setup": {},
                                "stimulus": {"kind": "filesystem", "path": "scripts/harness/agent_eval_runner.py"},
                                "expected": {"kind": "file_exists", "path": "scripts/harness/agent_eval_runner.py"},
                                "judge": {"kind": "file_exists", "path": "scripts/harness/agent_eval_runner.py"},
                                "failure_class": "novel_failure",
                                "remediable": False,
                                "owner": "harness",
                                "timeout_secs": 2
                            }
                        ]
                    }
                ),
                encoding="utf-8",
            )

            p1 = self._run(
                scenarios=scenarios,
                contract=contract,
                out_dir=out1,
                seed=42,
                generated_scenarios=generated,
            )
            p2 = self._run(
                scenarios=scenarios,
                contract=contract,
                out_dir=out2,
                seed=42,
                generated_scenarios=generated,
            )
            self.assertEqual(p1.returncode, 0, p1.stderr)
            self.assertEqual(p2.returncode, 0, p2.stderr)
            r1 = json.loads((out1 / "eval-results.json").read_text(encoding="utf-8"))
            r2 = json.loads((out2 / "eval-results.json").read_text(encoding="utf-8"))
            ids1 = [item["scenario_id"] for item in r1["results"]]
            ids2 = [item["scenario_id"] for item in r2["results"]]
            self.assertEqual(ids1, ids2)
            self.assertEqual(ids1, sorted(ids1))


if __name__ == "__main__":
    unittest.main()
