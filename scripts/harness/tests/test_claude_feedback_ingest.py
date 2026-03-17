#!/usr/bin/env python3

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest import mock


REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT = REPO_ROOT / "scripts/harness/claude_feedback_ingest.py"

spec = importlib.util.spec_from_file_location("claude_feedback_ingest", SCRIPT)
assert spec and spec.loader
claude_feedback_ingest = importlib.util.module_from_spec(spec)
spec.loader.exec_module(claude_feedback_ingest)  # type: ignore[arg-type]


def _comment_with_payload(head_sha: str, *, app_id: int | None, login: str) -> dict:
    payload = {
        "head_sha": head_sha,
        "findings": [
            {
                "id": "claude-1",
                "severity": "high",
                "confidence": 0.9,
                "path": "crates/openfang-runtime/src/agent_loop.rs",
                "line": 120,
                "summary": "Fix failing guard path",
                "actionable": True,
            }
        ],
    }
    return {
        "source": "issue_comment",
        "id": "123",
        "body": "<!-- openfang-claude-feedback -->\n```json\n"
        + json.dumps(payload)
        + "\n```",
        "author_login": login,
        "author_app_id": app_id,
        "html_url": "https://example.invalid/comment/123",
    }


class ClaudeFeedbackIngestTests(unittest.TestCase):
    def _run_ingest(self, *, contract: dict, comments: list[dict], head_sha: str = "abc1234") -> dict:
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            contract_path = tmp / "contract.json"
            out_path = tmp / "claude-findings.json"
            contract_path.write_text(json.dumps(contract), encoding="utf-8")

            args = argparse.Namespace(
                repo="owner/repo",
                pr=1,
                head_sha=head_sha,
                contract=str(contract_path),
                token_env="GITHUB_TOKEN",
                out=str(out_path),
            )

            with (
                mock.patch.object(claude_feedback_ingest, "parse_args", return_value=args),
                mock.patch.object(claude_feedback_ingest, "_iter_comment_sources", return_value=comments),
                mock.patch.dict(os.environ, {"GITHUB_TOKEN": "token-value"}, clear=False),
            ):
                exit_code = claude_feedback_ingest.main()
                self.assertEqual(exit_code, 0)

            return json.loads(out_path.read_text(encoding="utf-8"))

    def test_trusted_app_id_is_accepted(self) -> None:
        contract = {
            "reviewProviders": {
                "providers": {
                    "claude": {
                        "github": {
                            "trustedActorLogins": [],
                            "trustedAppIds": [321],
                            "commentMarker": "<!-- openfang-claude-feedback -->",
                            "requireHeadShaMatch": True,
                        },
                        "parse": {"maxFindings": 200},
                    }
                }
            }
        }
        payload = self._run_ingest(contract=contract, comments=[_comment_with_payload("abc1234", app_id=321, login="")])
        self.assertEqual(payload["status"], "success")
        self.assertEqual(payload["ingestion_metrics"]["parsed_comments"], 1)
        self.assertEqual(len(payload["findings"]), 1)

    def test_untrusted_comment_is_rejected(self) -> None:
        contract = {
            "reviewProviders": {
                "providers": {
                    "claude": {
                        "github": {
                            "trustedActorLogins": ["trusted-user"],
                            "trustedAppIds": [321],
                            "commentMarker": "<!-- openfang-claude-feedback -->",
                            "requireHeadShaMatch": True,
                        },
                        "parse": {"maxFindings": 200},
                    }
                }
            }
        }
        payload = self._run_ingest(
            contract=contract,
            comments=[_comment_with_payload("abc1234", app_id=999, login="intruder")],
        )
        self.assertEqual(payload["status"], "missing")
        self.assertEqual(payload["ingestion_metrics"]["ignored_untrusted"], 1)
        self.assertEqual(payload["ingestion_metrics"]["parsed_comments"], 0)
        self.assertEqual(len(payload["findings"]), 0)

    def test_stale_head_sha_is_ignored(self) -> None:
        contract = {
            "reviewProviders": {
                "providers": {
                    "claude": {
                        "github": {
                            "trustedActorLogins": ["gagan114662"],
                            "trustedAppIds": [],
                            "commentMarker": "<!-- openfang-claude-feedback -->",
                            "requireHeadShaMatch": True,
                        },
                        "parse": {"maxFindings": 200},
                    }
                }
            }
        }
        payload = self._run_ingest(
            contract=contract,
            comments=[_comment_with_payload("deadbeef", app_id=None, login="gagan114662")],
            head_sha="abc1234",
        )
        self.assertEqual(payload["status"], "missing")
        self.assertEqual(payload["ingestion_metrics"]["ignored_stale"], 1)
        self.assertEqual(payload["ingestion_metrics"]["parsed_comments"], 0)
        self.assertEqual(len(payload["findings"]), 0)


if __name__ == "__main__":
    unittest.main()
