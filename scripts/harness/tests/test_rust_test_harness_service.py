#!/usr/bin/env python3

from __future__ import annotations

import base64
import io
import tarfile
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
HARNESS_DIR = REPO_ROOT / "scripts" / "harness"
import sys

sys.path.insert(0, str(HARNESS_DIR))
import rust_test_harness_service as harness  # noqa: E402


def _cfg(allowed_roots: list[Path]) -> harness.HarnessConfig:
    return harness.HarnessConfig(
        host="127.0.0.1",
        port=7788,
        docker_image="rust:1.86-bookworm",
        docker_cpus="2.0",
        docker_memory="4g",
        docker_pids_limit=256,
        default_timeout_secs=900,
        max_request_bytes=5 * 1024 * 1024,
        max_output_tail_bytes=16 * 1024,
        max_inline_source_bytes=20 * 1024 * 1024,
        results_dir=REPO_ROOT / "artifacts" / "rust-harness-runs",
        allowed_roots=[p.resolve() for p in allowed_roots],
        sentry_dsn_env="SENTRY_DSN",
        sentry_environment="test",
    )


def _tar_b64(files: dict[str, bytes]) -> str:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w:gz") as tar:
        for name, content in files.items():
            info = tarfile.TarInfo(name=name)
            info.size = len(content)
            tar.addfile(info, io.BytesIO(content))
    return base64.b64encode(buf.getvalue()).decode("utf-8")


class RustHarnessServiceTests(unittest.TestCase):
    def test_normalize_checks_defaults(self) -> None:
        checks = harness._normalize_checks(None)
        self.assertEqual(len(checks), 2)
        self.assertEqual(checks[0]["name"], "build")

    def test_normalize_checks_rejects_empty_command(self) -> None:
        with self.assertRaises(harness.HarnessRequestError):
            harness._normalize_checks([{"name": "bad", "command": ""}])

    def test_validate_source_path_blocks_outside_allowed_root(self) -> None:
        with tempfile.TemporaryDirectory() as allowed, tempfile.TemporaryDirectory() as outside:
            cfg = _cfg([Path(allowed)])
            outside_path = Path(outside)
            with self.assertRaises(harness.HarnessRequestError):
                harness._validate_source_path(str(outside_path), cfg)

    def test_prepare_source_from_path_and_requires_cargo_toml(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            src = root / "src_project"
            src.mkdir()
            (src / "Cargo.toml").write_text("[package]\nname='x'\nversion='0.1.0'\n", encoding="utf-8")
            cfg = _cfg([root])
            project_dir = root / "stage" / "project"
            meta = harness._prepare_source({"source_path": str(src)}, project_dir, cfg)
            self.assertEqual(meta["kind"], "source_path")
            self.assertTrue((project_dir / "Cargo.toml").exists())

    def test_extract_tarball_rejects_path_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            cfg = _cfg([root])
            payload = _tar_b64({"../evil.txt": b"nope"})
            with self.assertRaises(harness.HarnessRequestError):
                harness._extract_tarball(payload, root / "dest", cfg)

    def test_prepare_source_from_inline_tarball(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            cfg = _cfg([root])
            payload = _tar_b64({"Cargo.toml": b"[package]\nname='x'\nversion='0.1.0'\n"})
            project_dir = root / "stage" / "project"
            meta = harness._prepare_source({"source_tar_gz_base64": payload}, project_dir, cfg)
            self.assertEqual(meta["kind"], "inline_tarball")
            self.assertTrue((project_dir / "Cargo.toml").exists())


if __name__ == "__main__":
    unittest.main()

