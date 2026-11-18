"""原生复制生产者的命令边界、stdin 授权门和凭据泄漏回归。"""
import json
import os
from pathlib import Path
import subprocess
import sys
import time
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone_post_process as process
from restore_build import file_digest
from workspace_directory import WorkspaceDirectory


class NativeProducerTests(unittest.TestCase):
    def setUp(self):
        self.root = next(
            path for path in Path(__file__).resolve().parents
            if (path / "Cargo.toml").is_file()
        )
        from rate_limit_tool import tool_binding, verified_tool
        selected = tool_binding()
        self.assertIsNotNone(selected, "必须显式绑定本次编译的原生工具")
        with verified_tool(selected) as tool:
            self.tool = Path(tool)
            self.binding = {key: selected[key] for key in ("path", "sha256")}

    def test_missing_coordinator_binding_rejects_without_spawning_a_process(self):
        with (
            patch("rate_limit_tool.tool_binding", return_value=None),
            patch.object(process, "_TEST_TOOL", self.binding, create=True),
            patch.object(process.subprocess, "run") as spawn,
        ):
            with self.assertRaisesRegex(ValueError, "缺少当前 xtask 工具绑定"):
                process._native({}, "command")
            spawn.assert_not_called()

    def test_existing_native_command_uses_explicit_public_configuration_binding(self):
        with WorkspaceDirectory(
            dir=self.root / ".local-tests/tmp", prefix="post-existing-command-"
        ) as temporary:
            directory = Path(temporary).resolve()
            output = directory / "post-copy/attempt-0001"
            output.mkdir(parents=True)
            configuration = output / "existing-configuration.json"
            configuration.write_text("{}\n", encoding="utf-8")
            request = {
                "reference_plan": {"path": str(directory / "plan.json")},
                "dataset": {"path": str(directory / "dataset.json")},
            }
            command = process.producer_command(
                self.root,
                request,
                directory,
                1,
                "existing",
                output,
                tool_override=self.binding,
                configuration=configuration,
            )
            self.assertEqual(command[1], "__recovery-existing-clone")
            self.assertEqual(
                command[command.index("--configuration") + 1], str(configuration)
            )
            serialized = json.dumps(command, ensure_ascii=False)
            for secret in ("admin_password", "access_key", "secret_key", "session_token"):
                self.assertNotIn(secret, serialized)

    def request(self, plan):
        return {
            "identity_tool": self.binding,
            "identity_plan": {
                "path": str(plan),
                "sha256": file_digest(plan)["sha256"],
            },
            "identity_state": str(plan.parent / "identities"),
        }

    def test_native_commands_share_one_xtask_and_expected_private_operations(self):
        with WorkspaceDirectory(
            dir=self.root / ".local-tests/tmp", prefix="native-producer-command-"
        ) as temporary:
            directory = Path(temporary).resolve()
            plan = directory / "plan.json"
            plan.write_text("{}\n", encoding="utf-8")
            request = self.request(plan)
            output = directory / "seed-runtime/attempt-0001"
            output.mkdir(parents=True)
            commands = {
                kind: process.producer_command(
                    self.root, request, directory, 1, kind, output
                )
                for kind in (
                    "identity-apply",
                    "quota-plan",
                    "quota-apply",
                    "quota-reconcile",
                    "department-plan",
                    "department-apply",
                    "department-reconcile",
                    "department-verify",
                )
            }
            self.assertEqual(
                {command[0] for command in commands.values()}, {str(self.tool)}
            )
            self.assertEqual(commands["identity-apply"][1], "__identity-producer")
            for kind, command in commands.items():
                if not kind.startswith("identity-"):
                    self.assertEqual(command[1], "__identity-bridge")

    def test_native_bridge_waits_for_stdin_and_rejects_wrong_grant_without_leaks(self):
        sentinel = "Native-Bridge-Secret-Sentinel!123"
        with WorkspaceDirectory(
            dir=self.root / ".local-tests/tmp", prefix="native-producer-gate-"
        ) as temporary:
            directory = Path(temporary).resolve()
            plan = directory / "plan.json"
            plan.write_text("{}\n", encoding="utf-8")
            request = self.request(plan)
            output = directory / "seed-runtime/attempt-0001"
            output.mkdir(parents=True)
            for kind in ("quota-plan", "department-plan"):
                command = process.producer_command(
                    self.root, request, directory, 1, kind, output
                )
                child = subprocess.Popen(
                    command,
                    cwd=self.root,
                    env={"APP_ENV": "test", "PATH": os.environ.get("PATH", "")},
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    creationflags=(
                        subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
                    ),
                )
                try:
                    time.sleep(0.1)
                    self.assertIsNone(child.poll(), kind)
                    wrong = {
                        "id": 1,
                        "operation": "initialize",
                        "run_dir": str(directory / "other"),
                        "attempt": 1,
                        "password": sentinel,
                    }
                    stdout, stderr = child.communicate(
                        (json.dumps(wrong) + "\n").encode(), timeout=10
                    )
                    self.assertEqual(child.returncode, 1, kind)
                    self.assertNotIn(sentinel.encode(), stdout)
                    self.assertNotIn(sentinel.encode(), stderr)
                    response = json.loads(stdout)
                    self.assertEqual(response["id"], 1)
                    self.assertIs(response["ok"], False)
                finally:
                    if child.poll() is None:
                        child.kill()
                    child.wait(timeout=10)


if __name__ == "__main__":
    unittest.main()
