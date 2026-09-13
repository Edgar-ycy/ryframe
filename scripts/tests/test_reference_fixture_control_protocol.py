import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import reference_fixture_environment as fixture_environment
import full_stack_artifacts as fixture_artifact
import full_stack_migration_history as fixture_retention
import reference_fixture_dataset as fixture_dataset
import reference_fixture_request as fixture_request
import reference_fixture_review as fixture_review
import reference_fixture_services as fixture_services
import reference_fixture_source_pair as fixture_source_pair
import reference_fixture_successor as fixture_successor
from reference_fixture_control_protocol import (
    FixtureControlProtocolError,
    PROTOCOL_KEY,
    private_arguments,
    run_private,
)


KIND = "ryframe-reference-fixture-control"


class FixtureControlProtocolTests(unittest.TestCase):
    def setUp(self):
        self.root = str(Path(__file__).resolve().parents[2])

    def protocol(self, domain, operation, write, **fields):
        return json.dumps({
            "backend_dir": self.root,
            "domain": domain,
            "format_version": 1,
            "kind": KIND,
            "operation": operation,
            "write": write,
            **fields,
        }, separators=(",", ":"))

    def arguments(self, domain, schemas, raw, *, positional):
        return private_arguments(
            domain,
            schemas,
            positional_operation=positional,
            argv=[],
            environment={PROTOCOL_KEY: raw},
        )

    def test_reconstructs_exact_operations_for_control_domains(self):
        input_path = str(Path(self.root) / ".local-tests" / "input.json")
        directory = str(Path(self.root) / ".local-tests" / "run")
        environment = self.protocol(
            "environment", "plan", False,
            review=input_path, fixture=input_path, maintenance_build=input_path, side="seed",
        )
        self.assertEqual(
            self.arguments("environment", fixture_environment.PROTOCOL_SCHEMAS, environment, positional=True)[0],
            "plan",
        )
        review = self.protocol(
            "review", "renew", True,
            template=input_path, fixture=input_path, future_root=directory, id="r24-current",
            api_port=18230, worker_port=19230, frontend_port=4200, rustfs_api_port=29210,
            rustfs_console_port=29211, redis_port=16391, output=input_path,
        )
        review_args = self.arguments(
            "review", fixture_review.PROTOCOL_SCHEMAS, review, positional=False
        )
        self.assertEqual(review_args[:2], ["--backend-dir", self.root])
        self.assertNotIn("renew", review_args)
        request = self.protocol(
            "request", "publish", True,
            environment=input_path, service_run=directory, id="fresh-base", side="base", output=input_path,
        )
        self.assertNotIn(
            "publish",
            self.arguments("request", fixture_request.PROTOCOL_SCHEMAS, request, positional=False),
        )
        source_pair = self.protocol(
            "source-pair", "publish", True, output=input_path
        )
        self.assertEqual(
            self.arguments(
                "source-pair",
                fixture_source_pair.PROTOCOL_SCHEMAS,
                source_pair,
                positional=False,
            ),
            ["--backend-dir", self.root, "--output", input_path, "--write"],
        )
        services = self.protocol(
            "services", "status", False, review=input_path, environment=input_path
        )
        self.assertEqual(
            self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, services, positional=True)[0],
            "status",
        )
        artifact = self.protocol(
            "artifact", "snapshot", True,
            runtime_dir=directory, job_id="9223372036854775807", receipt=input_path,
        )
        artifact_args = self.arguments(
            "artifact", fixture_artifact.PROTOCOL_SCHEMAS, artifact, positional=True
        )
        self.assertEqual(artifact_args[0], "snapshot")
        self.assertIn("9223372036854775807", artifact_args)
        self.assertIn("--write", artifact_args)
        dataset_plan = self.protocol(
            "dataset", "plan", True,
            environment=input_path, runtime=directory,
            work_dir=str(Path(directory) / "dataset work"), output=input_path,
            side="base",
        )
        dataset_plan_args = self.arguments(
            "dataset", fixture_dataset.PROTOCOL_SCHEMAS, dataset_plan,
            positional=True,
        )
        self.assertEqual(dataset_plan_args[0], "plan")
        self.assertIn("--work-dir", dataset_plan_args)
        self.assertIn("--output", dataset_plan_args)
        self.assertIn("--write", dataset_plan_args)
        dataset_prepare = self.protocol(
            "dataset", "prepare", True,
            environment=input_path, runtime=directory, plan=input_path,
            side="candidate",
        )
        dataset_prepare_args = self.arguments(
            "dataset", fixture_dataset.PROTOCOL_SCHEMAS, dataset_prepare,
            positional=True,
        )
        self.assertEqual(dataset_prepare_args[0], "prepare")
        self.assertIn("--plan", dataset_prepare_args)
        self.assertNotIn("--work-dir", dataset_prepare_args)
        self.assertIn("--write", dataset_prepare_args)
        retention = self.protocol(
            "retention", "historical-expired", True,
            runtime_dir=directory, tenant="tenant-0123abcd", migration="123",
            plan_sha256="a" * 64,
        )
        retention_args = self.arguments(
            "retention", fixture_retention.PROTOCOL_SCHEMAS, retention,
            positional=True,
        )
        self.assertEqual(retention_args[0], "historical-expired")
        self.assertIn("123", retention_args)
        self.assertIn("a" * 64, retention_args)
        self.assertIn("--write", retention_args)
        successor = self.protocol(
            "successor", "arm-request", False,
            successor=input_path, source_export_result=input_path, workspace=directory,
            id="arm-base", side="base", copy_directory=directory,
        )
        self.assertEqual(
            self.arguments("successor", fixture_successor.PROTOCOL_SCHEMAS, successor, positional=True)[0],
            "arm-request",
        )

    def test_rejects_duplicate_unknown_wrong_domain_and_ambiguous_write(self):
        path = str(Path(self.root) / ".local-tests" / "input.json")
        valid = self.protocol(
            "services", "status", False, review=path, environment=path
        )
        cases = [
            valid[:-1] + ',"review":"' + path.replace("\\", "\\\\") + '"}',
            self.protocol("services", "status", False, review=path, environment=path, unknown="x"),
            self.protocol("request", "status", False, review=path, environment=path),
            self.protocol("services", "status", True, review=path, environment=path),
            valid.replace('"format_version":1', '"format_version":true'),
        ]
        non_text_operation = json.loads(valid)
        non_text_operation["operation"] = []
        cases.append(json.dumps(non_text_operation, separators=(",", ":")))
        for raw in cases:
            with self.subTest(raw=raw), self.assertRaises(FixtureControlProtocolError):
                self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, raw, positional=True)
        missing_output = self.protocol(
            "successor", "generation-request", True,
            successor=path, source_backend=str(Path(self.root) / ".local-tests"),
            expected_head="a" * 40, backend_build=path, maintenance_build=path,
            source_environment=path, id="source-r24",
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "写入授权"):
            self.arguments(
                "successor", fixture_successor.PROTOCOL_SCHEMAS,
                missing_output, positional=True,
            )

    def test_rejects_public_argv_related_environment_and_non_absolute_paths(self):
        path = str(Path(self.root) / ".local-tests" / "input.json")
        raw = self.protocol(
            "services", "status", False, review=path, environment=path
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "不接受命令行参数"):
            private_arguments(
                "services", fixture_services.PROTOCOL_SCHEMAS,
                positional_operation=True, argv=["status"], environment={PROTOCOL_KEY: raw},
            )
        with self.assertRaisesRegex(FixtureControlProtocolError, "未知字段"):
            private_arguments(
                "services", fixture_services.PROTOCOL_SCHEMAS, positional_operation=True, argv=[],
                environment={PROTOCOL_KEY: raw, "RYFRAME_REFERENCE_FIXTURE_CONTROL_EXTRA": "x"},
            )
        relative = self.protocol(
            "services", "status", False, review="relative.json", environment=path
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "绝对路径"):
            self.arguments("services", fixture_services.PROTOCOL_SCHEMAS, relative, positional=True)

    def test_artifact_protocol_requires_string_positive_i64_and_exact_write_policy(self):
        path = str(Path(self.root) / ".local-tests" / "input.json")
        directory = str(Path(self.root) / ".local-tests" / "run")
        for job_id in (0, "0", "01", "9223372036854775808", "invalid"):
            raw = self.protocol(
                "artifact", "snapshot", True,
                runtime_dir=directory, job_id=job_id, receipt=path,
            )
            with self.subTest(job_id=job_id), self.assertRaisesRegex(
                FixtureControlProtocolError, "正 i64"
            ):
                self.arguments(
                    "artifact", fixture_artifact.PROTOCOL_SCHEMAS, raw, positional=True
                )
        readonly = self.protocol(
            "artifact", "verify-deleted", True,
            runtime_dir=directory, job_id="1", receipt=path,
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "写入授权"):
            self.arguments(
                "artifact", fixture_artifact.PROTOCOL_SCHEMAS, readonly, positional=True
            )

        for field in ("runtime_dir", "receipt"):
            unsafe_path = json.loads(self.protocol(
                "artifact", "snapshot", True,
                runtime_dir=directory, job_id="1", receipt=path,
            ))
            unsafe_path[field] = "relative/path"
            with self.subTest(field=field), self.assertRaisesRegex(
                FixtureControlProtocolError, "绝对路径"
            ):
                self.arguments(
                    "artifact", fixture_artifact.PROTOCOL_SCHEMAS,
                    json.dumps(unsafe_path, separators=(",", ":")),
                    positional=True,
                )

    def test_retention_protocol_rejects_invalid_identity_digest_and_write_policy(self):
        directory = str(Path(self.root) / ".local-tests" / "run")
        valid = dict(
            runtime_dir=directory,
            tenant="tenant-0123abcd",
            migration="1",
        )
        for migration in (0, "0", "01", "9223372036854775808", "invalid"):
            raw = self.protocol(
                "retention", "inspect", False, **{**valid, "migration": migration}
            )
            with self.subTest(migration=migration), self.assertRaisesRegex(
                FixtureControlProtocolError, "正 i64"
            ):
                self.arguments(
                    "retention", fixture_retention.PROTOCOL_SCHEMAS, raw,
                    positional=True,
                )
        for tenant in ("tenant-0123ABCD", "tenant-0123abc", "other-0123abcd"):
            raw = self.protocol(
                "retention", "inspect", False, **{**valid, "tenant": tenant}
            )
            with self.subTest(tenant=tenant), self.assertRaisesRegex(
                FixtureControlProtocolError, "tenant"
            ):
                self.arguments(
                    "retention", fixture_retention.PROTOCOL_SCHEMAS, raw,
                    positional=True,
                )
        for digest in ("a" * 63, "A" * 64, "g" * 64):
            raw = self.protocol(
                "retention", "historical-expired", True,
                **{**valid, "plan_sha256": digest},
            )
            with self.subTest(digest=digest), self.assertRaisesRegex(
                FixtureControlProtocolError, "小写十六进制"
            ):
                self.arguments(
                    "retention", fixture_retention.PROTOCOL_SCHEMAS, raw,
                    positional=True,
                )
        for operation, write in (("inspect", True), ("export-backup", False)):
            raw = self.protocol("retention", operation, write, **valid)
            with self.subTest(operation=operation), self.assertRaisesRegex(
                FixtureControlProtocolError, "写入授权"
            ):
                self.arguments(
                    "retention", fixture_retention.PROTOCOL_SCHEMAS, raw,
                    positional=True,
                )
        relative = self.protocol(
            "retention", "inspect", False, **{**valid, "runtime_dir": "relative"}
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "绝对路径"):
            self.arguments(
                "retention", fixture_retention.PROTOCOL_SCHEMAS, relative,
                positional=True,
            )
        missing_plan = self.protocol(
            "retention", "historical-expired", True, **valid
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "字段不完整"):
            self.arguments(
                "retention", fixture_retention.PROTOCOL_SCHEMAS, missing_plan,
                positional=True,
            )
        unexpected_plan = self.protocol(
            "retention", "inspect", False,
            **{**valid, "plan_sha256": "a" * 64},
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "未知字段"):
            self.arguments(
                "retention", fixture_retention.PROTOCOL_SCHEMAS, unexpected_plan,
                positional=True,
            )

    def test_dataset_protocol_requires_exact_paths_fields_and_write_policy(self):
        path = str(Path(self.root) / ".local-tests" / "input.json")
        directory = str(Path(self.root) / ".local-tests" / "run")
        plan = dict(
            environment=path,
            runtime=directory,
            work_dir=str(Path(directory) / "dataset"),
            output=str(Path(directory) / "dataset-plan.json"),
            side="base",
        )
        prepare = dict(
            environment=path,
            runtime=directory,
            plan=str(Path(directory) / "dataset-plan.json"),
            side="candidate",
        )
        for operation, values in (("plan", plan), ("prepare", prepare)):
            raw = self.protocol("dataset", operation, False, **values)
            with self.subTest(operation=operation), self.assertRaisesRegex(
                FixtureControlProtocolError, "写入授权"
            ):
                self.arguments(
                    "dataset", fixture_dataset.PROTOCOL_SCHEMAS, raw,
                    positional=True,
                )
        unexpected_plan = self.protocol(
            "dataset", "plan", True, **{**plan, "plan": path}
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "未知字段"):
            self.arguments(
                "dataset", fixture_dataset.PROTOCOL_SCHEMAS, unexpected_plan,
                positional=True,
            )
        missing_plan = self.protocol(
            "dataset", "prepare", True,
            **{name: value for name, value in prepare.items() if name != "plan"},
        )
        with self.assertRaisesRegex(FixtureControlProtocolError, "字段不完整"):
            self.arguments(
                "dataset", fixture_dataset.PROTOCOL_SCHEMAS, missing_plan,
                positional=True,
            )
        for field in ("environment", "runtime", "work_dir", "output"):
            relative = self.protocol(
                "dataset", "plan", True, **{**plan, field: "relative/path"}
            )
            with self.subTest(field=field), self.assertRaisesRegex(
                FixtureControlProtocolError, "绝对路径"
            ):
                self.arguments(
                    "dataset", fixture_dataset.PROTOCOL_SCHEMAS, relative,
                    positional=True,
                )

    def test_all_direct_python_programs_reject_argv_before_business_logic(self):
        scripts = [
            "full_stack_artifacts.py",
            "full_stack_migration_history.py",
            "prepare_full_stack_fixture.py",
            "reference_fixture_dataset.py",
            "reference_fixture_environment.py",
            "reference_fixture_review.py",
            "reference_fixture_request.py",
            "reference_fixture_successor.py",
            "reference_fixture_services.py",
            "reference_fixture_source_pair.py",
        ]
        environment = dict(os.environ)
        environment.pop(PROTOCOL_KEY, None)
        for name in scripts:
            script = Path(__file__).resolve().parents[1] / name
            with self.subTest(script=name):
                result = subprocess.run(
                    [sys.executable, "-X", "utf8", "-B", str(script), "untrusted-argv"],
                    cwd=script.parent.parent,
                    env=environment,
                    text=True,
                    encoding="utf-8",
                    capture_output=True,
                    check=False,
                )
                self.assertEqual(result.returncode, 2)
                self.assertIn("protocol_error", result.stderr)

    def test_retention_protocol_is_removed_before_business_dispatch(self):
        directory = str(Path(self.root) / ".local-tests" / "run")
        raw = self.protocol(
            "retention", "inspect", False,
            runtime_dir=directory, tenant="tenant-0123abcd", migration="1",
        )
        observed = []

        def fail_after_protocol(*_args):
            observed.append(PROTOCOL_KEY in os.environ)
            raise ValueError("expected fixture failure")

        with (
            mock.patch.object(fixture_retention, "run", side_effect=fail_after_protocol),
            mock.patch.object(sys, "argv", ["full_stack_migration_history.py"]),
            mock.patch.dict(os.environ, {PROTOCOL_KEY: raw}),
        ):
            self.assertEqual(
                run_private(
                    "retention",
                    fixture_retention.PROTOCOL_SCHEMAS,
                    fixture_retention.main,
                    positional_operation=True,
                ),
                1,
            )
            self.assertIn(PROTOCOL_KEY, os.environ)
        self.assertEqual(observed, [False])


if __name__ == "__main__":
    unittest.main()
