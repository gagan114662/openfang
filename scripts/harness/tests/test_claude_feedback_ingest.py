import json
import os
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

import claude_feedback_ingest as ingest


class ClaudeFeedbackIngestTests(unittest.TestCase):
    def _write_contract(self, tmp: Path) -> Path:
        contract = {
            "reviewProviders": {
                "providers": {
                    "claude": {
                        "marker": "<!-- claude-review-findings -->",
                        "github": {
                            "trustedAppIdsEnv": "OPENFANG_CLAUDE_TRUSTED_APP_IDS",
                            "trustedLogins": ["claude[bot]"],
                            "maintainerAllowlist": [],
                        },
                    }
                }
            }
        }
        contract_path = tmp / "policy.contract.json"
        contract_path.write_text(json.dumps(contract), encoding="utf-8")
        return contract_path

    def _run_ingest(self, sources):
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            out = tmp / "claude-findings.json"
            contract = self._write_contract(tmp)

            args = Namespace(
                repo="acme/repo",
                pr=123,
                head_sha="abc1234",
                contract=str(contract),
                token_env="GITHUB_TOKEN",
                out=str(out),
            )

            with patch.object(ingest, "parse_args", return_value=args), patch.object(
                ingest, "_iter_comment_sources", return_value=sources
            ), patch.dict(os.environ, {"GITHUB_TOKEN": "x", "OPENFANG_CLAUDE_TRUSTED_APP_IDS": "101"}):
                code = ingest.main()

            payload = json.loads(out.read_text(encoding="utf-8"))
            return code, payload

    def test_trusted_app_id_is_accepted(self):
        source = {
            "id": 7,
            "body": "<!-- claude-review-findings -->\n```json\n{\"head_sha\":\"abc1234\",\"status\":\"success\",\"findings\":[{\"id\":\"f1\",\"severity\":\"high\",\"confidence\":0.8,\"path\":\"x\",\"line\":1,\"summary\":\"bug\",\"actionable\":true}]}\n```",
            "user": {"login": "claude[bot]"},
            "performed_via_github_app": {"id": 101},
        }
        code, payload = self._run_ingest([source])

        self.assertEqual(code, 0)
        self.assertEqual(payload["status"], "success")
        self.assertEqual(len(payload["findings"]), 1)

    def test_untrusted_comment_is_rejected(self):
        source = {
            "id": 9,
            "body": "<!-- claude-review-findings -->\n{\"head_sha\":\"abc1234\",\"status\":\"success\",\"findings\":[]}",
            "user": {"login": "random-user"},
            "performed_via_github_app": {"id": 999},
        }
        code, payload = self._run_ingest([source])

        self.assertEqual(code, 2)
        self.assertEqual(payload["status"], "missing")
        self.assertTrue(any("untrusted" in error for error in payload["errors"]))

    def test_stale_head_sha_is_ignored(self):
        source = {
            "id": 11,
            "body": "<!-- claude-review-findings -->\n{\"head_sha\":\"oldsha\",\"status\":\"success\",\"findings\":[]}",
            "user": {"login": "claude[bot]"},
            "performed_via_github_app": {"id": 101},
        }
        code, payload = self._run_ingest([source])

        self.assertEqual(code, 2)
        self.assertEqual(payload["status"], "missing")
        self.assertTrue(any("stale" in error for error in payload["errors"]))


if __name__ == "__main__":
    unittest.main()
