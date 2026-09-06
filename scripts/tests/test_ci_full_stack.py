from __future__ import annotations

import importlib.util
import json
import sys
import os
import shutil
import subprocess
import unittest
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "ci_full_stack.py"
sys.path.insert(0, str(SCRIPT.parent))
SPEC = importlib.util.spec_from_file_location("ci_full_stack", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
from ci_full_stack_resources import BINARIES
TEMP_ROOT = SCRIPT.parents[1] / ".local-tests/python-unit"
TEMP_ROOT.mkdir(parents=True, exist_ok=True)


@contextmanager
def test_directory() -> Iterator[Path]:
    path = TEMP_ROOT / f"full-stack-{uuid.uuid4().hex}"
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path)


class FakeProcess:
    def __init__(self, results: list[int | None]) -> None:
        self.results = iter(results)

    def poll(self) -> int | None:
        return next(self.results)


class FullStackCiTests(unittest.TestCase):
    def environment(self, root: Path) -> dict[str, str]:
        return {
            "RUNNER_TEMP": str(root / "runner"),
            "APP_OBJECT_STORAGE_LOCAL_BASE_DIR": str(root / "storage"),
            "RYFRAME_CI_REDIS_CONTAINER_ID": "b" * 64,
            "APP_REDIS_DATABASE": "14",
            "APP_RESET_REDIS_OUTSIDE_SENTINEL_KEY": "outside-sentinel",
            "CARGO_TARGET_DIR": str(root / "target"),
            "APP_ENV": "test",
            "APP_SCOPE_ID": "ci-12-3",
            "APP_APP_HOST": "127.0.0.1",
            "APP_APP_PORT": "8080",
            "APP_JOBS_HEALTH_HOST": "127.0.0.1",
            "APP_JOBS_HEALTH_PORT": "9091",
        }

    def test_prepare_uses_reset_plan_and_explicit_migration_commands(self) -> None:
        with test_directory() as root:
            commands: list[list[str]] = []

            def run(arguments: list[str], **_: object):
                commands.append(arguments)
                stdout = f"plan_hash={'a' * 64}\n" if arguments[-1] == "plan" else ""
                if arguments[0] == "cargo":
                    name = arguments[arguments.index("--bin") + 1]
                    stdout = json.dumps({"reason": "compiler-artifact", "target": {"name": name},
                                         "executable": str(root / "artifacts" / name)})
                return subprocess.CompletedProcess(arguments, 0, stdout, "")

            with mock.patch.dict(os.environ, self.environment(root), clear=True), mock.patch.object(
                MODULE, "_run", side_effect=run
            ), mock.patch.object(MODULE, "plan_databases", return_value=mock.sentinel.plan), \
                    mock.patch.object(MODULE, "prepare_databases") as databases:
                MODULE.prepare(root)
                databases.assert_called_once_with(
                    MODULE._run,
                    MODULE._required_environment,
                    root,
                    root / "runner/ryframe-full-stack",
                    plan=mock.sentinel.plan,
                )

            for bucket in MODULE.BUCKETS:
                self.assertTrue((root / "storage" / bucket).is_dir())
            for index, (feature, name) in enumerate(BINARIES):
                self.assertEqual(commands[index], ["cargo", "build", "--locked", "-p", "ryframe",
                                 "--no-default-features", "--features", feature, "--bin", name,
                                 "--message-format=json"])
            self.assertEqual(
                commands[len(BINARIES)],
                [
                    "docker",
                    "exec",
                    "b" * 64,
                    "redis-cli",
                    "-n",
                    "14",
                    "SET",
                    "outside-sentinel",
                    "ci-sentinel",
                    "NX",
                ],
            )
            self.assertEqual(
                commands[-5][-5:],
                [
                    "execute",
                    "--plan-hash",
                    "a" * 64,
                    "--confirm-reset",
                    "RESET-RYFRAME-test-ci-12-3",
                ],
            )
            migrate = str(root / "artifacts/ryframe-migrate")
            self.assertEqual(commands[-4], [migrate, "control", "up"])
            self.assertEqual(
                commands[-3], [migrate, "tenant-data", "up", "--all"]
            )
            self.assertEqual(commands[-2], [migrate, "control", "verify"])
            self.assertEqual(commands[-1], [migrate, "tenant-data", "verify", "--all"])

    def test_wait_for_api_stops_on_success_or_process_exit(self) -> None:
        ready = mock.Mock(side_effect=[False, True])
        sleep = mock.Mock()
        MODULE.wait_for_api(FakeProcess([None, None]), ready=ready, sleep=sleep, attempts=2)
        sleep.assert_called_once_with(2)

        with self.assertRaisesRegex(MODULE.FullStackError, "退出码 7"):
            MODULE.wait_for_api(
                FakeProcess([7]),
                ready=lambda: False,
                sleep=lambda _: None,
                attempts=1,
            )

    def test_api_and_external_worker_have_distinct_id_generators(self) -> None:
        with test_directory() as root:
            environment = {**self.environment(root), "APP_JOBS_MODE": "external"}
            processes = [mock.Mock(pid=101)]
            with mock.patch.dict(os.environ, environment, clear=True), \
                    mock.patch.object(MODULE, "read_binaries", return_value={"ryframe": "api", "ryframe-worker": "worker"}), \
                    mock.patch.object(MODULE, "register_runtime", return_value={
                        "scope_id": "ci-12-3",
                        "worker_ready_url": "http://127.0.0.1:9091/readyz",
                    }), \
                    mock.patch.object(MODULE, "record_process") as record, \
                    mock.patch.object(MODULE, "ensure_port_free"), \
                    mock.patch.object(MODULE, "wait_for_port_free"), \
                    mock.patch.object(MODULE, "verify_listener"), \
                    mock.patch.object(MODULE, "control_worker") as worker, \
                    mock.patch.object(MODULE.subprocess, "Popen", side_effect=processes) as popen, \
                    mock.patch.object(MODULE, "wait_for_api") as ready:
                MODULE.start(root)
                self.assertEqual([call.kwargs["env"]["SNOWFLAKE_WORKER_ID"]
                                  for call in popen.call_args_list], ["1"])
                self.assertEqual(ready.call_count, 1)
                record.assert_called_once_with(root / "runner/ryframe-full-stack", "api", 101, "api", "ci-12-3")
                worker.assert_called_once_with("start", root, root / "runner/ryframe-full-stack", timeout=180)

    def test_worker_start_failure_reaps_the_registered_api(self) -> None:
        with test_directory() as root:
            environment = {**self.environment(root), "APP_JOBS_MODE": "external"}
            process = mock.Mock(pid=101)
            identity = {"pid": 101, "started": "1", "executable": "api"}
            with mock.patch.dict(os.environ, environment, clear=True), \
                    mock.patch.object(MODULE, "read_binaries", return_value={"ryframe": "api"}), \
                    mock.patch.object(MODULE, "register_runtime", return_value={
                        "scope_id": "ci-12-3",
                        "worker_ready_url": "http://127.0.0.1:9091/readyz",
                    }), \
                    mock.patch.object(MODULE, "record_process", return_value={"identity": identity}), \
                    mock.patch.object(MODULE, "read_process", return_value=identity), \
                    mock.patch.object(MODULE, "ensure_port_free"), \
                    mock.patch.object(MODULE, "wait_for_port_free") as port_free, \
                    mock.patch.object(MODULE, "verify_listener"), \
                    mock.patch.object(MODULE, "terminate_owned_process") as terminate, \
                    mock.patch.object(MODULE, "control_worker", side_effect=ValueError("worker failed")), \
                    mock.patch.object(MODULE.subprocess, "Popen", return_value=process), \
                    mock.patch.object(MODULE, "wait_for_api"):
                with self.assertRaisesRegex(MODULE.FullStackError, "API 已安全回收"):
                    MODULE.start(root)
            terminate.assert_called_once_with(identity, crash=False)
            process.wait.assert_called_once_with(timeout=5)
            port_free.assert_called_once_with("http://127.0.0.1:8080/readyz")

    def test_invalid_api_endpoint_is_rejected_before_spawn(self) -> None:
        with test_directory() as root:
            environment = {**self.environment(root), "APP_JOBS_MODE": "external",
                           "APP_APP_HOST": "0.0.0.0"}
            with mock.patch.dict(os.environ, environment, clear=True), \
                    mock.patch.object(MODULE.subprocess, "Popen") as popen:
                with self.assertRaisesRegex(MODULE.FullStackError, "本机 IPv4"):
                    MODULE.start(root)
            popen.assert_not_called()

    def test_api_and_worker_cannot_share_a_readiness_port(self) -> None:
        with test_directory() as root:
            environment = {**self.environment(root), "APP_JOBS_MODE": "external"}
            receipt = {"scope_id": "ci-12-3", "worker_ready_url": "http://127.0.0.1:8080/readyz"}
            with mock.patch.dict(os.environ, environment, clear=True), \
                    mock.patch.object(MODULE, "register_runtime", return_value=receipt), \
                    mock.patch.object(MODULE.subprocess, "Popen") as popen:
                with self.assertRaisesRegex(MODULE.FullStackError, "不同"):
                    MODULE.start(root)
            popen.assert_not_called()

    def test_prepare_preflight_rejects_unsafe_runtime_inputs_before_build(self) -> None:
        cases = (
            {"CARGO_TARGET_DIR": "relative-target"},
            {"RYFRAME_CI_REDIS_CONTAINER_ID": "--all"},
            {"APP_REDIS_DATABASE": "16"},
            {"APP_OBJECT_STORAGE_BACKEND": "unknown"},
            {"APP_OBJECT_STORAGE_BACKEND": "s3", "RYFRAME_CI_S3_CONTAINER_ID": "service-name"},
        )
        for changed in cases:
            with self.subTest(changed=changed), test_directory() as root:
                environment = {**self.environment(root), **changed}
                with mock.patch.dict(os.environ, environment, clear=True), \
                        mock.patch.object(MODULE, "plan_databases", return_value=mock.sentinel.plan), \
                        mock.patch.object(MODULE, "build_binaries") as build:
                    with self.assertRaises(MODULE.FullStackError):
                        MODULE.prepare(root)
                build.assert_not_called()
                self.assertFalse((root / "runner/ryframe-full-stack").exists())

    def test_collect_targets_only_recorded_process_and_declared_containers(self) -> None:
        with test_directory() as root:
            environment = self.environment(root)
            environment["RYFRAME_CI_MYSQL_CONTAINER_ID"] = "a" * 64
            output = root / "runner/ryframe-full-stack"
            output.mkdir(parents=True)
            identity = {"pid": 4242, "started": "100", "executable": "/owned/api"}
            (output / "api.json").write_text(json.dumps({"format_version": 1,
                "role": "api", "scope_id": "ci-12-3", "identity": identity}), encoding="utf-8")
            with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(
                MODULE, "terminate_owned_process"
            ) as terminate, mock.patch.object(MODULE, "wait_for_port_free"), \
                    mock.patch.object(MODULE, "_best_effort") as command:
                MODULE.collect()

            terminate.assert_called_once_with(identity)
            self.assertEqual(
                [call.args[0] for call in command.call_args_list],
                [
                    ["docker", "logs", "a" * 64],
                    ["docker", "logs", "b" * 64],
                    ["sccache", "--show-stats"],
                    ["sccache", "--stop-server"],
                ],
            )

    def test_cleanup_rejects_wrong_scope_but_still_collects_logs(self) -> None:
        with test_directory() as root:
            output = root / "runner/ryframe-full-stack"
            output.mkdir(parents=True)
            (output / "worker.json").write_text(json.dumps({"format_version": 1,
                "role": "worker", "scope_id": "other", "identity": {}}), encoding="utf-8")
            with mock.patch.dict(os.environ, self.environment(root), clear=True), \
                    mock.patch.object(MODULE, "terminate_owned_process") as terminate, \
                    mock.patch.object(MODULE, "wait_for_port_free"), \
                    mock.patch.object(MODULE, "_best_effort") as logs:
                with self.assertRaisesRegex(MODULE.FullStackError, "未安全回收"):
                    MODULE.collect()
            terminate.assert_not_called()
            self.assertGreater(logs.call_count, 0)
            self.assertIn("scope", (output / "cleanup-errors.log").read_text(encoding="utf-8"))


if __name__ == "__main__":
    unittest.main()
