#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "scripts/harness/infra_preflight.py"


class InfraPreflightTests(unittest.TestCase):
    def test_invalid_repo_fails_preflight(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            report = tmp / "infra.json"
            env = os.environ.copy()
            env["GITHUB_TOKEN"] = "dummy-token"
            proc = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--contract",
                    str(REPO_ROOT / ".harness/policy.contract.json"),
                    "--workflow",
                    "infra-preflight",
                    "--repo",
                    "invalid-repo-format",
                    "--token-env",
                    "GITHUB_TOKEN",
                    "--attempts-override",
                    "1",
                    "--out",
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
            self.assertEqual(payload["status"], "fail")
            self.assertGreaterEqual(len(payload.get("errors", [])), 1)


if __name__ == "__main__":
    unittest.main()
