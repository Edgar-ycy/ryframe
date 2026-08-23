from __future__ import annotations

import datetime as dt
import importlib.util
import json
import shutil
import textwrap
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


def policy(
    *,
    vulnerabilities: list[dict[str, str]] | None = None,
    service_images: list[dict[str, str]] | None = None,
) -> dict[str, object]:
    return {
        "schema_version": 1,
        "tools": {
            "cargo-audit": "0.22.2",
            "cargo-deny": "0.20.2",
            "cargo-cyclonedx": "0.5.9",
            "sccache": "0.17.0",
            "trivy": "0.72.0",
        },
        "service_images": service_images or [],
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


def workflow_with_run_steps(run_steps: str) -> str:
    steps = textwrap.indent(textwrap.dedent(run_steps).strip(), "      ")
    return f"""
jobs:
  supply-chain:
    steps:
      - uses: taiki-e/install-action@2222222222222222222222222222222222222222
        with:
          tool: cargo-audit@0.22.2,cargo-deny@0.20.2,cargo-cyclonedx@0.5.9,sccache@0.17.0,trivy@0.72.0
          fallback: none
{steps}
      - uses: actions/upload-artifact@3333333333333333333333333333333333333333
"""


def workflow_with_service_image(service: str, image: str) -> str:
    return workflow_with_run_steps(
        """
- run: cargo cyclonedx --format json
- run: trivy image --format cyclonedx target
- run: python scripts/check_supply_chain.py --trivy-report report.json
"""
    ).replace(
        "  supply-chain:\n    steps:",
        f"  supply-chain:\n    services:\n      {service}:\n"
        f"        image: {image}\n    steps:",
    )


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

    def test_ci_yaml_parser_requirement_is_exact_and_hashed(self) -> None:
        requirements = (SCRIPT.parent / "requirements-ci.txt").read_text(
            encoding="utf-8"
        )
        self.assertIn("PyYAML==6.0.3", requirements)
        self.assertEqual(requirements.count("--hash=sha256:"), 5)

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

    def test_policy_rejects_unsafe_duplicate_or_mutable_service_images(self) -> None:
        digest = "a" * 64
        entry = {
            "workflow": "ci.yml",
            "job": "integration",
            "service": "redis",
            "image": f"redis:7.4@sha256:{digest}",
        }
        invalid_policies = (
            policy(service_images=[entry, dict(entry)]),
            policy(service_images=[{**entry, "workflow": "../ci.yml"}]),
            policy(service_images=[{**entry, "image": "redis:7.4"}]),
            policy(service_images=[{**entry, "image": f"redis@sha256:{digest}"}]),
            policy(service_images=[{**entry, "unknown": "value"}]),
        )
        with self.temporary_directory() as raw:
            directory = Path(raw)
            for index, invalid in enumerate(invalid_policies):
                path = self.write_json(directory, f"policy-{index}.json", invalid)
                with self.assertRaises(MODULE.PolicyError):
                    MODULE.load_policy(path, today=dt.date(2026, 8, 20))

    def test_workflow_service_set_and_full_reference_must_match_policy(self) -> None:
        digest = "a" * 64
        exact_image = f"redis:7.4@sha256:{digest}"
        expected = {
            "workflow": "ci.yml",
            "job": "supply-chain",
            "service": "redis",
            "image": exact_image,
        }
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            workflow_path = workflow_dir / "ci.yml"

            workflow_path.write_text(
                workflow_with_service_image("redis", exact_image),
                encoding="utf-8",
            )
            self.assertEqual(
                MODULE.validate_workflows(
                    workflow_dir,
                    policy(service_images=[expected]),
                ),
                [],
            )

            tag_drift = f"redis:7.5@sha256:{digest}"
            workflow_path.write_text(
                workflow_with_service_image("redis", tag_drift),
                encoding="utf-8",
            )
            tag_errors = MODULE.validate_workflows(
                workflow_dir,
                policy(service_images=[expected]),
            )
            self.assertTrue(any("服务镜像引用漂移" in error for error in tag_errors))

            digest_drift = f"redis:7.4@sha256:{'b' * 64}"
            workflow_path.write_text(
                workflow_with_service_image("redis", digest_drift),
                encoding="utf-8",
            )
            digest_errors = MODULE.validate_workflows(
                workflow_dir,
                policy(service_images=[expected]),
            )
            self.assertTrue(
                any("服务镜像引用漂移" in error for error in digest_errors)
            )

            workflow_path.write_text(
                workflow_with_run_steps(
                    """
- run: cargo cyclonedx --format json
- run: trivy image --format cyclonedx target
- run: python scripts/check_supply_chain.py --trivy-report report.json
"""
                ),
                encoding="utf-8",
            )
            missing_errors = MODULE.validate_workflows(
                workflow_dir,
                policy(service_images=[expected]),
            )
            self.assertTrue(
                any("策略声明的服务镜像不存在" in error for error in missing_errors)
            )

            workflow_path.write_text(
                workflow_with_service_image("redis", exact_image),
                encoding="utf-8",
            )
            extra_errors = MODULE.validate_workflows(workflow_dir, policy())
            self.assertTrue(
                any("工作流服务镜像未在策略声明" in error for error in extra_errors)
            )

    def test_workflow_requires_action_sha_image_digest_and_exact_tools(self) -> None:
        mysql_image = (
            "mysql:8.4@sha256:"
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        )
        configured = policy(
            service_images=[
                {
                    "workflow": "ci.yml",
                    "job": "integration",
                    "service": "mysql",
                    "image": mysql_image,
                }
            ]
        )
        pinned = """
jobs:
  integration:
    services:
      mysql:
        image: mysql:8.4@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
    steps:
      - uses: owner/action@1111111111111111111111111111111111111111
      - uses: docker://owner/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
      - name: install
        uses: taiki-e/install-action@2222222222222222222222222222222222222222
        with:
          tool: cargo-audit@0.22.2,cargo-deny@0.20.2,cargo-cyclonedx@0.5.9,sccache@0.17.0,trivy@0.72.0
          fallback: none
      - run: docker run owner/image@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb check
      - run: cargo cyclonedx --format json
      - run: trivy image --format cyclonedx target
      - run: python scripts/check_supply_chain.py --trivy-report report.json
      - uses: actions/upload-artifact@3333333333333333333333333333333333333333
"""
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            (workflow_dir / "ci.yml").write_text(pinned, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, configured)
            self.assertEqual(errors, [])

            mutable = (
                pinned.replace(
                    "owner/action@1111111111111111111111111111111111111111",
                    "owner/action@v1",
                )
                .replace(
                    "owner/image@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                    "owner/image:latest",
                )
                .replace(
                    "mysql:8.4@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
                    "mysql:8.4",
                )
            )
            (workflow_dir / "ci.yml").write_text(mutable, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, configured)
        self.assertTrue(any("action 未固定" in error for error in errors))
        self.assertTrue(any("docker run 镜像未固定" in error for error in errors))
        self.assertTrue(any("service mysql 镜像未固定" in error for error in errors))

    def test_workflow_service_requires_exactly_one_image(self) -> None:
        missing = """
jobs:
  integration:
    services:
      redis:
        ports:
          - 6379:6379
"""
        duplicate = """
jobs:
  integration:
    services:
      redis:
        image: redis@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        image: redis@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
"""
        path = Path("ci.yml")
        self.assertTrue(
            any(
                "缺少固定 image" in error
                for error in MODULE.validate_service_images(path, missing)
            )
        )
        self.assertTrue(
            any(
                "重复键" in error
                for error in MODULE.validate_service_images(path, duplicate)
            )
        )

    def test_workflow_service_supports_inline_maps_and_aliases(self) -> None:
        digest = "a" * 64
        inline = (
            "jobs: {integration: {services: {redis: "
            f"{{image: 'redis:7.4@sha256:{digest}'}}}}}}}}\n"
        )
        alias = f"""
jobs:
  integration:
    services: &shared-services
      redis:
        image: redis:7.4@sha256:{digest}
  repeated:
    services: *shared-services
"""
        path = Path("ci.yml")
        self.assertEqual(MODULE.validate_service_images(path, inline), [])
        self.assertEqual(MODULE.validate_service_images(path, alias), [])

        mutable_alias = alias.replace(f"redis:7.4@sha256:{digest}", "redis:latest")
        errors = MODULE.validate_service_images(path, mutable_alias)
        self.assertEqual(sum("镜像未固定" in error for error in errors), 2)

        expression = inline.replace(
            f"redis:7.4@sha256:{digest}",
            f"${{{{env.SERVICE_IMAGE}}}}@sha256:{digest}",
        )
        self.assertTrue(
            any(
                "镜像未固定" in error
                for error in MODULE.validate_service_images(path, expression)
            )
        )

    def test_workflow_accepts_empty_service_map_and_rejects_null(self) -> None:
        empty = """
jobs:
  integration:
    services: {}
  check:
    runs-on: ubuntu-latest
"""
        invalid = """
jobs:
  integration:
    services:
"""
        path = Path("ci.yml")
        self.assertEqual(MODULE.validate_service_images(path, empty), [])
        self.assertTrue(
            any(
                "services 必须是对象" in error
                for error in MODULE.validate_service_images(path, invalid)
            )
        )

    def test_workflow_uses_yaml_12_boolean_rules_for_names(self) -> None:
        digest = "b" * 64
        workflow = f"""
jobs:
  on:
    services:
      yes:
        image: redis@sha256:{digest}
"""
        self.assertEqual(
            MODULE.validate_service_images(Path("ci.yml"), workflow),
            [],
        )

    def test_workflow_actions_support_inline_maps_and_job_reusable_workflow(
        self,
    ) -> None:
        action_sha = "1" * 40
        workflow_sha = "2" * 40
        install_sha = "3" * 40
        digest = "4" * 64
        workflow = (
            "jobs: {reusable: {uses: "
            f"'owner/repo/.github/workflows/check.yml@{workflow_sha}'"
            "}, check: {steps: ["
            f"{{uses: 'owner/action@{action_sha}'}}, "
            f"{{uses: 'docker://owner/image@sha256:{digest}'}}, "
            "{uses: './.github/actions/local'}, "
            f"{{uses: 'taiki-e/install-action@{install_sha}', "
            "with: {tool: 'sccache@0.17.0', fallback: 'none'}}"
            "]}}\n"
        )
        path = Path("ci.yml")
        self.assertEqual(MODULE.validate_action_uses(path, workflow, policy()), [])

        mutable = workflow.replace(f"owner/action@{action_sha}", "owner/action@main")
        self.assertTrue(
            any(
                "action 未固定" in error
                for error in MODULE.validate_action_uses(path, mutable, policy())
            )
        )
        mutable_workflow = workflow.replace(
            f"check.yml@{workflow_sha}",
            "check.yml@main",
        )
        self.assertTrue(
            any(
                "可复用工作流 未固定" in error
                for error in MODULE.validate_action_uses(
                    path,
                    mutable_workflow,
                    policy(),
                )
            )
        )

    def test_workflow_actions_follow_aliases_and_validate_install_configuration(
        self,
    ) -> None:
        install_sha = "5" * 40
        workflow = f"""
jobs:
  check:
    steps: &shared-steps
      - &install-step
        uses: taiki-e/install-action@{install_sha}
        with:
          tool: sccache@0.17.0
          fallback: none
  repeated:
    steps: *shared-steps
"""
        path = Path("ci.yml")
        self.assertEqual(MODULE.validate_action_uses(path, workflow, policy()), [])

        fallback = workflow.replace("fallback: none", "fallback: automatic")
        fallback_errors = MODULE.validate_action_uses(path, fallback, policy())
        self.assertEqual(
            sum("必须禁用 fallback" in error for error in fallback_errors), 2
        )

        drifting = workflow.replace("sccache@0.17.0", "sccache@latest")
        drift_errors = MODULE.validate_action_uses(path, drifting, policy())
        self.assertEqual(sum("未固定到完整版本" in error for error in drift_errors), 2)

        mixed_case = workflow.replace(
            "taiki-e/install-action@",
            "Taiki-E/Install-Action@",
        ).replace("fallback: none", "fallback: automatic")
        mixed_case_errors = MODULE.validate_action_uses(path, mixed_case, policy())
        self.assertEqual(
            sum("必须禁用 fallback" in error for error in mixed_case_errors),
            2,
        )

    def test_workflow_actions_reject_duplicate_keys_and_non_object_steps(self) -> None:
        action_sha = "6" * 40
        duplicate = f"""
jobs:
  check:
    steps:
      - uses: owner/action@{action_sha}
        uses: owner/action@{action_sha}
"""
        invalid_steps = f"""
jobs:
  mapping:
    steps: {{uses: owner/action@{action_sha}}}
  entries:
    steps:
      - 42
      - uses: [owner/action@{action_sha}]
      - run: [cargo, cyclonedx]
"""
        path = Path("ci.yml")
        self.assertTrue(
            any(
                "重复键" in error
                for error in MODULE.validate_action_uses(path, duplicate, policy())
            )
        )
        errors = MODULE.validate_action_uses(path, invalid_steps, policy())
        self.assertTrue(any("steps 必须是数组" in error for error in errors))
        self.assertTrue(any("steps[0] 必须是对象" in error for error in errors))
        self.assertTrue(any("uses 必须是静态字符串" in error for error in errors))
        self.assertTrue(any("run 必须是静态字符串" in error for error in errors))

    def test_workflow_command_gates_ignore_comments_and_echo_text(self) -> None:
        workflow = workflow_with_run_steps(
            """
- run: |
    # cargo cyclonedx --format json
    cargo cyclonedx
    echo "--format json"
    trivy image --format json target
    echo "--format cyclonedx"
    python scripts/check_supply_chain.py
    echo "--trivy-report report.json"
    echo "owner/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    docker run --label owner/fake@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa owner/image:latest
"""
        )
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            (workflow_dir / "ci.yml").write_text(workflow, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, policy())
        self.assertTrue(
            any("cargo cyclonedx --format json" in error for error in errors)
        )
        self.assertTrue(
            any("trivy image --format cyclonedx" in error for error in errors)
        )
        self.assertTrue(any("--trivy-report <报告>" in error for error in errors))
        self.assertTrue(any("docker run 镜像未固定" in error for error in errors))

    def test_workflow_command_gates_accept_literal_and_folded_commands(self) -> None:
        digest = "b" * 64
        workflow = workflow_with_run_steps(
            f"""
- run: |
    cargo cyclonedx \\
      --manifest-path crates/ryframe/Cargo.toml \\
      --format json
    python scripts/check_supply_chain.py \\
      --trivy-report report.json
    docker run --rm \\
      owner/image@sha256:{digest} check
- run: >-
    trivy image
    --format cyclonedx
    target
- run: >-
    docker pull
    owner/image@sha256:{digest}
"""
        )
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            (workflow_dir / "ci.yml").write_text(workflow, encoding="utf-8")
            self.assertEqual(MODULE.validate_workflows(workflow_dir, policy()), [])

    def test_workflow_validation_fails_closed_without_yaml_parser(self) -> None:
        parser = MODULE.yaml
        try:
            MODULE.yaml = None
            errors = MODULE.validate_service_images(Path("ci.yml"), "jobs: {}\n")
        finally:
            MODULE.yaml = parser
        self.assertTrue(any("缺少 PyYAML" in error for error in errors))

    def test_trivy_gate_matches_exact_package_version_and_rejects_unused_exception(
        self,
    ) -> None:
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
