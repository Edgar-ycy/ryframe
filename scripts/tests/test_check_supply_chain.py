from __future__ import annotations

import datetime as dt
import importlib.util
import json
import shutil
import unittest
import uuid
from contextlib import contextmanager
from collections.abc import Iterator
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "check_supply_chain.py"
TEMP_ROOT = SCRIPT.parents[1] / "target" / "script-tests"
TEMP_ROOT.mkdir(parents=True, exist_ok=True)
SPEC = importlib.util.spec_from_file_location("check_supply_chain", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def policy(*, vulnerabilities: list[dict[str, str]] | None = None) -> dict[str, object]:
    return {
        "schema_version": 1,
        "tools": {
            "cargo-audit": "0.22.2",
            "cargo-deny": "0.20.2",
            "cargo-cyclonedx": "0.5.9",
            "trivy": "0.72.0",
        },
        "vulnerability_gate": {
            "severities": ["HIGH", "CRITICAL"],
            "exceptions": vulnerabilities or [],
        },
        "dependency_graph_exceptions": [
            {
                "package": "optional-package",
                "version": "1.2.3",
                "enforcement": "must_be_absent_from_resolved_graph",
                "owner": "security-team",
                "expires": "2099-12-31",
                "reason": "仅允许保留在锁文件中，不得进入实际构建图",
            }
        ],
    }


class SupplyChainPolicyTests(unittest.TestCase):
    @contextmanager
    def temporary_directory(self) -> Iterator[str]:
        path = TEMP_ROOT / f"case-{uuid.uuid4().hex}"
        path.mkdir()
        try:
            yield str(path)
        finally:
            shutil.rmtree(path)

    def write_json(self, directory: Path, name: str, value: object) -> Path:
        path = directory / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def test_accepts_complete_future_dated_policy(self) -> None:
        with self.temporary_directory() as raw:
            path = self.write_json(Path(raw), "policy.json", policy())
            loaded = MODULE.load_policy(path, today=dt.date(2026, 8, 20))
        self.assertEqual(loaded["tools"]["trivy"], "0.72.0")

    def test_rejects_expired_or_ownerless_exception(self) -> None:
        invalid = policy()
        exception = invalid["dependency_graph_exceptions"][0]
        exception["expires"] = "2026-08-20"
        exception["owner"] = ""
        with self.temporary_directory() as raw:
            path = self.write_json(Path(raw), "policy.json", invalid)
            with self.assertRaises(MODULE.PolicyError):
                MODULE.load_policy(path, today=dt.date(2026, 8, 20))

    def test_workflow_requires_action_sha_image_digest_and_exact_tools(self) -> None:
        pinned = """
steps:
  - uses: owner/action@1111111111111111111111111111111111111111
  - uses: docker://owner/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  - name: install
    uses: taiki-e/install-action@2222222222222222222222222222222222222222
    with:
      tool: cargo-audit@0.22.2,cargo-deny@0.20.2,cargo-cyclonedx@0.5.9,trivy@0.72.0
      fallback: none
  - run: docker run owner/image@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb check
  - run: cargo cyclonedx --format cyclonedx
  - run: trivy image --format cyclonedx target
  - run: python scripts/check_supply_chain.py --trivy-report report.json
  - uses: actions/upload-artifact@3333333333333333333333333333333333333333
"""
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            (workflow_dir / "ci.yml").write_text(pinned, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, policy())
            self.assertEqual(errors, [])

            mutable = pinned.replace(
                "owner/action@1111111111111111111111111111111111111111",
                "owner/action@v1",
            ).replace(
                "owner/image@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "owner/image:latest",
            )
            (workflow_dir / "ci.yml").write_text(mutable, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, policy())
        self.assertTrue(any("action 未固定" in error for error in errors))
        self.assertTrue(any("docker 外部镜像未固定" in error for error in errors))

    def test_trivy_gate_matches_exact_package_version_and_rejects_unused_exception(self) -> None:
        exception = {
            "id": "CVE-2099-0001",
            "package": "libexample",
            "installed_version": "1.0.0",
            "owner": "security-team",
            "expires": "2099-12-31",
            "reason": "等待上游发布兼容的安全修复版本",
        }
        configured = policy(vulnerabilities=[exception])
        report = {
            "Results": [
                {
                    "Target": "runtime",
                    "Vulnerabilities": [
                        {
                            "VulnerabilityID": "CVE-2099-0001",
                            "PkgName": "libexample",
                            "InstalledVersion": "1.0.0",
                            "Severity": "CRITICAL",
                        }
                    ],
                }
            ]
        }
        with self.temporary_directory() as raw:
            report_path = self.write_json(Path(raw), "trivy.json", report)
            self.assertEqual(MODULE.evaluate_trivy_report(report_path, configured), [])
            report["Results"][0]["Vulnerabilities"] = []
            self.write_json(Path(raw), "trivy.json", report)
            errors = MODULE.evaluate_trivy_report(report_path, configured)
        self.assertTrue(any("例外未被报告使用" in error for error in errors))

    def test_resolved_graph_exception_fails_closed(self) -> None:
        errors = MODULE.resolved_graph_violations(
            "root v0.1.0\noptional-package v1.2.3\n",
            policy(),
        )
        self.assertEqual(len(errors), 1)
        self.assertIn("optional-package v1.2.3", errors[0])

    def test_cyclonedx_requires_components_and_reproducible_identity(self) -> None:
        document = {
            "bomFormat": "CycloneDX",
            "specVersion": "1.5",
            "metadata": {"component": {"name": "ryframe"}},
            "components": [{"name": "dependency"}],
        }
        with self.temporary_directory() as raw:
            path = self.write_json(Path(raw), "bom.json", document)
            self.assertEqual(
                MODULE.validate_cyclonedx(path, require_reproducible=True),
                [],
            )
            document["serialNumber"] = "urn:uuid:random"
            self.write_json(Path(raw), "bom.json", document)
            self.assertEqual(MODULE.validate_cyclonedx(path), [])
            errors = MODULE.validate_cyclonedx(path, require_reproducible=True)
        self.assertTrue(any("serialNumber" in error for error in errors))


if __name__ == "__main__":
    unittest.main()
