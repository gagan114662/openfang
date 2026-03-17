import json
import tempfile
import unittest
from argparse import Namespace
from pathlib import Path
from unittest.mock import patch

import risk_policy_gate as gate


class RiskPolicyGateTests(unittest.TestCase):
    def test_claude_required_missing_fails_gate(self):
        with tempfile.TemporaryDirectory() as td:
            tmp = Path(td)
            changed = tmp / "changed.txt"
            changed.write_text("docs/readme.md\n", encoding="utf-8")

            contract = {
                "rolloutPolicy": {
                    "currentPhase": "phase-2",
                    "phases": {
                        "phase-2": {
                            "enforceMergeBlock": True,
                            "enforceReviewState": False,
                            "enableRemediation": False,
                            "requireEvidence": False,
                            "enforceDocsDrift": False,
                        }
                    },
                },
                "riskTierRules": {"low": ["**"]},
                "mergePolicy": {"low": {"requiredChecks": ["risk-policy-gate"]}},
                "reviewPolicy": {
                    "provider": "greptile",
                    "checkRunName": "greptile-review",
                    "timeoutMinutes": 1,
                    "weakConfidenceThreshold": 0.55,
                    "actionableSummaryKeywords": [],
                },
                "reviewProviders": {
                    "providers": {
                        "greptile": {"enforcement": "advisory"},
                        "claude": {
                            "enforcement": "required",
                            "requireCurrentHeadIngestion": True,
                        },
                    }
                },
                "docsDriftRules": [],
                "evidencePolicy": {"uiImpactPaths": []},
            }
            contract_path = tmp / "policy.contract.json"
            contract_path.write_text(json.dumps(contract), encoding="utf-8")

            review_findings = tmp / "review-findings.json"
            report_out = tmp / "risk-policy-report.json"
            claude_findings = tmp / "claude-findings.json"
            claude_findings.write_text("{}", encoding="utf-8")

            args = Namespace(
                pr=77,
                head_sha="abc1234",
                changed_files=str(changed),
                contract=str(contract_path),
                repo="",
                token_env="GITHUB_TOKEN",
                review_findings=str(review_findings),
                claude_findings=str(claude_findings),
                browser_evidence_manifest=str(tmp / "browser-evidence-manifest.json"),
                infra_preflight_report=str(tmp / "infra.json"),
                live_provider_report=str(tmp / "live.json"),
                report_out=str(report_out),
                poll_seconds=1,
            )

            with patch.object(gate, "parse_args", return_value=args):
                exit_code = gate.main()

            self.assertEqual(exit_code, 1)
            report = json.loads(report_out.read_text(encoding="utf-8"))
            self.assertEqual(report["decision"], "fail")
            self.assertTrue(any("claude findings ingestion status" in reason for reason in report["reasons"]))


if __name__ == "__main__":
    unittest.main()
