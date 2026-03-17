#!/usr/bin/env python3

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "scripts/harness/live_provider_probe.py"


def _write_contract(path: Path) -> None:
    payload = {
        "agentEvalPolicy": {
            "liveProviderGate": {
                "enabled": True,
                "blockingRiskTiers": ["critical", "high"],
                "minSuccessfulProviders": 1,
                "failIfNoProviderSecrets": True,
                "retries": {"attempts": 1, "backoffSeconds": [0]},
                "providerCatalog": {
                    "openai": {"env": "OPENAI_API_KEY", "url": "https://api.openai.com/v1/models", "method": "GET"}
                },
            }
        }
    }
    path.write_text(json.dumps(payload), encoding="utf-8")


class LiveProviderProbeTests(unittest.TestCase):
    def test_blocking_tier_fails_when_no_secrets(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            out = tmp / "live.json"
            _write_contract(contract)
            proc = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--contract",
                    str(contract),
                    "--head-sha",
                    "deadbeefcafebabe",
                    "--risk-tier",
                    "high",
                    "--out",
                    str(out),
                    "--attempts-override",
                    "1",
                ],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=False,
            )
            self.assertNotEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            payload = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(payload["status"], "fail")

    def test_non_blocking_tier_is_advisory_when_no_secrets(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract = tmp / "contract.json"
            out = tmp / "live.json"
            _write_contract(contract)
            proc = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--contract",
                    str(contract),
                    "--head-sha",
                    "deadbeefcafebabe",
                    "--risk-tier",
                    "low",
                    "--out",
                    str(out),
                    "--attempts-override",
                    "1",
                ],
                capture_output=True,
                text=True,
                cwd=str(REPO_ROOT),
                check=False,
            )
            self.assertEqual(proc.returncode, 0, proc.stdout + proc.stderr)
            payload = json.loads(out.read_text(encoding="utf-8"))
            self.assertEqual(payload["status"], "advisory")


if __name__ == "__main__":
    unittest.main()
