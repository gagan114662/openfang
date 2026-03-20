"""Tests for harness maturity pattern implementations.

Verifies that all modules from the harness maturity assessment exist,
are registered, and follow expected conventions.
"""

import os
import pathlib

import pytest

# Repo root is 4 levels up from this test file
REPO_ROOT = pathlib.Path(__file__).resolve().parents[3]
RUNTIME_SRC = REPO_ROOT / "crates" / "openfang-runtime" / "src"
LIB_RS = RUNTIME_SRC / "lib.rs"


def _lib_rs_content() -> str:
    return LIB_RS.read_text()


# ── Existing modules ────────────────────────────────────────────────


class TestContextFolder:
    def test_module_exists(self):
        assert (RUNTIME_SRC / "context_folder.rs").is_file()

    def test_registered_in_lib(self):
        assert "pub mod context_folder;" in _lib_rs_content()

    def test_has_floor_char_boundary(self):
        content = (RUNTIME_SRC / "context_folder.rs").read_text()
        assert "floor_char_boundary" in content


class TestDelegation:
    def test_module_exists(self):
        assert (RUNTIME_SRC / "delegation.rs").is_file()

    def test_registered_in_lib(self):
        assert "pub mod delegation;" in _lib_rs_content()


class TestFeatureList:
    def test_module_exists(self):
        assert (RUNTIME_SRC / "feature_list.rs").is_file()

    def test_registered_in_lib(self):
        assert "pub mod feature_list;" in _lib_rs_content()


class TestProgress:
    def test_module_exists(self):
        assert (RUNTIME_SRC / "progress.rs").is_file()

    def test_registered_in_lib(self):
        assert "pub mod progress;" in _lib_rs_content()


# ── New modules ──────────────────────────────────────────────────────


class TestBrowserAutomation:
    def test_module_exists(self):
        assert (RUNTIME_SRC / "browser_automation.rs").is_file()

    def test_registered_in_lib(self):
        assert "pub mod browser_automation;" in _lib_rs_content()

    def test_has_dom_assertion(self):
        content = (RUNTIME_SRC / "browser_automation.rs").read_text()
        assert "DomAssertion" in content

    def test_has_verification_plan(self):
        content = (RUNTIME_SRC / "browser_automation.rs").read_text()
        assert "VerificationPlan" in content

    def test_has_verify_feature(self):
        content = (RUNTIME_SRC / "browser_automation.rs").read_text()
        assert "fn verify_feature" in content

    def test_has_build_verification_report(self):
        content = (RUNTIME_SRC / "browser_automation.rs").read_text()
        assert "fn build_verification_report" in content


class TestStartupSequence:
    def test_module_exists(self):
        assert (RUNTIME_SRC / "startup_sequence.rs").is_file()

    def test_registered_in_lib(self):
        assert "pub mod startup_sequence;" in _lib_rs_content()

    def test_has_startup_step(self):
        content = (RUNTIME_SRC / "startup_sequence.rs").read_text()
        assert "StartupStep" in content

    def test_has_default_sequence(self):
        content = (RUNTIME_SRC / "startup_sequence.rs").read_text()
        assert "fn default_sequence" in content

    def test_has_orientation_context(self):
        content = (RUNTIME_SRC / "startup_sequence.rs").read_text()
        assert "fn orientation_context" in content


class TestObservability:
    def test_module_exists(self):
        assert (RUNTIME_SRC / "observability.rs").is_file()

    def test_registered_in_lib(self):
        assert "pub mod observability;" in _lib_rs_content()

    def test_has_log_entry(self):
        content = (RUNTIME_SRC / "observability.rs").read_text()
        assert "LogEntry" in content

    def test_has_log_query(self):
        content = (RUNTIME_SRC / "observability.rs").read_text()
        assert "LogQuery" in content

    def test_has_query_logs(self):
        content = (RUNTIME_SRC / "observability.rs").read_text()
        assert "fn query_logs" in content

    def test_has_recent_errors(self):
        content = (RUNTIME_SRC / "observability.rs").read_text()
        assert "fn recent_errors" in content

    def test_has_build_observability_section(self):
        content = (RUNTIME_SRC / "observability.rs").read_text()
        assert "fn build_observability_section" in content


class TestInitializerAgent:
    def test_agent_dir_exists(self):
        assert (REPO_ROOT / "agents" / "initializer").is_dir()

    def test_agent_toml_exists(self):
        assert (REPO_ROOT / "agents" / "initializer" / "agent.toml").is_file()

    def test_agent_toml_has_name(self):
        content = (REPO_ROOT / "agents" / "initializer" / "agent.toml").read_text()
        assert 'name = "initializer"' in content

    def test_agent_produces_three_outputs(self):
        content = (REPO_ROOT / "agents" / "initializer" / "agent.toml").read_text()
        assert "init.sh" in content
        assert "FEATURES.json" in content
        assert "PROGRESS.md" in content


# ── Guard script ─────────────────────────────────────────────────────


class TestGuardScript:
    @pytest.mark.skipif(
        not (pathlib.Path(__file__).resolve().parents[3] / "scripts" / "worktree" / "guard.sh").is_file(),
        reason="guard.sh not present on this branch",
    )
    def test_guard_exists(self):
        assert (REPO_ROOT / "scripts" / "worktree" / "guard.sh").is_file()

    @pytest.mark.skipif(
        not (pathlib.Path(__file__).resolve().parents[3] / "scripts" / "worktree" / "guard.sh").is_file(),
        reason="guard.sh not present on this branch",
    )
    def test_guard_enforces_branch_convention(self):
        content = (REPO_ROOT / "scripts" / "worktree" / "guard.sh").read_text()
        assert '"$tool/"' in content


# ── Arch enforcement ─────────────────────────────────────────────────


class TestArchEnforcement:
    def test_arch_test_exists(self):
        assert (REPO_ROOT / "xtask" / "tests" / "arch_enforcement.rs").is_file()

    def test_has_layer_violation_struct(self):
        content = (REPO_ROOT / "xtask" / "tests" / "arch_enforcement.rs").read_text()
        assert "LayerViolation" in content

    def test_has_remediation_steps(self):
        content = (REPO_ROOT / "xtask" / "tests" / "arch_enforcement.rs").read_text()
        assert "remediation_steps" in content

    def test_has_agent_friendly_format(self):
        content = (REPO_ROOT / "xtask" / "tests" / "arch_enforcement.rs").read_text()
        assert "VIOLATION:" in content
        assert "RULE:" in content
        assert "FIX:" in content


# ── Assessment doc ───────────────────────────────────────────────────


class TestAssessmentDoc:
    def test_doc_exists(self):
        assert (REPO_ROOT / "docs" / "harness-maturity-assessment.md").is_file()

    def test_doc_has_pattern_table(self):
        content = (REPO_ROOT / "docs" / "harness-maturity-assessment.md").read_text()
        assert "Pattern Coverage Summary" in content

    def test_doc_lists_new_modules(self):
        content = (REPO_ROOT / "docs" / "harness-maturity-assessment.md").read_text()
        assert "browser_automation" in content
        assert "startup_sequence" in content
        assert "observability" in content
