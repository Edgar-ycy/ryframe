from __future__ import annotations

import importlib.util
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
SPEC = importlib.util.spec_from_file_location("ci_full_stack", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
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
            "RYFRAME_CI_REDIS_CONTAINER_ID": "redis-container",
            "APP_REDIS_DATABASE": "14",
            "APP_RESET_REDIS_OUTSIDE_SENTINEL_KEY": "outside-sentinel",
            "CARGO_TARGET_DIR": str(root / "target"),
            "APP_ENV": "test",
            "APP_SCOPE_ID": "ci-12-3",
        }

    def test_prepare_uses_fixed_buckets_redis_db_and_reset_plan(self) -> None:
        with test_directory() as root:
            commands: list[list[str]] = []

            def run(arguments: list[str], **_: object):
                commands.append(arguments)
                stdout = f"plan_hash={'a' * 64}\n" if arguments[-1] == "plan" else ""
                return subprocess.CompletedProcess(arguments, 0, stdout, "")

            with mock.patch.dict(os.environ, self.environment(root), clear=True), mock.patch.object(
                MODULE, "_run", side_effect=run
            ):
                MODULE.prepare(root)

            for bucket in MODULE.BUCKETS:
                self.assertTrue((root / "storage" / bucket).is_dir())
            self.assertEqual(
                commands[0],
                [
                    "docker",
                    "exec",
                    "redis-container",
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
                commands[1],
                [
                    "cargo",
                    "build",
                    "--locked",
                    "-p",
                    "ryframe",
                    "--no-default-features",
                    "--features",
                    "bin-reset",
                    "--bin",
                    "ryframe-reset",
                ],
            )
            self.assertEqual(
                commands[2],
                [
                    "cargo",
                    "build",
                    "--locked",
                    "-p",
                    "ryframe",
                    "--no-default-features",
                    "--features",
                    "bin-api",
                    "--bin",
                    "ryframe",
                ],
            )
            self.assertEqual(
                commands[-1][-5:],
                [
                    "execute",
                    "--plan-hash",
                    "a" * 64,
                    "--confirm-reset",
                    "RESET-RYFRAME-test-ci-12-3",
                ],
            )

    def test_wait_for_api_stops_on_success_or_process_exit(self) -> None:
        ready = mock.Mock(side_effect=[False, True])
        sleep = mock.Mock()
        MODULE.wait_for_api(FakeProcess([None]), ready=ready, sleep=sleep, attempts=2)
        sleep.assert_called_once_with(2)

        with self.assertRaisesRegex(MODULE.FullStackError, "退出码 7"):
            MODULE.wait_for_api(
                FakeProcess([7]),
                ready=lambda: False,
                sleep=lambda _: None,
                attempts=1,
            )

    def test_collect_targets_only_recorded_process_and_declared_containers(self) -> None:
        with test_directory() as root:
            environment = self.environment(root)
            environment["RYFRAME_CI_MYSQL_CONTAINER_ID"] = "mysql-container"
            output = root / "runner/ryframe-full-stack"
            output.mkdir(parents=True)
            (output / "api.pid").write_text("4242\n", encoding="ascii")
            with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(
                MODULE, "_terminate_process_group"
            ) as terminate, mock.patch.object(MODULE, "_best_effort") as command:
                MODULE.collect()

            terminate.assert_called_once_with(4242)
            self.assertEqual(
                [call.args[0] for call in command.call_args_list],
                [
                    ["docker", "logs", "mysql-container"],
                    ["docker", "logs", "redis-container"],
                    ["sccache", "--show-stats"],
                    ["sccache", "--stop-server"],
                ],
            )


if __name__ == "__main__":
    unittest.main()
