"""clone 公开参数收归 Rust 后，私有 Python 协议的严格边界。"""
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO, BytesIO
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone
import devex_clone_protocol as protocol
import devex_clone_run_cli as cli
from restore_build import file_digest


class CloneProtocolTests(unittest.TestCase):
    _next = itertools.count(1)

    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.directory = self.backend / ".local-tests" / (
            f"clone-protocol-{os.getpid()}-{next(self._next)}"
        )
        self.directory.mkdir()
        self.addCleanup(shutil.rmtree, self.directory)
        self.run = self.directory / "run"
        self.run.mkdir()
        self.new_run = self.directory / "new-run"
        self.file = self.directory / "input.json"
        self.file.write_text("{}\n", encoding="utf-8")
        executable = Path(sys.executable).resolve()
        digest = file_digest(executable)
        self.native_tool = {
            "format_version": 1,
            "kind": "ryframe-native-rate-limit-tool",
            "path": str(executable),
            "bytes": digest["bytes"],
            "sha256": digest["sha256"],
        }

    def payload(self, command, options, write):
        return {
            "format_version": 1,
            "kind": protocol.PROTOCOL_KIND,
            "native_tool": self.native_tool,
            "request": {
                "backend_dir": str(self.backend),
                "command": command,
                "effect": protocol.effect(command, options),
                "options": options,
                "write": write,
            },
        }

    def decode(self, command, options, write):
        return protocol.decode(json.dumps(self.payload(command, options, write)))

    def test_decodes_every_clone_command_family(self):
        cases = [
            ("plan", {"input": str(self.file), "output": str(self.file.with_name("plan.json"))}, True),
            ("verify", {"plan": str(self.file)}, False),
            ("init", {"manifest": str(self.file), "run_dir": str(self.new_run)}, True),
            ("status", {"run_dir": str(self.run)}, False),
            ("stage", {"run_dir": str(self.run), "stage": "copy", "mode": "resume"}, True),
            ("runtime", {"run_dir": str(self.run), "side": "target", "operation": "status", "roles": ["api"]}, False),
            ("recover", {"run_dir": str(self.run), "owner_binding": str(self.file)}, True),
            ("recover-copy", {"run_dir": str(self.run), "owner_binding": str(self.file)}, True),
            ("post-copy", {"run_dir": str(self.run), "operation": "register", "request": str(self.file), "producer_binding": None}, True),
            ("seed-runtime", {"run_dir": str(self.run), "operation": "status", "request": None, "producer_binding": None}, False),
            ("storage", {"run_dir": str(self.run), "side": "source", "operation": "restart", "request": None}, True),
            ("cache", {"run_dir": str(self.run), "operation": "status", "request": None}, False),
        ]
        for command, options, write in cases:
            with self.subTest(command=command):
                backend, request = self.decode(command, options, write)
                self.assertEqual(backend, self.backend)
                self.assertEqual(request.command, command)
                self.assertEqual(request.write, write)

    def test_effect_classification_distinguishes_observation_from_resource_changes(self):
        cases = [
            ("stage", {"stage": "copy", "mode": "reconcile"}, "evidence-write"),
            ("stage", {"stage": "copy", "mode": "resume"}, "business-write"),
            ("runtime", {"operation": "recover"}, "evidence-write"),
            ("post-copy", {"operation": "reconcile"}, "evidence-write"),
            ("post-copy", {"operation": "schedules"}, "business-write"),
            ("seed-runtime", {"operation": "quotas-reconcile"}, "evidence-write"),
            ("seed-runtime", {"operation": "departments-reconcile"}, "evidence-write"),
            ("seed-runtime", {"operation": "recover"}, "evidence-write"),
            ("cache", {"operation": "reconcile"}, "evidence-write"),
            ("cache", {"operation": "resume"}, "business-write"),
        ]
        for command, options, expected in cases:
            with self.subTest(command=command, options=options):
                self.assertEqual(protocol.effect(command, options), expected)

    def test_decodes_every_nested_operation_and_write_contract(self):
        cases = []
        for mode in ("run", "reconcile", "resume"):
            cases.append(("stage", {"run_dir": str(self.run), "stage": "copy", "mode": mode}, True))
        for operation in ("start", "stop", "status", "recover"):
            cases.append(("runtime", {
                "run_dir": str(self.run), "side": "source", "operation": operation,
                "roles": ["api", "worker"],
            }, operation != "status"))
        for operation in ("register", "amend", "prepare", "schedules", "reconcile", "verify", "recover-session"):
            cases.append(("post-copy", {
                "run_dir": str(self.run), "operation": operation,
                "request": str(self.file) if operation in {"register", "amend"} else None,
                "producer_binding": str(self.file) if operation == "recover-session" else None,
            }, True))
        seed_operations = {
            "register", "quotas-plan", "quotas-apply", "quotas-reconcile",
            "departments-plan", "departments-apply", "departments-reconcile",
            "departments-verify", "identities-apply", "identities-verify", "prepare",
            "start", "close", "arm-input", "stop", "status", "recover", "recover-session",
        }
        for operation in seed_operations:
            cases.append(("seed-runtime", {
                "run_dir": str(self.run), "operation": operation,
                "request": str(self.file) if operation in {"register", "arm-input"} else None,
                "producer_binding": str(self.file) if operation == "recover-session" else None,
            }, operation != "status"))
        for operation in ("restart", "status", "stop", "recover"):
            cases.append(("storage", {
                "run_dir": str(self.run), "side": "target", "operation": operation,
                "request": str(self.file) if operation == "restart" else None,
            }, operation != "status"))
        for operation in ("restart", "status", "stop", "recover", "reconcile", "resume"):
            cases.append(("cache", {
                "run_dir": str(self.run), "operation": operation,
                "request": str(self.file) if operation == "restart" else None,
            }, operation != "status"))
        for command, options, write in cases:
            with self.subTest(command=command, operation=options.get("operation")):
                _, request = self.decode(command, options, write)
                self.assertEqual(request.command, command)
                self.assertEqual(request.write, write)

    def test_rejects_duplicate_unknown_and_non_absolute_inputs(self):
        valid = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False))
        invalid = [
            "{}",
            valid.replace('"format_version": 1,', '"format_version": 1, "format_version": 1,'),
            json.dumps(self.payload("status", {"run_dir": "relative"}, False)),
            json.dumps(self.payload("status", {"run_dir": str(self.run), "extra": 1}, False)),
            json.dumps(self.payload("runtime", {"run_dir": str(self.run), "side": "target", "operation": "status", "roles": ["api", "api"]}, False)),
            json.dumps(self.payload("runtime", {"run_dir": str(self.run), "side": "target", "operation": "status", "roles": [{}]}, False)),
            json.dumps(self.payload("cache", {"run_dir": str(self.run), "operation": "stop", "request": None}, False)),
            json.dumps(self.payload("post-copy", {"run_dir": str(self.run), "operation": "prepare", "request": str(self.file), "producer_binding": None}, True)),
            json.dumps(self.payload("seed-runtime", {"run_dir": str(self.run), "operation": "source-export", "request": None, "producer_binding": None}, True)),
            json.dumps(self.payload("stage", {"run_dir": str(self.run), "stage": "target-verify", "mode": "resume"}, True)),
        ]
        for value in invalid:
            with self.subTest(value=value[:80]), self.assertRaises(protocol.CloneProtocolError):
                protocol.decode(value)

    def test_rejects_hostile_json_and_paths_before_dispatch(self):
        outside = self.backend / "Cargo.toml"
        bridge = self.payload("status", {"run_dir": str(outside)}, False)
        hostile = (
            "[" * 5000 + "0" + "]" * 5000,
            "9" * 5000,
            json.dumps(bridge),
            json.dumps({**self.payload("status", {"run_dir": str(self.run)}, False),
                        "request": {**self.payload("status", {"run_dir": str(self.run)}, False)["request"],
                                    "backend_dir": str(self.backend.parent)}}),
        )
        for source in hostile:
            with self.subTest(source=source[:40]), self.assertRaises(protocol.CloneProtocolError):
                protocol.decode(source)

        source = json.dumps(bridge)
        with patch("sys.stdin", SimpleNamespace(buffer=BytesIO(source.encode("utf-8")))), \
                patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main(["--private-stdin"]), 2)
        dispatch.assert_not_called()

    def test_main_transmits_request_without_child_environment(self):
        source = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False))
        def dispatched(request, backend):
            self.assertEqual(backend, self.backend)
            self.assertNotIn(source, os.environ.values())
            child = subprocess.run([sys.executable, "-c", "import os,json; print(json.dumps(dict(os.environ)))"],
                                   capture_output=True, text=True, check=True)
            self.assertNotIn(source, json.loads(child.stdout).values())
            return {"status": request.command}
        with patch("sys.stdin", SimpleNamespace(buffer=BytesIO(source.encode("utf-8")))), \
                patch("rate_limit_tool.install_binding") as install, \
                patch.object(cli, "dispatch", side_effect=dispatched) as dispatch, redirect_stdout(StringIO()):
            self.assertEqual(devex_clone.main(["--private-stdin"]), 0)
        install.assert_called_once()
        dispatch.assert_called_once()

    def test_main_rejects_invalid_transport_before_dispatch_or_writes(self):
        source = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False)).encode()
        before = {p: p.read_bytes() for p in self.directory.rglob("*") if p.is_file()}
        for raw in [b"", b"x" * 65537, b"\xff", b"{}", source + b"{}",
                    source.replace(b'"format_version": 1', b'"format_version": true'),
                    source.replace(b'"format_version": 1', b'"format_version": 1,"format_version":1'),
                    source.replace(protocol.PROTOCOL_KIND.encode(), b"unknown"),
                    source.replace(b'"status"', b'"status","command":"status"', 1)]:
            with self.subTest(raw=raw[:50]), patch("sys.stdin", SimpleNamespace(buffer=BytesIO(raw))), \
                    patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
                self.assertEqual(devex_clone.main(["--private-stdin"]), 2)
            dispatch.assert_not_called()
        for arguments in [[], ["status"], ["--private-stdin", "status"], ["--private-stdin", "--private-stdin"]]:
            with patch("sys.stdin") as stream, patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
                self.assertEqual(devex_clone.main(arguments), 2)
            stream.buffer.read.assert_not_called()
            dispatch.assert_not_called()
        for legacy in ["RYFRAME_XTASK_RECOVERY_FRESH_TARGET", "RYFRAME_XTASK_RECOVERY_SEED_SOURCE",
                       "RYFRAME_XTASK_RECOVERY_CLONE", "RYFRAME_XTASK_RECOVERY_CLONE_STAGE"]:
            with patch.dict(os.environ, {legacy: source.decode()}), \
                    patch("sys.stdin", SimpleNamespace(buffer=BytesIO(b""))), \
                    patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
                self.assertEqual(devex_clone.main(["--private-stdin"]), 2)
            dispatch.assert_not_called()
        self.assertEqual(before, {p: p.read_bytes() for p in self.directory.rglob("*") if p.is_file()})

    def test_stage_kind_uses_existing_decoder_and_dispatch_once(self):
        value = {"format_version": 1, "kind": cli.STAGE_PROTOCOL_KIND,
                 "native_tool": self.native_tool,
                 "request": {"backend_dir": str(self.backend), "run_dir": str(self.run),
                             "stage": "copy", "mode": "run", "operation": "run",
                             "attempt": 7, "write": True}}
        source = json.dumps(value).encode()
        with patch("sys.stdin", SimpleNamespace(buffer=BytesIO(source))), \
                patch("rate_limit_tool.install_binding") as install, \
                patch.object(cli, "dispatch_stage_operation", return_value={"status": "fixture"}) as dispatch, \
                patch.object(cli, "dispatch") as ordinary, redirect_stdout(StringIO()):
            self.assertEqual(devex_clone.main(["--private-stdin"]), 0)
        dispatch.assert_called_once()
        install.assert_called_once_with(json.dumps(self.native_tool, ensure_ascii=False))
        self.assertEqual(dispatch.call_args.args[0].native_tool, self.native_tool)
        self.assertEqual(dispatch.call_args.args[0].attempt, 7)
        self.assertTrue(dispatch.call_args.args[0].write)
        ordinary.assert_not_called()
        retired = {**value, "request": {**value["request"], "stage": "target-verify",
                   "operation": "target-authority", "attempt": None, "write": False}}
        for raw in [json.dumps(retired).encode(),
                    source.replace(b'"stage": "copy"', b'"stage": "export"'),
                    source.replace(b'"stage": "copy"', b'"stage": "target-verify"'),
                    source.replace(b'"attempt": 7', b'"attempt": 0'),
                    source.replace(b'"write": true', b'"write": false'),
                    source + b" " * (32769 - len(source))]:
            with patch("sys.stdin", SimpleNamespace(buffer=BytesIO(raw))), \
                    patch.object(cli, "dispatch_stage_operation") as dispatch, redirect_stderr(StringIO()):
                self.assertEqual(devex_clone.main(["--private-stdin"]), 2)
            dispatch.assert_not_called()

    def test_all_private_kinds_require_valid_native_binding_before_dispatch(self):
        requests = [
            self.payload("status", {"run_dir": str(self.run)}, False),
            {"format_version": 1, "kind": cli.FRESH_TARGET_PROTOCOL_KIND,
             "request": {"backend_dir": str(self.backend), "workspace": str(self.new_run),
                         "operation": "status", "request": None, "environment": None,
                         "storage_run": None, "observation_dir": None, "write": False}},
            {"format_version": 1, "kind": cli.SEED_SOURCE_PROTOCOL_KIND,
             "request": {"backend_dir": str(self.backend), "run_dir": str(self.run),
                         "operation": "source-generation-status", "request": None, "write": False}},
            {"format_version": 1, "kind": cli.STAGE_PROTOCOL_KIND,
             "request": {"backend_dir": str(self.backend), "run_dir": str(self.run),
                         "stage": "copy", "mode": "run", "operation": "preflight",
                         "attempt": None, "write": False}},
        ]
        before = {p: p.read_bytes() for p in self.directory.rglob("*") if p.is_file()}
        for request in requests:
            for binding in [None, {}, {**self.native_tool, "bytes": True},
                            {**self.native_tool, "path": "relative"},
                            {**self.native_tool, "sha256": "invalid"}]:
                value = {**request, "native_tool": binding}
                if binding is None:
                    del value["native_tool"]
                raw = json.dumps(value).encode("utf-8")
                with self.subTest(kind=value["kind"], binding=binding), \
                        patch("sys.stdin", SimpleNamespace(buffer=BytesIO(raw))), \
                        patch("rate_limit_tool.install_binding") as install, \
                        patch.object(cli, "dispatch") as ordinary, \
                        patch.object(cli, "dispatch_stage_operation") as stage, \
                        redirect_stderr(StringIO()):
                    self.assertEqual(devex_clone.main(["--private-stdin"]), 2)
                install.assert_not_called()
                ordinary.assert_not_called()
                stage.assert_not_called()
        self.assertEqual(before, {p: p.read_bytes() for p in self.directory.rglob("*") if p.is_file()})

    def test_cancelled_or_exited_dispatch_is_not_retried(self):
        source = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False)).encode()
        for error in [KeyboardInterrupt(), SystemExit(7)]:
            with patch("sys.stdin", SimpleNamespace(buffer=BytesIO(source))), \
                    patch("rate_limit_tool.install_binding"), \
                    patch.object(cli, "dispatch", side_effect=error) as dispatch, \
                    self.assertRaises(type(error)):
                devex_clone.main(["--private-stdin"])
            dispatch.assert_called_once()

    def test_direct_process_accepts_only_the_private_protocol(self):
        script = Path(devex_clone.__file__).resolve()
        source = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False))
        def run(arguments=("--private-stdin",), clone_protocol=None):
            return subprocess.run(
                [sys.executable, "-X", "utf8", "-B", str(script), *arguments], cwd=self.backend,
                input=clone_protocol or "", capture_output=True, text=True, encoding="utf-8", check=False,
            )

        valid = run(clone_protocol=source)
        self.assertEqual(valid.returncode, 1, valid.stderr)
        self.assertIn("开发复制未完成", valid.stderr)
        for result in (
            run(("status", "--run-dir", str(self.run))),
            run(clone_protocol="{}"),
            run(clone_protocol=source + "{}"),
        ):
            self.assertEqual(result.returncode, 2, result.stderr)


if __name__ == "__main__":
    unittest.main()
