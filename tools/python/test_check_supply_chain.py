from __future__ import annotations

import datetime as dt
import hashlib
import importlib.util
import json
import shutil
import textwrap
import unittest
import uuid
from contextlib import contextmanager
from collections.abc import Iterator
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch


SCRIPT = Path(__file__).resolve().parent / "check_supply_chain.py"
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
    local_patch_licenses: list[dict[str, object]] | None = None,
) -> dict[str, object]:
    aws_required_packages = ["aws-lc-rs", "aws-lc-sys"]
    aws_required_features = {
        "jsonwebtoken": ["aws_lc_rs"],
        "reqwest": ["__rustls-aws-lc-rs"],
        "rustls": ["aws_lc_rs"],
        "sqlx": ["tls-rustls-aws-lc-rs"],
        "tokio-rustls": ["aws_lc_rs"],
    }

    def profile(
        name: str,
        package: str,
        features: list[str],
        *,
        required_packages: list[str] | None = None,
        required_features: dict[str, list[str]] | None = None,
        forbidden_packages: list[str] | None = None,
        forbidden_features: dict[str, list[str]] | None = None,
        maximum: int | None = None,
    ) -> dict[str, object]:
        return {
            "name": name,
            "command": {
                "package": package,
                "no_default_features": True,
                "features": features,
                "edges": ["normal", "build"],
            },
            "constraints": {
                "required_packages": list(required_packages or []),
                "required_features": {
                    package: list(features)
                    for package, features in (required_features or {}).items()
                },
                "forbidden_packages": list(forbidden_packages or []),
                "forbidden_features": {
                    package: list(features)
                    for package, features in (forbidden_features or {}).items()
                },
                "max_unique_packages": maximum,
            },
        }

    return {
        "schema_version": 4,
        "tools": {
            "cargo-audit": "0.22.2",
            "cargo-deny": "0.20.2",
            "cargo-cyclonedx": "0.5.9",
            "sccache": "0.17.0",
            "trivy": "0.72.0",
        },
        "local_patch_licenses": local_patch_licenses or [],
        "service_images": service_images or [],
        "vulnerability_gate": {
            "severities": ["HIGH", "CRITICAL"],
            "exceptions": vulnerabilities or [],
        },
        "runtime_feature_tree_gate": {
            "profiles": [
                profile(
                    "API",
                    "ryframe",
                    ["bin-api", "runtime-swagger-ui"],
                    required_packages=aws_required_packages,
                    required_features=aws_required_features,
                    forbidden_packages=["ring"],
                    forbidden_features={"rustls": ["ring"]},
                ),
                profile(
                    "Worker",
                    "ryframe",
                    ["bin-worker"],
                    required_packages=aws_required_packages,
                    required_features=aws_required_features,
                    forbidden_packages=["ring", "ryframe-api"],
                    forbidden_features={"rustls": ["ring"]},
                ),
                profile(
                    "Migrate",
                    "ryframe",
                    ["bin-migrate"],
                    forbidden_packages=["ryframe-api", "axum", "reqwest"],
                ),
                profile(
                    "GeneratorDefault",
                    "ryframe-generator",
                    [],
                    forbidden_packages=["sea-orm", "sqlx", "ryframe-tenant-db", "tokio"],
                    maximum=90,
                ),
            ],
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


def local_patch_policy_entry(
    mit: bytes,
    apache: bytes,
) -> dict[str, object]:
    return {
        "patch": "fixture-patch",
        "package": "fixture-package",
        "version": "1.2.3",
        "path": "vendor/fixture-package",
        "license_expression": "MIT OR Apache-2.0",
        "license_files": [
            {
                "spdx": "MIT",
                "file": "LICENSE-MIT",
                "sha256": hashlib.sha256(mit).hexdigest(),
                "registry_source": {
                    "package": "fixture-source",
                    "version": "1.2.3",
                    "file": "LICENSE-MIT",
                },
            },
            {
                "spdx": "Apache-2.0",
                "file": "LICENSE-APACHE",
                "sha256": hashlib.sha256(apache).hexdigest(),
                "registry_source": {
                    "package": "fixture-source",
                    "version": "1.2.3",
                    "file": "LICENSE-APACHE",
                },
            },
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
- run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible
- run: cargo xtask check ci security report cyclonedx --input /tmp/image.json
- run: cargo xtask check ci security report trivy --input /tmp/trivy.json
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

    def write_local_patch_fixture(
        self, root: Path
    ) -> tuple[dict[str, object], Path, Path, Path]:
        mit = b"fixture MIT license\n"
        apache = b"fixture Apache license\n"
        vendor = root / "vendor" / "fixture-package"
        vendor.mkdir(parents=True)
        (root / "Cargo.toml").write_text(
            textwrap.dedent(
                """
                [patch.crates-io]
                fixture-patch = { path = "vendor/fixture-package" }
                """
            ).strip()
            + "\n",
            encoding="utf-8",
        )
        (vendor / "Cargo.toml").write_text(
            textwrap.dedent(
                """
                [package]
                name = "fixture-package"
                version = "1.2.3"
                license = "MIT OR Apache-2.0"
                """
            ).strip()
            + "\n",
            encoding="utf-8",
        )
        (vendor / "LICENSE-MIT").write_bytes(mit)
        (vendor / "LICENSE-APACHE").write_bytes(apache)

        cargo_home = root / "cargo-home"
        registry = (
            cargo_home
            / "registry"
            / "src"
            / "fixture-registry"
            / "fixture-source-1.2.3"
        )
        registry.mkdir(parents=True)
        (registry / "LICENSE-MIT").write_bytes(mit)
        (registry / "LICENSE-APACHE").write_bytes(apache)
        configured = policy(
            local_patch_licenses=[local_patch_policy_entry(mit, apache)]
        )
        return configured, cargo_home, vendor, registry

    def test_ci_yaml_parser_requirement_is_exact_and_hashed(self) -> None:
        requirements = (SCRIPT.parents[2] / "scripts/requirements-ci.txt").read_text(
            encoding="utf-8"
        )
        for requirement in (
            "PyYAML==6.0.3",
            "tree-sitter==0.25.2",
            "tree-sitter-rust==0.24.2",
        ):
            self.assertIn(requirement, requirements)
        self.assertEqual(requirements.count("--hash=sha256:"), 11)

    def test_accepts_complete_future_dated_policy(self) -> None:
        with self.temporary_directory() as raw:
            path = self.write_json(Path(raw), "policy.json", policy())
            loaded = MODULE.load_policy(path, today=dt.date(2026, 8, 20))
        self.assertEqual(loaded["tools"]["trivy"], "0.72.0")

    def test_checked_in_feature_profiles_cover_process_and_generator_surfaces(
        self,
    ) -> None:
        loaded = MODULE.load_policy(
            SCRIPT.parents[1] / "policies/supply_chain_policy.json",
            today=dt.date(2026, 8, 20),
        )
        profiles = {
            profile["name"]: profile
            for profile in loaded["runtime_feature_tree_gate"]["profiles"]
        }
        self.assertEqual(
            set(profiles), {"API", "Worker", "Migrate", "GeneratorDefault"}
        )
        self.assertIn(
            "ryframe-api", profiles["Worker"]["constraints"]["forbidden_packages"]
        )
        migrate = profiles["Migrate"]
        self.assertEqual(
            set(migrate["constraints"]["required_features"]), {"rustls", "sqlx"}
        )
        self.assertNotIn(
            "tokio-rustls", migrate["constraints"]["required_features"]
        )
        self.assertTrue(
            {
                "ryframe-api",
                "ryframe-adapters",
                "axum",
                "utoipa",
                "redis",
                "image",
                "opentelemetry-otlp",
                "reqwest",
            }.issubset(migrate["constraints"]["forbidden_packages"])
        )
        generator = profiles["GeneratorDefault"]
        self.assertEqual(generator["command"]["package"], "ryframe-generator")
        self.assertEqual(generator["command"]["features"], [])
        self.assertEqual(generator["constraints"]["max_unique_packages"], 90)
        self.assertTrue(
            {"sea-orm", "sqlx", "ryframe-tenant-db", "tokio"}.issubset(
                generator["constraints"]["forbidden_packages"]
            )
        )

    def test_runtime_feature_tree_policy_rejects_malformed_contracts(self) -> None:
        invalid_policies = []
        missing_profile_features = policy()
        del missing_profile_features["runtime_feature_tree_gate"]["profiles"][0][
            "command"
        ]
        invalid_policies.append(missing_profile_features)
        duplicate_profiles = policy()
        duplicate_profiles["runtime_feature_tree_gate"]["profiles"][1]["name"] = (
            "API"
        )
        invalid_policies.append(duplicate_profiles)
        overlapping_package = policy()
        overlapping_package["runtime_feature_tree_gate"]["profiles"][0][
            "constraints"
        ]["forbidden_packages"] = ["aws-lc-rs"]
        invalid_policies.append(overlapping_package)
        overlapping_feature = policy()
        overlapping_feature["runtime_feature_tree_gate"]["profiles"][0][
            "constraints"
        ]["forbidden_features"] = {"rustls": ["aws_lc_rs"]}
        invalid_policies.append(overlapping_feature)
        invalid_boolean = policy()
        invalid_boolean["runtime_feature_tree_gate"]["profiles"][0]["command"][
            "no_default_features"
        ] = "true"
        invalid_policies.append(invalid_boolean)
        missing_edge = policy()
        missing_edge["runtime_feature_tree_gate"]["profiles"][0]["command"][
            "edges"
        ] = ["normal"]
        invalid_policies.append(missing_edge)
        invalid_maximum = policy()
        invalid_maximum["runtime_feature_tree_gate"]["profiles"][3]["constraints"][
            "max_unique_packages"
        ] = 0
        invalid_policies.append(invalid_maximum)
        duplicate_command = policy()
        duplicate_command["runtime_feature_tree_gate"]["profiles"][1]["command"] = (
            duplicate_command["runtime_feature_tree_gate"]["profiles"][0]["command"]
        )
        invalid_policies.append(duplicate_command)
        missing_required_profile = policy()
        missing_required_profile["runtime_feature_tree_gate"]["profiles"].pop()
        invalid_policies.append(missing_required_profile)

        with self.temporary_directory() as raw:
            directory = Path(raw)
            for index, invalid in enumerate(invalid_policies):
                path = self.write_json(directory, f"runtime-policy-{index}.json", invalid)
                with self.subTest(index=index), self.assertRaises(MODULE.PolicyError):
                    MODULE.load_policy(path, today=dt.date(2026, 8, 20))

    def test_local_patch_license_matches_registry_and_hash_fallback(self) -> None:
        with self.temporary_directory() as raw:
            root = Path(raw)
            configured, cargo_home, _, _ = self.write_local_patch_fixture(root)
            policy_path = self.write_json(root, "policy.json", configured)
            loaded = MODULE.load_policy(policy_path, today=dt.date(2026, 8, 20))
            self.assertEqual(
                MODULE.validate_local_patch_licenses(
                    root, loaded, cargo_home=cargo_home
                ),
                [],
            )
            empty_cargo_home = root / "empty-cargo-home"
            empty_cargo_home.mkdir()
            self.assertEqual(
                MODULE.validate_local_patch_licenses(
                    root, loaded, cargo_home=empty_cargo_home
                ),
                [],
            )

    def test_local_patch_license_hash_is_stable_across_line_endings(self) -> None:
        with self.temporary_directory() as raw:
            root = Path(raw)
            configured, cargo_home, vendor, registry = self.write_local_patch_fixture(
                root
            )
            for directory in (vendor, registry):
                path = directory / "LICENSE-APACHE"
                path.write_bytes(path.read_bytes().replace(b"\n", b"\r\n"))
            self.assertEqual(
                MODULE.validate_local_patch_licenses(
                    root, configured, cargo_home=cargo_home
                ),
                [],
            )

    def test_local_patch_license_accepts_registry_manifest_license(self) -> None:
        with self.temporary_directory() as raw:
            root = Path(raw)
            configured, cargo_home, _, registry = self.write_local_patch_fixture(root)
            source = configured["local_patch_licenses"][0]["license_files"][1][
                "registry_source"
            ]
            source.pop("file")
            source["manifest_license"] = "Apache-2.0"
            (registry / "LICENSE-APACHE").unlink()
            (registry / "Cargo.toml").write_text(
                '[package]\nname = "fixture-source"\nversion = "1.2.3"\n'
                'license = "Apache-2.0"\n',
                encoding="utf-8",
            )
            policy_path = self.write_json(root, "policy.json", configured)
            loaded = MODULE.load_policy(policy_path, today=dt.date(2026, 8, 20))
            self.assertEqual(
                MODULE.validate_local_patch_licenses(
                    root, loaded, cargo_home=cargo_home
                ),
                [],
            )

            (registry / "Cargo.toml").write_text(
                '[package]\nname = "fixture-source"\nversion = "1.2.3"\n'
                'license = "MIT"\n',
                encoding="utf-8",
            )
            errors = MODULE.validate_local_patch_licenses(
                root, loaded, cargo_home=cargo_home
            )
            self.assertTrue(any("registry 清单许可证不一致" in error for error in errors))

    def test_local_patch_license_rejects_vendor_or_registry_drift(self) -> None:
        with self.temporary_directory() as raw:
            root = Path(raw)
            configured, cargo_home, vendor, registry = self.write_local_patch_fixture(
                root
            )
            (vendor / "LICENSE-MIT").write_bytes(b"tampered vendor license\n")
            errors = MODULE.validate_local_patch_licenses(
                root, configured, cargo_home=cargo_home
            )
            self.assertTrue(any("登记哈希不一致" in error for error in errors))

            (vendor / "LICENSE-MIT").write_bytes(b"fixture MIT license\n")
            (registry / "LICENSE-APACHE").write_bytes(b"tampered registry license\n")
            errors = MODULE.validate_local_patch_licenses(
                root, configured, cargo_home=cargo_home
            )
            self.assertTrue(any("registry 原件不一致" in error for error in errors))

    def test_local_patch_license_rejects_missing_source_or_unregistered_patch(
        self,
    ) -> None:
        with self.temporary_directory() as raw:
            root = Path(raw)
            configured, cargo_home, vendor, registry = self.write_local_patch_fixture(
                root
            )
            (vendor / "LICENSE-APACHE").unlink()
            errors = MODULE.validate_local_patch_licenses(
                root, configured, cargo_home=cargo_home
            )
            self.assertTrue(any("Apache-2.0 文件" in error for error in errors))
            (vendor / "LICENSE-APACHE").write_bytes(b"fixture Apache license\n")

            (registry / "LICENSE-MIT").unlink()
            errors = MODULE.validate_local_patch_licenses(
                root, configured, cargo_home=cargo_home
            )
            self.assertTrue(any("registry 原件" in error for error in errors))

            (root / "Cargo.toml").write_text(
                textwrap.dedent(
                    """
                    [patch.crates-io]
                    fixture-patch = { path = "vendor/fixture-package" }
                    unknown = { path = "vendor/unknown" }
                    """
                ).strip()
                + "\n",
                encoding="utf-8",
            )
            errors = MODULE.validate_local_patch_licenses(
                root, configured, cargo_home=root / "empty-cargo-home"
            )
            self.assertTrue(any("未登记许可证原件" in error for error in errors))

    def test_local_patch_policy_requires_complete_safe_license_mapping(self) -> None:
        mit = b"fixture MIT license\n"
        apache = b"fixture Apache license\n"
        invalid_entries = []
        missing_spdx = local_patch_policy_entry(mit, apache)
        missing_spdx["license_files"] = missing_spdx["license_files"][:1]
        invalid_entries.append(missing_spdx)
        unsafe_path = local_patch_policy_entry(mit, apache)
        unsafe_path["path"] = "../vendor/fixture-package"
        invalid_entries.append(unsafe_path)
        bad_hash = local_patch_policy_entry(mit, apache)
        bad_hash["license_files"][0]["sha256"] = "A" * 64
        invalid_entries.append(bad_hash)

        with self.temporary_directory() as raw:
            root = Path(raw)
            for index, entry in enumerate(invalid_entries):
                path = self.write_json(
                    root,
                    f"invalid-policy-{index}.json",
                    policy(local_patch_licenses=[entry]),
                )
                with self.subTest(index=index), self.assertRaises(MODULE.PolicyError):
                    MODULE.load_policy(path, today=dt.date(2026, 8, 20))

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
            self.assertTrue(any("服务镜像引用漂移" in error for error in digest_errors))

            workflow_path.write_text(
                workflow_with_run_steps(
                    """
- run: cargo cyclonedx --format json
- run: trivy image --format cyclonedx target
- run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible
- run: cargo xtask check ci security report cyclonedx --input /tmp/image.json
- run: cargo xtask check ci security report trivy --input /tmp/trivy.json
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
      - run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible
      - run: cargo xtask check ci security report cyclonedx --input /tmp/image.json
      - run: cargo xtask check ci security report trivy --input /tmp/trivy.json
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

    def test_workflow_rejects_runtime_context_in_job_environment(self) -> None:
        workflow = """
jobs:
  integration:
    runs-on: ubuntu-latest
    env:
      ARTIFACT_DIR: ${{ runner.temp }}/integration
    steps:
      - run: echo ok
"""
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            (workflow_dir / "ci.yml").write_text(workflow, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, policy())
        self.assertTrue(any("不能引用 runner 上下文" in error for error in errors))

    def test_workflow_accepts_runtime_context_in_step_environment(self) -> None:
        workflow = """
jobs:
  integration:
    runs-on: ubuntu-latest
    steps:
      - env:
          ARTIFACT_DIR: ${{ runner.temp }}/integration
        run: echo ok
"""
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            (workflow_dir / "ci.yml").write_text(workflow, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, policy())
        self.assertFalse(any("runner 上下文" in error for error in errors))

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
    python tools/python/check_supply_chain.py
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
        self.assertTrue(any("security report trivy" in error for error in errors))
        self.assertTrue(any("docker run 镜像未固定" in error for error in errors))

    def test_workflow_command_gates_accept_literal_and_folded_commands(self) -> None:
        digest = "b" * 64
        workflow = workflow_with_run_steps(
            f"""
- run: |
    cargo cyclonedx \\
      --manifest-path crates/ryframe/Cargo.toml \\
      --format json
    cargo xtask check ci security report cyclonedx \\
      --input /tmp/cargo.json \\
      --require-reproducible
    cargo xtask check ci security report cyclonedx \\
      --input /tmp/image.json
    cargo xtask check ci security report trivy \\
      --input /tmp/trivy.json
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

    def test_workflow_rejects_legacy_report_invocation(self) -> None:
        workflow = workflow_with_run_steps(
            """
- run: cargo cyclonedx --format json
- run: trivy image --format cyclonedx target
- run: cargo xtask check ci security report cyclonedx --input /tmp/cargo.json --require-reproducible
- run: cargo xtask check ci security report cyclonedx --input /tmp/image.json
- run: cargo xtask check ci security report trivy --input /tmp/trivy.json
- run: python tools/python/check_supply_chain.py --cyclonedx /tmp/legacy.json
"""
        )
        with self.temporary_directory() as raw:
            workflow_dir = Path(raw)
            (workflow_dir / "ci.yml").write_text(workflow, encoding="utf-8")
            errors = MODULE.validate_workflows(workflow_dir, policy())
        self.assertTrue(any("禁止直接调用" in error for error in errors))

    def test_typed_security_report_parser_rejects_ambiguous_commands(self) -> None:
        kind, reproducible, errors = MODULE._typed_security_report(
            [
                "cargo",
                "xtask",
                "check",
                "ci",
                "security",
                "report",
                "cyclonedx",
                "--input",
                "/tmp/report.json",
                "--require-reproducible",
            ]
        )
        self.assertEqual((kind, reproducible, errors), ("cyclonedx", True, []))
        _, _, errors = MODULE._typed_security_report(
            [
                "cargo",
                "xtask",
                "check",
                "ci",
                "security",
                "report",
                "trivy",
                "--input",
                "/tmp/report.json",
                "--require-reproducible",
            ]
        )
        self.assertTrue(any("不支持" in error for error in errors))

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

    def test_feature_tree_accepts_aws_lc_ring_compatibility_features(self) -> None:
        tree = """\
aws-lc-rs v1.17.0|alloc,aws-lc-sys,default,ring-io,ring-sig-verify
aws-lc-sys v0.41.0|prebuilt-nasm
jsonwebtoken v11.0.0|aws_lc_rs
reqwest v0.13.4|__rustls,__rustls-aws-lc-rs,rustls
rustls v0.23.40|aws-lc-rs,aws_lc_rs,std,tls12
sqlx v0.9.0|runtime-tokio,tls-rustls-aws-lc-rs
tokio-rustls v0.26.4|aws_lc_rs,aws-lc-rs
"""

        api = policy()["runtime_feature_tree_gate"]["profiles"][0]
        self.assertEqual(MODULE.feature_tree_violations(tree, api), [])

    def test_feature_tree_rejects_actual_ring_package_or_provider(self) -> None:
        tree = """\
aws-lc-rs v1.17.0|default,ring-io,ring-sig-verify
aws-lc-sys v0.41.0|
jsonwebtoken v11.0.0|aws_lc_rs
reqwest v0.13.4|__rustls,rustls
ring v0.17.14|alloc,default
ryframe-api v0.11.3|
rustls v0.23.40|ring,std,tls12
sqlx v0.9.0|tls-rustls-ring
tokio-rustls v0.26.4|ring
"""

        worker = policy()["runtime_feature_tree_gate"]["profiles"][1]
        errors = MODULE.feature_tree_violations(tree, worker)
        self.assertTrue(any("ring package/provider" in error for error in errors))
        self.assertTrue(any("ryframe-api" in error for error in errors))
        self.assertTrue(any("reqwest" in error for error in errors))
        self.assertTrue(any("rustls" in error for error in errors))
        self.assertTrue(any("sqlx" in error for error in errors))

    def test_feature_tree_fails_closed_on_malformed_or_empty_output(self) -> None:
        api = policy()["runtime_feature_tree_gate"]["profiles"][0]
        errors = MODULE.feature_tree_violations("not-a-package\n", api)

        self.assertTrue(any("格式无效" in error for error in errors))
        self.assertTrue(any("没有有效 package" in error for error in errors))
        self.assertTrue(any("缺少 aws-lc-rs" in error for error in errors))
        self.assertTrue(MODULE.feature_tree_violations("", api))

    def test_feature_tree_rejects_missing_aws_lc_package_or_feature(self) -> None:
        tree = """\
aws-lc-rs v1.17.0|default,ring-io,ring-sig-verify
jsonwebtoken v11.0.0|aws_lc_rs
reqwest v0.13.4|__rustls-aws-lc-rs
rustls v0.23.40|aws_lc_rs
sqlx v0.9.0|runtime-tokio
tokio-rustls v0.26.4|aws_lc_rs
"""
        worker = policy()["runtime_feature_tree_gate"]["profiles"][1]
        errors = MODULE.feature_tree_violations(tree, worker)

        self.assertTrue(any("缺少 aws-lc-sys" in error for error in errors))
        self.assertTrue(any("sqlx" in error for error in errors))

    def test_feature_tree_enforces_profile_packages_and_unique_closure(self) -> None:
        profiles = policy()["runtime_feature_tree_gate"]["profiles"]
        migrate = profiles[2]
        migrate_errors = MODULE.feature_tree_violations(
            "ryframe v0.11.3|bin-migrate\naxum v0.8.0|http1\nreqwest v0.13.4|rustls\n",
            migrate,
        )
        self.assertTrue(any("axum" in error for error in migrate_errors))
        self.assertTrue(any("reqwest" in error for error in migrate_errors))

        generator = profiles[3]
        forbidden = MODULE.feature_tree_violations(
            "ryframe-generator v0.11.3|\ntokio v1.0.0|rt\nsea-orm v2.0.0|\n",
            generator,
        )
        self.assertTrue(any("tokio" in error for error in forbidden))
        self.assertTrue(any("sea-orm" in error for error in forbidden))

        generator["constraints"]["max_unique_packages"] = 1
        closure = MODULE.feature_tree_violations(
            "ryframe-generator v0.11.3|\nserde v1.0.0|derive\nserde v1.0.0|derive (*)\n",
            generator,
        )
        self.assertTrue(any("unique package closure" in error for error in closure))

    def test_feature_tree_commands_are_policy_driven(self) -> None:
        profiles = policy()["runtime_feature_tree_gate"]["profiles"]
        self.assertEqual(
            profiles[0]["command"]["features"], ["bin-api", "runtime-swagger-ui"]
        )
        expected_prefix = [
            "tree",
            "--locked",
            "-p",
            "ryframe",
            "--no-default-features",
            "--features",
        ]
        expected_suffix = [
            "--edges",
            "normal,build",
            "--prefix",
            "none",
            "--format",
            "{p}|{f}",
        ]
        self.assertEqual(
            MODULE.feature_tree_args(profiles[0]),
            [*expected_prefix, "bin-api,runtime-swagger-ui", *expected_suffix],
        )
        self.assertEqual(
            MODULE.feature_tree_args(profiles[1]),
            [*expected_prefix, "bin-worker", *expected_suffix],
        )
        self.assertEqual(
            MODULE.feature_tree_args(profiles[3]),
            [
                "tree",
                "--locked",
                "-p",
                "ryframe-generator",
                "--no-default-features",
                *expected_suffix,
            ],
        )

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

    def test_report_main_does_not_repeat_source_checks(self) -> None:
        cyclone = {
            "bomFormat": "CycloneDX",
            "specVersion": "1.5",
            "metadata": {"component": {"name": "ryframe"}},
            "components": [{"name": "dependency"}],
        }
        trivy = {"Results": []}
        with self.temporary_directory() as raw:
            root = Path(raw)
            cyclone_path = self.write_json(root, "cyclone.json", cyclone)
            trivy_path = self.write_json(root, "trivy.json", trivy)
            cyclone_args = SimpleNamespace(
                policy=Path("unused-policy.json"),
                workflow_dir=Path("unused-workflows"),
                trivy_report=None,
                cyclonedx=cyclone_path,
                require_reproducible_cyclonedx=True,
                verify_cargo_graph=False,
            )
            with (
                patch.object(MODULE, "parse_args", return_value=cyclone_args),
                patch.object(MODULE, "load_policy", side_effect=AssertionError),
                patch.object(
                    MODULE, "validate_local_patch_licenses", side_effect=AssertionError
                ),
                patch.object(MODULE, "validate_workflows", side_effect=AssertionError),
            ):
                self.assertEqual(MODULE.main(), 0)

            trivy_args = SimpleNamespace(
                policy=Path("policy.json"),
                workflow_dir=Path("unused-workflows"),
                trivy_report=trivy_path,
                cyclonedx=None,
                require_reproducible_cyclonedx=False,
                verify_cargo_graph=False,
            )
            with (
                patch.object(MODULE, "parse_args", return_value=trivy_args),
                patch.object(MODULE, "load_policy", return_value=policy()),
                patch.object(
                    MODULE, "validate_local_patch_licenses", side_effect=AssertionError
                ),
                patch.object(MODULE, "validate_workflows", side_effect=AssertionError),
            ):
                self.assertEqual(MODULE.main(), 0)


if __name__ == "__main__":
    unittest.main()
