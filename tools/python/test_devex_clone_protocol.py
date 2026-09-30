"""clone 公开参数收归 Rust 后，私有 Python 协议的严格边界。"""
from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
import itertools
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import devex_clone
import devex_clone_protocol as protocol
import devex_clone_run_cli as cli


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

    def payload(self, command, options, write):
        return {
            "format_version": 1,
            "kind": protocol.PROTOCOL_KIND,
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
            ("bridge", {"build": str(self.file), "inventory": str(self.file), "output": str(self.file.with_name("bridge.json"))}, True),
            ("post-copy", {"run_dir": str(self.run), "operation": "register", "request": str(self.file), "producer_binding": None}, True),
            ("seed-runtime", {"run_dir": str(self.run), "operation": "status", "request": None, "producer_binding": None}, False),
            ("storage", {"run_dir": str(self.run), "side": "source", "operation": "restart", "request": None}, True),
            ("cache", {"run_dir": str(self.run), "operation": "status", "request": None}, False),
            ("maintenance", {"operation": "verify", "output": str(self.file)}, False),
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
        for stage in ("export", "target-verify", "copy"):
            modes = ("run",) if stage == "target-verify" else ("run", "reconcile", "resume")
            for mode in modes:
                cases.append(("stage", {"run_dir": str(self.run), "stage": stage, "mode": mode}, True))
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
        bridge = self.payload("bridge", {
            "build": str(outside),
            "inventory": str(self.file),
            "output": str(self.directory / "bridge.json"),
        }, True)
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
        with patch.dict(os.environ, {protocol.PROTOCOL_ENV: source}, clear=True), \
                patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main([]), 2)
        dispatch.assert_not_called()

    def test_main_pops_all_private_protocols_before_dispatch(self):
        source = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False))

        def dispatched(request, backend):
            self.assertEqual(backend, self.backend)
            for name in (
                cli.FRESH_TARGET_PROTOCOL_ENV,
                cli.SEED_SOURCE_PROTOCOL_ENV,
                protocol.PROTOCOL_ENV,
            ):
                self.assertNotIn(name, os.environ)
            return {"status": request.command}

        output = StringIO()
        with patch.dict(os.environ, {protocol.PROTOCOL_ENV: source}), \
                patch.object(cli, "dispatch", side_effect=dispatched) as dispatch, \
                redirect_stdout(output):
            self.assertEqual(devex_clone.main([]), 0)
        dispatch.assert_called_once()
        self.assertEqual(json.loads(output.getvalue()), {"status": "status"})

    def test_main_rejects_argv_and_conflicting_protocols(self):
        source = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False))
        with patch.dict(os.environ, {}, clear=True), redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main(["status", "--run-dir", str(self.run)]), 2)
        with patch.dict(os.environ, {
            protocol.PROTOCOL_ENV: source,
            cli.FRESH_TARGET_PROTOCOL_ENV: "{}",
        }), patch.object(cli, "dispatch") as dispatch, redirect_stderr(StringIO()):
            self.assertEqual(devex_clone.main([]), 2)
        dispatch.assert_not_called()

    def test_direct_process_accepts_only_the_private_protocol(self):
        script = Path(devex_clone.__file__).resolve()
        source = json.dumps(self.payload("status", {"run_dir": str(self.run)}, False))
        environment = {
            key: value for key, value in os.environ.items()
            if key not in {cli.FRESH_TARGET_PROTOCOL_ENV, cli.SEED_SOURCE_PROTOCOL_ENV,
                           protocol.PROTOCOL_ENV}
        }

        def run(arguments=(), clone_protocol=None, extra=None):
            current = dict(environment)
            if clone_protocol is not None:
                current[protocol.PROTOCOL_ENV] = clone_protocol
            current.update(extra or {})
            return subprocess.run(
                [sys.executable, "-B", str(script), *arguments], cwd=self.backend,
                env=current, capture_output=True, text=True, check=False,
            )

        valid = run(clone_protocol=source)
        self.assertEqual(valid.returncode, 1, valid.stderr)
        self.assertIn("开发复制未完成", valid.stderr)
        for result in (
            run(("status", "--run-dir", str(self.run))),
            run(clone_protocol="{}"),
            run(clone_protocol=source, extra={cli.FRESH_TARGET_PROTOCOL_ENV: "{}"}),
        ):
            self.assertEqual(result.returncode, 2, result.stderr)


if __name__ == "__main__":
    unittest.main()
