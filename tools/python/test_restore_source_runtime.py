"""同代 source-generation 来源验收的收据、进程和未知写入回归。"""

import contextlib
import copy
import datetime as dt
import io
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash
import restore_source as source_cli
import restore_source_runtime as runtime
import restore_source_runtime_producer as producer
from workspace_directory import WorkspaceDirectory
import source_fingerprints
from test_source_fingerprints import inventory


def descriptor_file(path: Path, value=None) -> dict:
    path.parent.mkdir(parents=True, exist_ok=True)
    write_json(path, value or {"value": path.name})
    return binding(path)


def encoded(value):
    return "NULL" if value is None else str(value).encode().hex().upper()


class FakeProcess:
    def __init__(self, state, result, argv):
        self.pid = 987654
        self.returncode = None
        self.state = state
        self.result = result
        script = Path(argv[1])
        staging = script.parents[1]
        manifest = read_json(staging / "manifest.json")
        contract = next(row for row in manifest["files"] if row["path"] == manifest["contract"])
        value = {
            "format_version": 1,
            "kind": "restore-source-producer-ready",
            "operation_id": argv[argv.index("--operation-id") + 1],
            "source_generation_sha256": argv[argv.index("--source-generation-sha256") + 1],
            "staging_manifest_sha256": binding(staging / "manifest.json")["sha256"],
            "runtime_files_sha256": plan_hash(
                [
                    {"path": row["path"], "bytes": row["bytes"], "sha256": row["sha256"]}
                    for row in manifest["files"]
                    if row["path"] in manifest["runtime_modules"]
                ]
            ),
            "contract_file_sha256": contract["sha256"],
            "lineage_file_sha256": binding(Path(argv[argv.index("--lineage") + 1]))["sha256"],
        }
        self.stdout = io.BytesIO(json.dumps(value, sort_keys=True).encode() + b"\n")

    def communicate(self, *, input, timeout):
        self.state["authorization"] = json.loads(input)
        self.state["alive"] = False
        self.returncode = 0
        return json.dumps(self.result, sort_keys=True).encode() + b"\n", b""

    def wait(self, timeout):
        self.returncode = 137
        self.state["alive"] = False
        return self.returncode


class FailedProcess(FakeProcess):
    def communicate(self, *, input, timeout):
        self.state["authorization"] = json.loads(input)
        self.state["alive"] = False
        self.returncode = 7
        return b"", b"preserved source verification failure"


class InvalidReadyProcess(FakeProcess):
    def __init__(self, state, result, argv):
        super().__init__(state, result, argv)
        value = json.loads(self.stdout.getvalue())
        value["contract_file_sha256"] = "f" * 64
        self.stdout = io.BytesIO(json.dumps(value, sort_keys=True).encode() + b"\n")

    def communicate(self, *, input, timeout):
        raise AssertionError("无效 ready 不能取得服务请求授权")


class FakeResources:
    def __init__(self, lineage, subjects):
        self.lineage = lineage
        self.subjects = subjects
        self.selected = {"redis": {"namespace": "ryframe:{perf-seed-unit}:"}}
        self.deleted = False
        self.old = self.row(
            "10", "system", "old-user", "198.18.20.200", "1",
            "2026-09-01T00:00:00.000000",
        )

    @staticmethod
    def row(row_id, tenant, username, address, status, logged):
        values = [
            row_id, encoded(tenant), encoded(username), encoded(address), encoded(None),
            encoded("Node"), encoded(None), encoded(status), encoded(None), logged,
        ]
        return "\t".join(values)

    def _mysql(self, _database, sql):
        if "WHERE `id` <=" in sql:
            return self.old
        if "WHERE `id` >" in sql:
            logged = dt.datetime.now(dt.timezone.utc).replace(tzinfo=None).isoformat(timespec="microseconds")
            return "\n".join(
                self.row(
                    str(11 + index), tenant["tenant_id"], tenant["username"],
                    f"198.18.20.{index + 1}", "1", logged,
                )
                for index, tenant in enumerate(self.lineage["tenants"])
            )
        return self.old

    def _redis(self, parts):
        command, *arguments = parts
        if command == "EVAL":
            script, count, *values = arguments
            assert script == runtime.AUTHORIZATION_CACHE_CLEANUP_SCRIPT and count == "33"
            keys, comparisons = values[:33], values[33:]
            assert len(comparisons) == 55
            expired = []
            for index in range(11):
                tenant, user, snapshot = keys[index * 3:index * 3 + 3]
                expected = comparisons[index * 5:index * 5 + 5]
                assert self._redis(["GET", tenant]) == expected[0]
                assert self._redis(["GET", user]) == expected[1]
                assert self._redis(["TYPE", snapshot]) == expected[2]
                if expected[2] == "none":
                    expired.append(snapshot)
                else:
                    assert self._redis(["HKEYS", snapshot]) == [expected[3]]
                    assert self._redis(["HGET", snapshot, expected[3]]) == expected[4]
            self.deleted = True
            return [33 - len(expired), expired]
        key = arguments[0]
        if command == "TYPE":
            if self.deleted:
                return "none"
            if key.endswith(":epoch") or key.endswith(":version"):
                return "string"
            return "hash"
        if command == "GET":
            return "1"
        if command == "HKEYS":
            return ["1:1"]
        if command == "HGET":
            tenant = next(item for item in self.subjects if "{" + item["tenant_id"] + "}" in key)
            payload = {
                "versions": {
                    "tenant_authorization_epoch": 1,
                    "user_authorization_version": 1,
                },
                "tenant_session_version": 1,
                "principal": {
                    "actor": {
                        "user_id": int(tenant["user_id"]),
                        "tenant_id": tenant["tenant_id"],
                        "username": "owner",
                        "dept_id": None,
                        "dept_path": None,
                        "data_scope": "all",
                        "custom_dept_ids": [],
                        "include_self": True,
                        "is_super_admin": False,
                    },
                    "tenant_authorization_epoch": 1,
                    "preferred_locale": None,
                    "roles": [],
                    "role_ids": [],
                    "permissions": [],
                    "tenant_request_limit_per_minute": 600,
                },
            }
            return json.dumps(payload, sort_keys=True)
        raise AssertionError(parts)


class SourceRuntimeTests(unittest.TestCase):
    def setUp(self):
        tools = inventory()
        tools["source"]["snapshot"]["clean"] = True
        self.execution_sha = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=Path(__file__).resolve().parents[2], text=True
        ).strip()
        tools["source"]["snapshot"]["head"] = self.execution_sha
        self.coordinator_source = source_fingerprints.execution_source(tools)
        self.enterContext(patch.object(source_fingerprints, "current_execution_source", return_value=self.coordinator_source))
        repository = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=repository / ".local-tests/python-unit", prefix="source-runtime-"
        )
        self.addCleanup(temporary.cleanup)
        self.backend = repository.resolve()
        self.copy_run = Path(temporary.name) / "copy-run"
        self.results = self.copy_run / "results"
        self.generation = self.copy_run / "g0001"
        self.results.mkdir(parents=True)
        self.generation.mkdir()
        self.start = descriptor_file(self.results / "0001.json", {"status": "running"})
        self.source_registration = descriptor_file(
            self.copy_run / "source-registration.json", {"status": "registered"}
        )
        self.lineage_value = self.lineage()
        self.lineage = descriptor_file(
            self.generation / "dataset-lineage.json", self.lineage_value
        )
        self.before_image, self.after_image = self.images()
        self.subjects = [
            {
                "tenant_id": tenant["tenant_id"],
                "user_id": str(9_007_199_254_740_993 + index),
                "user_authorization_version": 1,
            }
            for index, tenant in enumerate(self.lineage_value["tenants"])
        ]
        self.node_result = {
            "format_version": 1,
            "kind": "restore-source-existing-verification",
            "status": "source_existing_data_verified",
            "scope_id": "perf-seed-unit",
            "origin_tenant_scope_id": "restore-source-unit",
            "lineage_sha256": plan_hash(self.lineage_value),
            "actions": {"business": "read_only", "objects": "read_only", "session": "login_logout"},
            "restore_success": False,
            "subjects": self.subjects,
            "tenants": 11,
            "posts": 33,
            "files": 256,
            "source_generation_sha256": self.start["sha256"],
            "lineage_file_sha256": self.lineage["sha256"],
        }
        self.facts = {
            "start_descriptor": self.start,
            "coordinator_source": self.coordinator_source,
            "receipt": {
                "source_registration": self.source_registration,
                "dataset_lineage": self.lineage,
            },
            "directory": self.copy_run,
            "output": self.generation,
            "request": {"expected_backend_sha": self.execution_sha},
            "source": {
                "request": {"tools": {"node": {"path": sys.executable, "sha256": "unused"}}},
                "seed_target": {
                    "target": {
                        "databases": [
                            {"key": "shared-control", "kind": "combined", "database": "control"}
                        ]
                    }
                },
            },
            "execution": self.backend,
            "selected": {"scope_id": "perf-seed-unit"},
            "running": {"image": self.before_image},
            "lineage": self.lineage_value,
            "environment": {"environment": {}},
        }
        self.resources = FakeResources(self.lineage_value, self.subjects)
        self.process_state = {"alive": True}
        self.process_identity = {
            "pid": 987654,
            "started": "12345",
            "executable": str(Path(sys.executable).resolve()),
        }
        self.controller_identity = {
            "pid": os.getpid(),
            "started": "54321",
            "executable": str(Path(sys.executable).resolve()),
        }

    @staticmethod
    def lineage():
        return {
            "verification": {"scope_id": "perf-seed-unit"},
            "scopes": {"origin_tenant_scope_id": "restore-source-unit"},
            "scale": {"tenants": 11, "post_samples": 33, "business_objects": 256},
            "tenants": [
                {
                    "tenant_id": "system" if index == 0 else f"restore-source-unit-{index:02d}",
                    "username": "owner",
                }
                for index in range(11)
            ],
        }

    @staticmethod
    def database(key, login_rows, login_hash):
        return {
            "target": {
                "database": {
                    "key": key,
                    "placements": [],
                    "tables": [
                        {"table": "sys_login_info", "rows": login_rows, "sha256": login_hash},
                        {"table": "sys_oper_log", "rows": 0, "sha256": "2" * 64},
                        {"table": "sys_outbox_event", "rows": 0, "sha256": "3" * 64},
                        {"table": "sys_post", "rows": 100_000, "sha256": "4" * 64},
                    ],
                },
                "preserved_tables": [],
            }
        }

    def images(self):
        before = {
            "databases": {
                key: self.database(key, 1, "1" * 64)
                for key in ("shared-control", "shared", "dedicated-a", "dedicated-b")
            },
            "schema": [],
            "objects": {},
            "owners": [],
            "redis": {"keys": ["owner"]},
            "storage": {"generation": "same"},
        }
        after = copy.deepcopy(before)
        after["databases"]["shared-control"] = self.database("shared-control", 12, "5" * 64)
        return before, after

    def identity(self, pid):
        if pid == os.getpid():
            return self.controller_identity
        if pid == 987654 and self.process_state["alive"]:
            return self.process_identity
        return None

    def capture(self, _backend, _execution, _selected, _request, _source, _environment, output, _run, **_kwargs):
        output.mkdir()
        image = self.before_image if output.name == "before" else self.after_image
        write_json(output / "image.json", {"image": image})
        return binding(output / "image.json")

    @staticmethod
    def verify_image(_backend, descriptor, *_args, **_kwargs):
        return read_json(Path(descriptor["path"]))

    @contextlib.contextmanager
    def registered(self, _backend, descriptor):
        self.assertEqual(descriptor, self.start)
        yield lambda: self.facts

    def modules(self):
        generation = SimpleNamespace(
            verify_running_source=lambda _backend, descriptor, live: self.facts,
            verify_historical_running_source=lambda _backend, descriptor, live: self.facts,
            registered_running_source=self.registered,
        )
        images = SimpleNamespace(capture_image=self.capture, verify_image=self.verify_image)
        return patch.dict(
            sys.modules,
            {
                "devex_clone_seed_generation": generation,
                "devex_clone_seed_generation_images": images,
            },
        )

    def fake_popen(self, argv, **_kwargs):
        self.process_state["alive"] = True
        return FakeProcess(self.process_state, self.node_result, argv)

    def test_execute_and_static_validator_bind_same_generation_and_exact_effects(self):
        output = self.generation / "verification/source-runtime.json"

        class Tools:
            def __init__(_self, *_args, **_kwargs):
                pass

            def command(_self, name):
                self.assertEqual(name, "node")
                return [sys.executable]

        with self.modules(), patch.object(runtime, "_resources", return_value=self.resources), \
                patch.object(runtime, "ExternalTools", Tools), \
                patch("restore_reference_io.ExternalTools", Tools), \
                patch.object(producer, "process_identity", side_effect=self.identity):
            result = runtime.execute_source_verification(
                self.backend, Path(self.start["path"]), output, popen=self.fake_popen
            )
            self.assertEqual(result["status"], "source_runtime_verified")
            receipt = read_json(output)
            self.assertEqual(receipt["remote_writes"]["confirmed_total_commands"], 23)
            self.assertEqual(receipt["remote_writes"]["authorization_cache"]["deleted_keys"], 33)
            self.assertEqual(receipt["write_effects"]["database_rows"]["sys_login_info"], 11)
            first = runtime.verify_source_runtime(self.backend, binding(output), live=True)
            second = runtime.verify_source_runtime(self.backend, binding(output), live=False)
            self.assertEqual(first, second)
            self.assertEqual(first["after"]["image"], self.after_image)
            self.assertEqual(self.process_state["authorization"]["source_generation_sha256"], self.start["sha256"])
            self.assertEqual(
                producer.require_source_verifier_stopped(self.backend, self.start)["status"],
                "verified_stopped",
            )

            self.process_state["alive"] = True
            with self.assertRaisesRegex(ValueError, "仍在运行"):
                producer.require_source_verifier_stopped(self.backend, self.start)
            self.process_state["alive"] = False

            empty = output.parent / "after/empty"
            empty.mkdir()
            with self.assertRaisesRegex(ValueError, "空目录"):
                runtime.verify_source_runtime(self.backend, binding(output), live=False)
            empty.rmdir()

            receipt = read_json(output)
            original_receipt = copy.deepcopy(receipt)
            cleanup_path = output.parent / "cache-cleanup.json"
            cleanup = read_json(cleanup_path)
            original_cleanup = copy.deepcopy(cleanup)
            cleanup["before"][0]["snapshot"]["field"] = "1:2"
            cleanup_path.unlink()
            write_json(cleanup_path, cleanup)
            receipt["cache_cleanup"] = binding(cleanup_path)
            output.unlink()
            write_json(output, receipt)
            with self.assertRaisesRegex(ValueError, "键或版本"):
                runtime.verify_source_runtime(self.backend, binding(output), live=False)
            cleanup_path.unlink()
            write_json(cleanup_path, original_cleanup)
            output.unlink()
            write_json(output, original_receipt)

            extra = output.parent / "after/unknown.txt"
            extra.write_text("unknown", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "文件集合|未登记文件"):
                runtime.verify_source_runtime(self.backend, binding(output), live=False)

    def test_parameter_and_existing_directory_fail_before_lock_or_spawn(self):
        called = []
        generation = SimpleNamespace(
            verify_running_source=lambda *_args, **_kwargs: called.append("verify") or self.facts,
            registered_running_source=lambda *_args, **_kwargs: (_ for _ in ()).throw(AssertionError()),
        )
        with patch.dict(sys.modules, {"devex_clone_seed_generation": generation}):
            for output in (
                self.generation / "other.json",
                self.backend / "outside/source-runtime.json",
            ):
                with self.assertRaises(ValueError):
                    runtime.execute_source_verification(
                        self.backend, Path(self.start["path"]), output, popen=self.fake_popen
                    )
        self.assertEqual(called, [])

    def test_failed_node_preserves_bounded_stderr_and_registered_identity(self):
        directory = self.copy_run / "failure/verification"
        directory.mkdir(parents=True)
        self.process_state["alive"] = True
        with patch.object(producer, "process_identity", side_effect=self.identity), \
                self.assertRaisesRegex(ValueError, "Node 失败"):
            producer.run_source_producer(
                self.backend,
                self.backend,
                directory,
                "a" * 32,
                Path(sys.executable).resolve(),
                self.start,
                self.lineage,
                {},
                coordinator_source=self.coordinator_source,
                execution_sha=self.execution_sha,
                popen=lambda argv, **_kwargs: FailedProcess(
                    self.process_state, self.node_result, argv
                ),
            )
        self.assertEqual(
            (directory / producer.STDERR).read_bytes(),
            b"preserved source verification failure",
        )
        completion = read_json(directory / producer.COMPLETION)
        self.assertEqual(completion["exit_code"], 7)
        self.assertEqual(completion["process"], self.process_identity)

    def test_invalid_ready_is_preserved_and_rejected_before_authorization(self):
        directory = self.copy_run / "invalid-ready/verification"
        directory.mkdir(parents=True)
        self.process_state = {"alive": True}
        with patch.object(producer, "process_identity", side_effect=self.identity), \
                patch.object(
                    producer,
                    "terminate_owned_process",
                    side_effect=lambda *_a, **_k: self.process_state.update(alive=False),
                ) as stop, self.assertRaisesRegex(ValueError, "ready 未绑定"):
            producer.run_source_producer(
                self.backend,
                self.backend,
                directory,
                "a" * 32,
                Path(sys.executable).resolve(),
                self.start,
                self.lineage,
                {},
                coordinator_source=self.coordinator_source,
                execution_sha=self.execution_sha,
                popen=lambda argv, **_kwargs: InvalidReadyProcess(
                    self.process_state, self.node_result, argv
                ),
            )
        self.assertTrue((directory / producer.READY).is_file())
        self.assertNotIn("authorization", self.process_state)
        self.assertFalse(self.process_state["alive"])
        stop.assert_called_once_with(self.process_identity, crash=True)

    def test_tool_drift_before_spawn_before_authorization_or_after_node_fails_closed(self):
        changed = copy.deepcopy(self.coordinator_source)
        changed["fingerprints"]["test_tools"]["sha256"] = "f" * 64
        for boundary in (0, 2, 3):
            directory = self.copy_run / str(boundary) / "verification"
            directory.mkdir(parents=True)
            self.process_state = {"alive": False}
            with self.subTest(boundary=boundary), \
                    patch.object(source_fingerprints, "current_execution_source", side_effect=[
                        *[self.coordinator_source] * boundary, changed]), \
                    patch.object(producer, "process_identity", side_effect=self.identity), \
                    patch.object(producer, "terminate_owned_process", side_effect=lambda *_a, **_k: self.process_state.update(alive=False)) as stop, \
                    patch.object(self, "fake_popen", wraps=self.fake_popen) as spawn, \
                    self.assertRaisesRegex(ValueError, "test_tools"):
                producer.run_source_producer(self.backend, self.backend, directory, "b" * 32,
                    Path(sys.executable).resolve(), self.start, self.lineage, {},
                    coordinator_source=self.coordinator_source,
                    execution_sha=self.execution_sha, popen=spawn)
            if boundary == 0:
                spawn.assert_not_called()
                self.assertEqual(list(directory.iterdir()), [])
            if boundary < 3:
                self.assertNotIn("authorization", self.process_state)
            else:
                self.assertTrue((directory / producer.COMPLETION).is_file())
            if boundary == 2:
                stop.assert_called_once_with(self.process_identity, crash=True)
            self.assertFalse(self.process_state["alive"])

    def test_failed_and_interrupted_producer_evidence_proves_exit_without_claiming_verification(self):
        from process_environment import configured

        directory = self.generation / "verification"
        directory.mkdir()
        before = descriptor_file(directory / "before/image.json", {"image": self.before_image})
        descriptor_file(directory / "audit/login-before.tsv", {"fixture": "original audit"})
        with patch.object(producer, "process_identity", side_effect=self.identity), \
                self.assertRaisesRegex(ValueError, "Node 失败"):
            producer.run_source_producer(self.backend, self.backend, directory, "c" * 32,
                Path(sys.executable).resolve(), self.start, self.lineage, configured({}),
                coordinator_source=self.coordinator_source, execution_sha=self.execution_sha,
                popen=lambda argv, **_kw: FailedProcess(self.process_state, self.node_result, argv))
        descriptor_file(directory / "failed.json", {"status": "failed", "error": "ValueError"})
        tools = SimpleNamespace(command=lambda _name: [sys.executable])
        with self.modules(), patch.object(producer, "process_identity", side_effect=self.identity), \
                patch("restore_reference_io.ExternalTools", return_value=tools):
            observed = producer.require_source_verifier_stopped(self.backend, self.start)
            self.assertEqual(observed["status"], "stopped")
            self.assertEqual(observed["exit_code"], 7)
            self.assertIn(producer.STDERR, [row["path"] for row in observed["evidence"]["files"]])
            self.assertEqual(binding(Path(before["path"])), before)
            completion_path = directory / producer.COMPLETION
            original_completion = completion_path.read_bytes()
            changed = read_json(completion_path)
            changed["stdout"]["sha256"] = "f" * 64
            completion_path.write_text(json.dumps(changed), encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "完成证据"):
                producer.require_source_verifier_stopped(self.backend, self.start)
            completion_path.write_bytes(original_completion)
            (directory / producer.COMPLETION).unlink()
            interrupted = producer.require_source_verifier_stopped(self.backend, self.start)
            self.assertIsNone(interrupted["exit_code"])
            (directory / "after").mkdir()
            interrupted = producer.require_source_verifier_stopped(self.backend, self.start)
            self.assertIn("after", interrupted["evidence"]["directories"])
            unknown = directory / "after/unknown.txt"
            unknown.write_text("unknown", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "未登记文件"):
                producer.require_source_verifier_stopped(self.backend, self.start)
            unknown.unlink()
            self.process_state["alive"] = True
            with self.assertRaisesRegex(ValueError, "仍在运行"):
                producer.require_source_verifier_stopped(self.backend, self.start)
            self.process_state["alive"] = False
            Path(before["path"]).write_text("{}", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "授权前完整证据"):
                producer.require_source_verifier_stopped(self.backend, self.start)


class SourceRuntimeCliTests(unittest.TestCase):
    def test_verify_uses_only_generation_lineage_and_explicit_output(self):
        backend = Path(__file__).resolve().parents[2]
        start = backend / ".local-tests/run/results/start.json"
        output = backend / ".local-tests/run/g0001/verification/source-runtime.json"
        result = {"output": str(output), "status": "source_runtime_verified"}
        protocol = {
            "backend_dir": str(backend),
            "format_version": 1,
            "kind": source_cli.PROTOCOL_KIND,
            "operation": "verify",
            "source_generation": str(start),
            "output": str(output),
            "write": True,
        }
        request = source_cli.private_protocol_request(
            [], {source_cli.PROTOCOL_KEY: json.dumps(protocol)}
        )
        with patch.object(
            source_cli, "execute_source_verification", return_value=result
        ) as execute, patch("builtins.print") as printed:
            source_cli.main(request)
        execute.assert_called_once_with(backend.resolve(), start, output)
        self.assertEqual(json.loads(printed.call_args.args[0]), result)

        protocol["write"] = False
        with self.assertRaises(source_cli.SourceProtocolError):
            source_cli.private_protocol_request(
                [], {source_cli.PROTOCOL_KEY: json.dumps(protocol)}
            )
        with self.assertRaises(source_cli.SourceProtocolError):
            source_cli.private_protocol_request(["verify"], {})
        protocol["operation"] = "quiesce"
        with self.assertRaises(source_cli.SourceProtocolError):
            source_cli.private_protocol_request(
                [], {source_cli.PROTOCOL_KEY: json.dumps(protocol)}
            )


class SourceProducerStoppedTests(unittest.TestCase):
    def setUp(self):
        repository = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(
            dir=repository / ".local-tests/python-unit", prefix="source-producer-"
        )
        self.addCleanup(temporary.cleanup)
        self.backend = repository.resolve()
        self.output = Path(temporary.name) / "g0001"
        self.output.mkdir()
        self.start = descriptor_file(Path(temporary.name) / "results/0001.json")

    def facts_module(self):
        return SimpleNamespace(
            verify_running_source=lambda _backend, _start, live: {"output": self.output},
            verify_historical_running_source=lambda _backend, _start, live: {"output": self.output},
        )

    def test_no_verification_is_not_started_but_unregistered_or_unknown_is_rejected(self):
        with patch.dict(sys.modules, {"devex_clone_seed_generation": self.facts_module()}):
            self.assertEqual(
                producer.require_source_verifier_stopped(self.backend, self.start)["status"],
                "not_started",
            )
            directory = self.output / "verification"
            directory.mkdir()
            write_json(directory / producer.INTENT, {"incomplete": True})
            with self.assertRaisesRegex(ValueError, "已登记进程身份"):
                producer.require_source_verifier_stopped(self.backend, self.start)


if __name__ == "__main__":
    unittest.main()
