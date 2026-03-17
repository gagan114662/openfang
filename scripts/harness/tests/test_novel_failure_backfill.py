#!/usr/bin/env python3

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "scripts/harness/novel_failure_backfill.py"


class NovelFailureBackfillTests(unittest.TestCase):
    def test_backfill_generates_then_dedupes_candidates(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            generated = tmp / "scenarios.generated.json"
            dedupe = tmp / "novel-failure-signatures.json"
            sentry = tmp / "sentry-findings.json"
            out = tmp / "novel-failure-candidates.json"

            contract.write_text(
                json.dumps(
                    {
                        "agentEvalPolicy": {
                            "novelFailureBackfill": {
                                "enabled": True,
                                "maxNewScenariosPerRun": 25,
                                "minConfidence": 0.7,
                                "targetScenarioFile": str(generated),
                                "dedupeFile": str(dedupe),
                                "autoPr": True,
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            sentry.write_text(
                json.dumps(
                    {
                        "provider": "sentry",
                        "status": "success",
                        "findings": [
                            {
                                "id": "123",
                                "severity": "high",
                                "confidence": 0.95,
                                "path": "crates/openfang-runtime/src/agent_loop.rs",
                                "line": 42,
                                "summary": "agent loop panic on timeout",
                                "actionable": True,
                                "failure_class": "retry_timeout_handling",
                            }
                        ],
                    }
                ),
                encoding="utf-8",
            )

            cmd = [
                "python3",
                str(SCRIPT),
                "--contract",
                str(contract),
                "--sentry-findings",
                str(sentry),
                "--eval-findings",
                str(tmp / "missing-eval.json"),
                "--review-findings",
                str(tmp / "missing-review.json"),
                "--claude-findings",
                str(tmp / "missing-claude.json"),
                "--head-sha",
                "deadbeefcafebabe",
                "--out",
                str(out),
            ]
            first = subprocess.run(cmd, capture_output=True, text=True, cwd=str(REPO_ROOT), check=False)
            self.assertEqual(first.returncode, 0, first.stdout + first.stderr)
            first_out = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(first_out["new_candidates"], 1)

            second = subprocess.run(cmd, capture_output=True, text=True, cwd=str(REPO_ROOT), check=False)
            self.assertEqual(second.returncode, 0, second.stdout + second.stderr)
            second_out = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(second_out["new_candidates"], 0)
            self.assertGreaterEqual(second_out["deduped_candidates"], 1)


if __name__ == "__main__":
    unittest.main()
