#!/usr/bin/env python3
"""Harness maturity validation tests.

Programmatically verifies that OpenFang's harness infrastructure exists,
is internally consistent, and matches the patterns described in the
harness engineering literature (SWE-agent ACI, Anthropic two-agent
architecture, OpenAI Codex zero-manual-code).

Run: pytest scripts/harness/tests/test_harness_maturity.py -v
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]


def _load_contract() -> dict:
    path = REPO_ROOT / ".harness" / "policy.contract.json"
    return json.loads(path.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# Pattern 1: Progressive Disclosure
# Short CLAUDE.md map + deep docs/; capped context; startup orientation
# ---------------------------------------------------------------------------

class TestProgressiveDisclosure(unittest.TestCase):
    """Verify CLAUDE.md is a short map pointing to deeper docs/."""

    def test_claude_md_exists(self):
        self.assertTrue((REPO_ROOT / "CLAUDE.md").exists())

    def test_agents_md_exists(self):
        self.assertTrue((REPO_ROOT / "AGENTS.md").exists())

    def test_claude_md_is_concise(self):
        """CLAUDE.md should be a map, not a monolith (<300 lines)."""
        lines = (REPO_ROOT / "CLAUDE.md").read_text(encoding="utf-8").splitlines()
        self.assertLess(len(lines), 300,
                        f"CLAUDE.md is {len(lines)} lines — too large for progressive disclosure")

    def test_docs_directory_has_depth(self):
        """docs/ should have substantial content for agents to drill into."""
        docs = list((REPO_ROOT / "docs").glob("*.md"))
        self.assertGreaterEqual(len(docs), 10,
                                f"docs/ has only {len(docs)} files — insufficient depth")

    def test_harness_engineering_doc_exists(self):
        self.assertTrue((REPO_ROOT / "docs" / "harness-engineering.md").exists())

    def test_architecture_doc_exists(self):
        self.assertTrue((REPO_ROOT / "docs" / "architecture.md").exists())

    def test_context_budget_module_exists(self):
        """Runtime context budget prevents SWE-agent-style context flooding."""
        self.assertTrue(
            (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "context_budget.rs").exists()
        )


# ---------------------------------------------------------------------------
# Pattern 2: Git Worktree Isolation
# Root read-only; claude/<task> and codex/<task>; concurrent locks
# ---------------------------------------------------------------------------

class TestWorktreeIsolation(unittest.TestCase):
    """Verify worktree isolation scripts exist and are executable."""

    REQUIRED_SCRIPTS = [
        "guard.sh",
        "open_agent_worktree.sh",
        "agent_entry.sh",
        "common.sh",
        "finish_agent_task.sh",
        "install_agent_launchers.sh",
        "status.sh",
        "recover.sh",
        "root_mode.sh",
    ]

    def test_worktree_scripts_exist(self):
        worktree_dir = REPO_ROOT / "scripts" / "worktree"
        for script in self.REQUIRED_SCRIPTS:
            path = worktree_dir / script
            self.assertTrue(path.exists(), f"Missing worktree script: {script}")

    def test_guard_enforces_branch_convention(self):
        """guard.sh should reference claude/ and codex/ branch patterns."""
        guard = (REPO_ROOT / "scripts" / "worktree" / "guard.sh").read_text(encoding="utf-8")
        self.assertIn("claude/", guard, "guard.sh doesn't enforce claude/ branch convention")
        self.assertIn("codex/", guard, "guard.sh doesn't enforce codex/ branch convention")

    def test_finish_gate_enforces_clean_state(self):
        """finish_agent_task.sh should enforce cargo build + test."""
        finish = (REPO_ROOT / "scripts" / "worktree" / "finish_agent_task.sh").read_text(encoding="utf-8")
        self.assertIn("cargo", finish, "finish gate doesn't run cargo commands")

    def test_root_mode_supports_lock_unlock(self):
        """root_mode.sh should support lock/unlock/status."""
        root_mode = (REPO_ROOT / "scripts" / "worktree" / "root_mode.sh").read_text(encoding="utf-8")
        self.assertIn("lock", root_mode)
        self.assertIn("unlock", root_mode)
        self.assertIn("status", root_mode)


# ---------------------------------------------------------------------------
# Pattern 3: Spec First / Repository as System of Record
# policy.contract.json drives all enforcement; JSON schemas validate;
# feature_list.rs + progress.rs parse structured state files
# ---------------------------------------------------------------------------

class TestSpecFirst(unittest.TestCase):
    """Verify machine-readable policy contract is complete and consistent."""

    def setUp(self):
        self.contract = _load_contract()

    def test_contract_has_version(self):
        self.assertIn("version", self.contract)

    def test_contract_has_rollout_policy(self):
        rollout = self.contract.get("rolloutPolicy", {})
        self.assertIn("currentPhase", rollout)
        self.assertIn("phases", rollout)
        current = rollout["currentPhase"]
        self.assertIn(current, rollout["phases"],
                      f"currentPhase '{current}' not defined in phases")

    def test_rollout_phases_have_required_fields(self):
        required_fields = [
            "enforceMergeBlock", "enforceReviewState", "enforceDocsDrift",
            "enableRemediation", "requireEvidence",
        ]
        phases = self.contract["rolloutPolicy"]["phases"]
        for phase_name, phase in phases.items():
            for field in required_fields:
                self.assertIn(field, phase,
                              f"Phase '{phase_name}' missing field '{field}'")

    def test_risk_tiers_cover_all_levels(self):
        rules = self.contract.get("riskTierRules", {})
        for tier in ["critical", "high", "medium", "low"]:
            self.assertIn(tier, rules, f"Missing risk tier: {tier}")
            self.assertIsInstance(rules[tier], list)
            self.assertGreater(len(rules[tier]), 0,
                               f"Risk tier '{tier}' has no path patterns")

    def test_merge_policy_matches_risk_tiers(self):
        """Every risk tier in riskTierRules should have a mergePolicy entry."""
        risk_tiers = set(self.contract.get("riskTierRules", {}).keys())
        merge_tiers = set(self.contract.get("mergePolicy", {}).keys())
        missing = risk_tiers - merge_tiers
        self.assertEqual(missing, set(),
                         f"Risk tiers without merge policy: {missing}")

    def test_merge_policy_checks_include_gate(self):
        """Every tier's required checks should include risk-policy-gate."""
        merge_policy = self.contract.get("mergePolicy", {})
        for tier, config in merge_policy.items():
            checks = config.get("requiredChecks", [])
            self.assertIn("risk-policy-gate", checks,
                          f"Tier '{tier}' missing risk-policy-gate in required checks")

    def test_review_providers_defined(self):
        providers = self.contract.get("reviewProviders", {}).get("providers", {})
        self.assertIn("greptile", providers)
        self.assertIn("claude", providers)

    def test_provider_enforcement_values_valid(self):
        valid = {"required", "advisory", "disabled"}
        providers = self.contract.get("reviewProviders", {}).get("providers", {})
        for name, cfg in providers.items():
            enforcement = cfg.get("enforcement", "required")
            self.assertIn(enforcement, valid,
                          f"Provider '{name}' has invalid enforcement: {enforcement}")

    def test_docs_drift_rules_are_list(self):
        rules = self.contract.get("docsDriftRules", [])
        self.assertIsInstance(rules, list)

    def test_docs_drift_rules_have_required_fields(self):
        rules = self.contract.get("docsDriftRules", [])
        for rule in rules:
            self.assertIn("name", rule)
            self.assertIn("whenTouched", rule)
            self.assertIn("requireAny", rule)

    def test_remediation_policy_has_guardrails(self):
        policy = self.contract.get("remediationPolicy", {})
        self.assertIn("allowedPathGlobs", policy)
        self.assertIn("forbiddenPathGlobs", policy)
        self.assertIn("validationCommands", policy)

    def test_forbidden_paths_include_safety_dirs(self):
        """Remediation must never touch .git, .github, target, .harness."""
        forbidden = self.contract.get("remediationPolicy", {}).get("forbiddenPathGlobs", [])
        for safety_dir in [".git/**", ".github/**", "target/**", ".harness/**"]:
            self.assertIn(safety_dir, forbidden,
                          f"Remediation missing safety exclusion: {safety_dir}")

    def test_schemas_exist(self):
        schemas_dir = REPO_ROOT / ".harness" / "schemas"
        self.assertTrue(schemas_dir.exists(), ".harness/schemas/ directory missing")
        schemas = list(schemas_dir.glob("*.schema.json"))
        self.assertGreaterEqual(len(schemas), 2,
                                f"Only {len(schemas)} schemas found — expected >= 2")

    def test_feature_list_module_exists(self):
        self.assertTrue(
            (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "feature_list.rs").exists()
        )

    def test_progress_module_exists(self):
        self.assertTrue(
            (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "progress.rs").exists()
        )


# ---------------------------------------------------------------------------
# Pattern 4: Mechanical Architecture Enforcement
# xtask check-layers; risk-policy-gate blocks merge; CI fanout per tier;
# remediation constrained to allowed paths
# ---------------------------------------------------------------------------

class TestMechanicalEnforcement(unittest.TestCase):
    """Verify mechanical enforcement is wired and internally consistent."""

    def test_xtask_exists(self):
        self.assertTrue((REPO_ROOT / "xtask" / "src" / "main.rs").exists())

    def test_ci_workflow_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".github" / "workflows" / "ci.yml").exists()
        )

    def test_risk_policy_gate_workflow_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".github" / "workflows" / "risk-policy-gate.yml").exists()
        )

    def test_claude_remediation_workflow_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".github" / "workflows" / "claude-remediation-agent.yml").exists()
        )

    def test_ci_fanout_workflow_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".github" / "workflows" / "ci-fanout.yml").exists()
        )

    def test_pr_review_harness_workflow_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".github" / "workflows" / "pr-review-harness.yml").exists()
        )

    def test_risk_policy_gate_script_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "harness" / "risk_policy_gate.py").exists()
        )

    def test_checks_resolver_script_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "harness" / "checks_resolver.py").exists()
        )

    def test_remediation_runner_script_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "harness" / "remediation_runner.py").exists()
        )

    def test_ci_workflow_uses_nextest(self):
        """CI must use cargo-nextest (not cargo test) to avoid runner OOM."""
        ci = (REPO_ROOT / ".github" / "workflows" / "ci.yml").read_text(encoding="utf-8")
        self.assertIn("cargo-nextest", ci, "CI not using cargo-nextest")
        self.assertIn("cargo nextest run", ci, "CI not running nextest")


# ---------------------------------------------------------------------------
# Pattern 5: Integrated Feedback Loops
# Claude hooks → Sentry events; gate returns decisions; remediation inline;
# weekly metrics; browser evidence schema
# ---------------------------------------------------------------------------

class TestIntegratedFeedbackLoops(unittest.TestCase):
    """Verify feedback loops are wired from hooks through to metrics."""

    def test_claude_hook_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "claude" / "claude_hook.py").exists()
        )

    def test_claude_desktop_setup_check_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "claude" / "check_desktop_setup.py").exists()
        )

    def test_mcp_config_exists(self):
        self.assertTrue((REPO_ROOT / ".mcp.json").exists())

    def test_mcp_config_has_openfang_server(self):
        mcp = json.loads((REPO_ROOT / ".mcp.json").read_text(encoding="utf-8"))
        servers = mcp.get("mcpServers", {})
        self.assertIn("openfang", servers,
                      "MCP config missing openfang server")

    def test_emit_structured_event_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "harness" / "emit_structured_event.py").exists()
        )

    def test_sentry_live_summary_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "harness" / "sentry_live_summary.py").exists()
        )

    def test_sentry_logs_validate_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "harness" / "sentry_logs_live_validate.py").exists()
        )

    def test_browser_evidence_verify_exists(self):
        self.assertTrue(
            (REPO_ROOT / "scripts" / "harness" / "browser_evidence_verify.py").exists()
        )

    def test_browser_evidence_schema_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".harness" / "schemas" / "browser-evidence.schema.json").exists()
        )

    def test_greptile_rerun_workflow_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".github" / "workflows" / "greptile-rerun.yml").exists()
        )

    def test_weekly_metrics_workflow_exists(self):
        self.assertTrue(
            (REPO_ROOT / ".github" / "workflows" / "harness-weekly-metrics.yml").exists()
        )


# ---------------------------------------------------------------------------
# Cross-cutting: Contract ↔ Workflow consistency
# ---------------------------------------------------------------------------

class TestContractWorkflowConsistency(unittest.TestCase):
    """Verify the policy contract and GitHub workflows agree."""

    def setUp(self):
        self.contract = _load_contract()

    def test_review_policy_provider_matches_workflow(self):
        """reviewPolicy.provider should match the check name used in workflows."""
        provider = self.contract["reviewPolicy"]["provider"]
        check_name = self.contract["reviewPolicy"]["checkRunName"]
        # The greptile-rerun workflow should reference this check name
        rerun_path = REPO_ROOT / ".github" / "workflows" / "greptile-rerun.yml"
        if rerun_path.exists():
            content = rerun_path.read_text(encoding="utf-8")
            self.assertIn(provider, content.lower(),
                          f"greptile-rerun.yml doesn't reference provider '{provider}'")

    def test_remediation_markers_consistent(self):
        """Remediation SHA marker in workflow must match the commit grep."""
        workflow = (REPO_ROOT / ".github" / "workflows" / "claude-remediation-agent.yml")
        content = workflow.read_text(encoding="utf-8")
        self.assertIn("harness-remediation:", content,
                      "Remediation workflow missing SHA marker pattern")

    def test_rerun_marker_matches_contract(self):
        """rerunPolicy.marker in contract should appear in rerun workflow."""
        marker = self.contract.get("rerunPolicy", {}).get("marker", "")
        if marker:
            rerun_path = REPO_ROOT / ".github" / "workflows" / "greptile-rerun.yml"
            if rerun_path.exists():
                content = rerun_path.read_text(encoding="utf-8")
                self.assertIn(marker, content,
                              f"Rerun workflow doesn't use marker from contract: {marker}")


# ---------------------------------------------------------------------------
# Cross-cutting: Risk tier path coverage
# ---------------------------------------------------------------------------

class TestRiskTierCoverage(unittest.TestCase):
    """Verify risk tiers cover the actual crate structure."""

    def setUp(self):
        self.contract = _load_contract()
        self.rules = self.contract.get("riskTierRules", {})

    def _all_patterns(self) -> list[str]:
        patterns = []
        for tier_patterns in self.rules.values():
            patterns.extend(tier_patterns)
        return patterns

    def test_runtime_crate_is_critical(self):
        self.assertIn("crates/openfang-runtime/**", self.rules.get("critical", []))

    def test_kernel_crate_is_critical(self):
        self.assertIn("crates/openfang-kernel/**", self.rules.get("critical", []))

    def test_api_crate_is_high(self):
        self.assertIn("crates/openfang-api/**", self.rules.get("high", []))

    def test_low_tier_is_catchall(self):
        """Low tier should have ** to catch everything else."""
        self.assertIn("**", self.rules.get("low", []))

    def test_crates_dir_exists(self):
        """All crate paths referenced in risk tiers should exist."""
        import fnmatch as _fnmatch

        crate_dirs = [d.name for d in (REPO_ROOT / "crates").iterdir() if d.is_dir()]
        # Check that critical+high tier patterns reference real crates
        for tier in ["critical", "high"]:
            for pattern in self.rules.get(tier, []):
                if pattern.startswith("crates/"):
                    crate_name = pattern.split("/")[1]
                    # Allow glob patterns like openfang-types/src/agent.rs
                    matching = [c for c in crate_dirs if _fnmatch.fnmatch(c, crate_name)]
                    self.assertGreater(len(matching), 0,
                                       f"Risk tier '{tier}' references non-existent crate: {crate_name}")


# ---------------------------------------------------------------------------
# Anthropic pattern: Feature list + Progress tracking runtime modules
# ---------------------------------------------------------------------------

class TestAnthropicPatterns(unittest.TestCase):
    """Verify runtime modules for feature tracking and progress."""

    def test_feature_list_rs_has_status_tracking(self):
        """feature_list.rs should parse pass/fail statuses (Anthropic feature list pattern)."""
        content = (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "feature_list.rs").read_text(encoding="utf-8")
        # Should handle pass/fail/blocked status values
        for keyword in ["pass", "fail"]:
            self.assertIn(keyword, content.lower(),
                          f"feature_list.rs missing '{keyword}' status handling")

    def test_progress_rs_parses_task_lists(self):
        """progress.rs should parse GFM task lists (Anthropic progress pattern)."""
        content = (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "progress.rs").read_text(encoding="utf-8")
        # Should reference checkbox patterns like [x] or [ ]
        has_checkbox = "[x]" in content.lower() or "checkbox" in content.lower() or "task" in content.lower()
        self.assertTrue(has_checkbox,
                        "progress.rs doesn't appear to parse task lists")

    def test_workspace_context_module_exists(self):
        self.assertTrue(
            (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "workspace_context.rs").exists()
        )

    def test_prompt_builder_module_exists(self):
        self.assertTrue(
            (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "prompt_builder.rs").exists()
        )


# ---------------------------------------------------------------------------
# OpenAI pattern: Agent templates as structured definitions
# ---------------------------------------------------------------------------

class TestAgentTemplates(unittest.TestCase):
    """Verify agent templates follow structured patterns."""

    def test_agents_directory_exists(self):
        agents_dir = REPO_ROOT / "agents"
        self.assertTrue(agents_dir.exists(), "agents/ directory missing")

    def test_agents_have_definitions(self):
        """Each agent should have a structured definition file."""
        agents_dir = REPO_ROOT / "agents"
        if not agents_dir.exists():
            self.skipTest("agents/ directory not found")
        agent_dirs = [d for d in agents_dir.iterdir() if d.is_dir()]
        self.assertGreaterEqual(len(agent_dirs), 5,
                                f"Only {len(agent_dirs)} agent templates — expected >= 5")

    def test_agents_have_system_prompts(self):
        """Each agent should have a system.md or agent.json."""
        agents_dir = REPO_ROOT / "agents"
        if not agents_dir.exists():
            self.skipTest("agents/ directory not found")
        agent_dirs = [d for d in agents_dir.iterdir() if d.is_dir()]
        for agent_dir in agent_dirs:
            has_def = (
                (agent_dir / "system.md").exists()
                or (agent_dir / "agent.json").exists()
                or (agent_dir / "config.toml").exists()
            )
            self.assertTrue(has_def,
                            f"Agent '{agent_dir.name}' has no definition file")


# ---------------------------------------------------------------------------
# SWE-agent pattern: Capped search / context management
# ---------------------------------------------------------------------------

class TestContextManagement(unittest.TestCase):
    """Verify context management prevents SWE-agent failure modes."""

    def test_context_budget_has_per_result_cap(self):
        """context_budget.rs should cap per-result context (SWE-agent capped search)."""
        content = (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "context_budget.rs").read_text(encoding="utf-8")
        # Should reference percentage-based or absolute caps
        has_cap = any(keyword in content.lower() for keyword in ["cap", "limit", "max", "budget", "headroom"])
        self.assertTrue(has_cap, "context_budget.rs doesn't implement result capping")

    def test_context_folder_exists(self):
        self.assertTrue(
            (REPO_ROOT / "crates" / "openfang-runtime" / "src" / "context_folder.rs").exists()
        )


if __name__ == "__main__":
    unittest.main()
