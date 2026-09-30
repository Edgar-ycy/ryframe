import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_runtime
import restore_source

ROOT = Path(__file__).resolve().parents[2]
SHA_A = "a" * 40
SHA_B = "b" * 40


def runtime_protocol(operation: str) -> dict:
    root = str(ROOT)
    common = {
        "backend_dir": root,
        "format_version": 1,
        "kind": restore_runtime.PROTOCOL_KIND,
        "operation": operation,
        "write": operation in restore_runtime.WRITING_OPERATIONS,
    }
    fields = {
        "build": {
            "source_backend": root,
            "source_frontend": root,
            "frontend_dir": root,
            "expected_head": SHA_A,
            "expected_frontend_head": SHA_B,
            "output": str(ROOT / ".local-tests/build.json"),
        },
        "register": {
            "plan": str(ROOT / ".local-tests/reference.json"),
            "target_plan": str(ROOT / ".local-tests/target.json"),
            "output": str(ROOT / ".local-tests/registration.json"),
        },
        "start": {
            "runtime_registration": str(ROOT / ".local-tests/registration.json"),
            "target_plan": str(ROOT / ".local-tests/target.json"),
            "source_backend": root,
            "source_frontend": root,
            "build_receipt": str(ROOT / ".local-tests/build.json"),
            "bindings": str(ROOT / ".local-tests/bindings.json"),
            "timeout": 12.5,
        },
        "status": {
            "runtime_registration": str(ROOT / ".local-tests/registration.json"),
            "target_plan": str(ROOT / ".local-tests/target.json"),
        },
        "stop": {
            "runtime_registration": str(ROOT / ".local-tests/registration.json"),
            "target_plan": str(ROOT / ".local-tests/target.json"),
            "generation": 1,
        },
        "recover": {
            "runtime_registration": str(ROOT / ".local-tests/registration.json"),
            "target_plan": str(ROOT / ".local-tests/target.json"),
            "generation": 1,
            "owner": str(ROOT / ".local-tests/owner.json"),
        },
        "bind": {
            "source_backend": root,
            "source_frontend": root,
            "build_receipt": str(ROOT / ".local-tests/build.json"),
            "launch_receipt": str(ROOT / ".local-tests/runtime-launch.json"),
            "bindings": str(ROOT / ".local-tests/bindings.json"),
            "output": str(ROOT / ".local-tests/runtime.json"),
        },
        "verify": {
            "source_backend": root,
            "source_frontend": root,
            "bindings": str(ROOT / ".local-tests/bindings.json"),
            "receipt": str(ROOT / ".local-tests/runtime.json"),
        },
    }
    return {**common, **fields[operation]}


def source_protocol(operation: str) -> dict:
    root = str(ROOT)
    common = {
        "backend_dir": root,
        "format_version": 1,
        "kind": restore_source.PROTOCOL_KIND,
        "operation": operation,
        "write": operation in restore_source.WRITING_OPERATIONS,
    }
    fields = {
        "verify": {
            "source_generation": str(ROOT / ".local-tests/start.json"),
            "output": str(ROOT / ".local-tests/source-runtime.json"),
        },
        "verify-recover": {
            "source_generation": str(ROOT / ".local-tests/start.json"),
            "output": str(ROOT / ".local-tests/source-runtime.json"),
        },
        "comparison-capture": {
            "b0_backend": root,
            "b0_adapter_backend": root,
            "b0_frontend": root,
            "b0_backend_build": str(ROOT / ".local-tests/b0-build.json"),
            "b0_frontend_build": str(ROOT / "dist/.vite/restore-build.json"),
            "b1_backend": root,
            "b1_frontend": root,
            "b1_backend_build": str(ROOT / ".local-tests/b1-build.json"),
            "b1_frontend_build": str(ROOT / "dist/.vite/restore-build.json"),
            "source_export_result": str(ROOT / ".local-tests/source-export.json"),
            "output": str(ROOT / ".local-tests/comparison.json"),
        },
        "comparison-verify": {"receipt": str(ROOT / ".local-tests/comparison.json")},
    }
    return {**common, **fields[operation]}


def private_environment() -> dict[str, str]:
    values = {
        name: value for name, value in os.environ.items()
        if not name.startswith(restore_runtime.PROTOCOL_PREFIX)
        and not name.startswith(restore_source.PROTOCOL_PREFIX)
    }
    values["PYTHONIOENCODING"] = "utf-8"
    return values


class RestorePrivateProtocolTests(unittest.TestCase):
    def test_runtime_decodes_every_operation_without_argv(self):
        for operation in sorted(restore_runtime.OPERATIONS):
            with self.subTest(operation=operation):
                request = restore_runtime.private_protocol_request(
                    [], {restore_runtime.PROTOCOL_KEY: json.dumps(runtime_protocol(operation))}
                )
                self.assertEqual(request.operation, operation)
                self.assertEqual(
                    request.namespace().write,
                    operation in restore_runtime.WRITING_OPERATIONS,
                )

    def test_source_decodes_every_operation_without_argv(self):
        for operation in sorted(restore_source.OPERATIONS):
            with self.subTest(operation=operation):
                request = restore_source.private_protocol_request(
                    [], {restore_source.PROTOCOL_KEY: json.dumps(source_protocol(operation))}
                )
                self.assertEqual(request.operation, operation)
                self.assertEqual(
                    request.namespace().write,
                    operation in restore_source.WRITING_OPERATIONS,
                )

    def test_protocols_reject_duplicates_unknown_fields_and_wrong_write(self):
        for module, payload in (
            (restore_runtime, runtime_protocol("status")),
            (restore_source, source_protocol("comparison-verify")),
        ):
            with self.subTest(module=module.__name__, case="unknown"):
                invalid = {**payload, "unknown": "value"}
                with self.assertRaises(ValueError):
                    module.private_protocol_request([], {module.PROTOCOL_KEY: json.dumps(invalid)})
            with self.subTest(module=module.__name__, case="write"):
                invalid = {**payload, "write": True}
                with self.assertRaises(ValueError):
                    module.private_protocol_request([], {module.PROTOCOL_KEY: json.dumps(invalid)})
            with self.subTest(module=module.__name__, case="duplicate"):
                raw = json.dumps(payload)[:-1] + ',"operation":"status"}'
                with self.assertRaises(ValueError):
                    module.private_protocol_request([], {module.PROTOCOL_KEY: raw})
            with self.subTest(module=module.__name__, case="foreign-protocol"):
                with self.assertRaises(ValueError):
                    module.private_protocol_request(
                        [],
                        {
                            module.PROTOCOL_KEY: json.dumps(payload),
                            module.FOREIGN_PROTOCOL_KEY: "untrusted",
                        },
                    )

    def test_runtime_rejects_invalid_adapter_generation_timeout_and_sha(self):
        build = runtime_protocol("build")
        for changes in (
            {"expected_head": "0" * 40},
            {"adapter_contract": "unknown"},
            {"adapter_contract": "legacy-stable-readiness-b0-v1"},
        ):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                restore_runtime.private_protocol_request(
                    [], {restore_runtime.PROTOCOL_KEY: json.dumps({**build, **changes})}
                )
        for operation, name, value in (("stop", "generation", 0), ("start", "timeout", 0)):
            payload = runtime_protocol(operation)
            payload[name] = value
            with self.assertRaises(ValueError):
                restore_runtime.private_protocol_request(
                    [], {restore_runtime.PROTOCOL_KEY: json.dumps(payload)}
                )

    def test_source_recovery_requires_explicit_write_and_exact_bound_paths(self):
        payload = source_protocol("verify-recover")
        for changes in ({"write": False}, {"write": 1}, {"output": "relative.json"},
                        {"source_generation": ""}, {"resume": True}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                restore_source.private_protocol_request(
                    [], {restore_source.PROTOCOL_KEY: json.dumps({**payload, **changes})}
                )

    def test_protocol_is_removed_before_business_dispatch(self):
        for module, payload in (
            (restore_runtime, runtime_protocol("status")),
            (restore_source, source_protocol("comparison-verify")),
        ):
            raw = json.dumps(payload)

            def execute(_request, key=module.PROTOCOL_KEY):
                self.assertNotIn(key, os.environ)
                return {"status": "checked"}

            with self.subTest(module=module.__name__), patch.dict(
                os.environ, {module.PROTOCOL_KEY: raw}, clear=True
            ), patch.object(sys, "argv", [module.__file__]), patch.object(
                module, "execute", side_effect=execute
            ), patch("builtins.print"):
                module.main()

    def test_direct_script_argv_is_rejected_with_parameter_exit_code(self):
        for script in (ROOT / "tools/python/restore_runtime.py", ROOT / "tools/python/restore_source.py"):
            result = subprocess.run(
                [sys.executable, "-B", str(script), "verify"],
                cwd=ROOT,
                env=private_environment(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                check=False,
            )
            self.assertEqual(result.returncode, 2, result.stderr)
            self.assertNotIn(str(ROOT), result.stderr)

    def test_business_failure_preserves_the_task_exit_code_and_diagnostic(self):
        runtime = runtime_protocol("status")
        runtime["runtime_registration"] = str(
            ROOT / ".local-tests" / f"missing-runtime-{os.getpid()}.json"
        )
        source = source_protocol("comparison-verify")
        source["receipt"] = str(
            ROOT / ".local-tests" / f"missing-source-{os.getpid()}.json"
        )
        cases = (
            (
                ROOT / "tools/python/restore_runtime.py",
                restore_runtime.PROTOCOL_KEY,
                runtime,
            ),
            (
                ROOT / "tools/python/restore_source.py",
                restore_source.PROTOCOL_KEY,
                source,
            ),
        )
        for script, key, protocol in cases:
            environment = private_environment()
            environment[key] = json.dumps(protocol)
            result = subprocess.run(
                [sys.executable, "-B", str(script)],
                cwd=ROOT,
                env=environment,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                encoding="utf-8",
                check=False,
            )
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("Traceback", result.stderr)


if __name__ == "__main__":
    unittest.main()
