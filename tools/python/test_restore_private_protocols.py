import io
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_runtime
import restore_source
from rate_limit_tool import tool_binding
from workspace_directory import WorkspaceDirectory

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
    values = dict(os.environ)
    values["PYTHONIOENCODING"] = "utf-8"
    return values


class RestorePrivateProtocolTests(unittest.TestCase):
    def test_runtime_decodes_every_operation_without_argv(self):
        for operation in sorted(restore_runtime.OPERATIONS):
            with self.subTest(operation=operation):
                request = restore_runtime.decode_request(json.dumps(runtime_protocol(operation)))
                self.assertEqual(request.operation, operation)
                self.assertEqual(
                    request.namespace().write,
                    operation in restore_runtime.WRITING_OPERATIONS,
                )

    def test_source_decodes_every_operation_from_bounded_stdin(self):
        for operation in sorted(restore_source.OPERATIONS):
            with self.subTest(operation=operation):
                request = restore_source.private_protocol_request(
                    ["--private-stdin"],
                    io.BytesIO(json.dumps(source_protocol(operation)).encode("utf-8")),
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
                    self.decode(module, json.dumps(invalid))
            with self.subTest(module=module.__name__, case="write"):
                invalid = {**payload, "write": True}
                with self.assertRaises(ValueError):
                    self.decode(module, json.dumps(invalid))
            with self.subTest(module=module.__name__, case="duplicate"):
                raw = json.dumps(payload)[:-1] + ',"operation":"status"}'
                with self.assertRaises(ValueError):
                    self.decode(module, raw)

    @staticmethod
    def decode(module, raw):
        if module is restore_runtime:
            return module.decode_request(raw)
        return module.decode_request(raw)

    def test_runtime_rejects_invalid_adapter_generation_timeout_and_sha(self):
        build = runtime_protocol("build")
        for changes in (
            {"expected_head": "0" * 40},
            {"adapter_contract": "unknown"},
            {"adapter_contract": "legacy-stable-readiness-b0-v1"},
        ):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                restore_runtime.decode_request(json.dumps({**build, **changes}))
        for operation, name, value in (("stop", "generation", 0), ("start", "timeout", 0)):
            payload = runtime_protocol(operation)
            payload[name] = value
            with self.assertRaises(ValueError):
                restore_runtime.decode_request(json.dumps(payload))

    def test_source_recovery_requires_explicit_write_and_exact_bound_paths(self):
        payload = source_protocol("verify-recover")
        for changes in ({"write": False}, {"write": 1}, {"output": "relative.json"},
                        {"source_generation": ""}, {"resume": True}):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                restore_source.decode_request(json.dumps({**payload, **changes}))

    def test_source_stdin_ignores_retired_environment_before_dispatch(self):
        payload = source_protocol("comparison-verify")
        retired = {
            "RYFRAME_RESTORE_SOURCE_PROTOCOL": "invalid inherited source",
            "RYFRAME_RESTORE_SOURCE_UNKNOWN": "invalid inherited field",
            "RYFRAME_RESTORE_RUNTIME_PROTOCOL": "invalid inherited runtime",
        }
        with patch.dict(os.environ, retired), patch.object(
            sys, "argv", [restore_source.__file__, "--private-stdin"]
        ), patch.object(
            sys, "stdin", SimpleNamespace(buffer=io.BytesIO(json.dumps(payload).encode("utf-8")))
        ), patch.object(restore_source, "execute", return_value={"status": "checked"}) as execute, \
                patch("builtins.print"):
            restore_source.main()
            execute.assert_called_once()
            self.assertEqual(execute.call_args.args[0].operation, "comparison-verify")
            self.assertEqual({key: os.environ[key] for key in retired}, retired)

    def test_source_transport_limits_bytes_and_requires_exact_marker(self):
        raw = json.dumps(source_protocol("comparison-verify")).encode("utf-8")
        exact = io.BytesIO(raw + b" " * (restore_source.PROTOCOL_MAX_BYTES - len(raw)))
        self.assertEqual(restore_source.private_protocol_request(
            ["--private-stdin"], exact
        ).operation, "comparison-verify")
        oversized = io.BytesIO(b"x" * (restore_source.PROTOCOL_MAX_BYTES * 2))
        with self.assertRaises(restore_source.SourceProtocolError):
            restore_source.private_protocol_request(["--private-stdin"], oversized)
        self.assertEqual(oversized.tell(), restore_source.PROTOCOL_MAX_BYTES + 1)
        for arguments in ([], ["verify"], ["--private-stdin", "extra"],
                          ["--private-stdin", "--private-stdin"]):
            stream = io.BytesIO(raw)
            with self.subTest(arguments=arguments), self.assertRaises(restore_source.SourceProtocolError):
                restore_source.private_protocol_request(arguments, stream)
            self.assertEqual(stream.tell(), 0)

    def test_source_rejects_invalid_requests_before_dispatch_without_file_changes(self):
        valid = source_protocol("comparison-verify")
        invalid = [b"", b"\xff", b"\xef\xbb\xbf{}", b"[]", b"null", b"{", b"{}",
                   json.dumps(valid).encode() + b"\n",
                   (json.dumps(valid)[:-1] + ',"operation":"verify"}').encode()]
        for changes in ({"format_version": True}, {"format_version": 2}, {"kind": "other"},
                        {"operation": "unknown"}, {"write": 1}, {"write": True},
                        {"receipt": "relative"}, {"receipt": ""}, {"receipt": str(ROOT) + "\0"},
                        {"receipt": str(ROOT) + "\n"}, {"receipt": str(ROOT) + "\r"},
                        {"receipt": float("nan")}, {"receipt": float("inf")},
                        {"receipt": -float("inf")}, {"unknown": "value"}):
            invalid.append(json.dumps({**valid, **changes}).encode("utf-8"))
        invalid.append(json.dumps({**valid, "receipt": str(ROOT / ("界" * 6000))},
                                  ensure_ascii=False).encode("utf-8"))
        with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
            sentinel = Path(directory) / "unchanged.json"
            sentinel.write_bytes(b"{\"before\":true}")
            before = {path.name: path.read_bytes() for path in Path(directory).iterdir()}
            for raw in invalid:
                with self.subTest(raw=raw[:32]), patch.object(
                    sys, "argv", [restore_source.__file__, "--private-stdin"]
                ), patch.object(sys, "stdin", SimpleNamespace(buffer=io.BytesIO(raw))), \
                        patch.object(restore_source, "execute") as execute:
                    with self.assertRaises(restore_source.SourceProtocolError):
                        restore_source.main()
                    execute.assert_not_called()
                    self.assertEqual({path.name: path.read_bytes() for path in Path(directory).iterdir()}, before)

    def test_source_cancel_and_stdin_read_failure_never_dispatch(self):
        for failure in (OSError("unavailable"), KeyboardInterrupt()):
            stream = io.BytesIO()
            with patch.object(stream, "read", side_effect=failure) as read, patch.object(
                sys, "stdin", SimpleNamespace(buffer=stream)
            ), patch.object(sys, "argv", [restore_source.__file__, "--private-stdin"]), \
                    patch.object(restore_source, "execute") as execute:
                with self.assertRaises(restore_source.SourceProtocolError if isinstance(failure, OSError)
                                       else KeyboardInterrupt):
                    restore_source.main()
                read.assert_called_once_with(restore_source.PROTOCOL_MAX_BYTES + 1)
                execute.assert_not_called()

    def test_source_script_rejects_bad_transport_even_with_valid_retired_environment(self):
        raw = json.dumps(source_protocol("comparison-verify")).encode("utf-8")
        environment = private_environment()
        environment.update({
            "RYFRAME_RESTORE_SOURCE_PROTOCOL": raw.decode("utf-8"),
            "RYFRAME_RESTORE_RUNTIME_PROTOCOL": json.dumps(runtime_protocol("status")),
        })
        cases = (([], raw), (["--private-stdin", "extra"], raw),
                 (["--private-stdin"], b""), (["--private-stdin"], b"\xff"),
                 (["--private-stdin"], b"x" * (restore_source.PROTOCOL_MAX_BYTES + 1)))
        with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
            sentinel = Path(directory) / "unchanged.json"
            sentinel.write_bytes(b"{\"before\":true}")
            before = {path.name: path.read_bytes() for path in Path(directory).iterdir()}
            for arguments, data in cases:
                with self.subTest(arguments=arguments, size=len(data)):
                    result = subprocess.run(
                        [sys.executable, "-X", "utf8", "-B", restore_source.__file__, *arguments],
                        cwd=directory, input=data, env=environment,
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=30,
                    )
                    self.assertEqual(result.returncode, 2, result.stderr.decode("utf-8"))
                    self.assertEqual(result.stdout, b"")
                    self.assertNotIn(b"Traceback", result.stderr)
                    self.assertEqual({path.name: path.read_bytes() for path in Path(directory).iterdir()}, before)

    def test_runtime_file_protocol_preserves_stdin_and_ignores_ambient_protocol(self):
        with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
            path = Path(directory) / "请求 file.json"
            envelope = {"request": runtime_protocol("status"), "native_tool": json.dumps(tool_binding())}
            path.write_text(json.dumps(envelope), encoding="utf-8")
            with patch.dict(os.environ, {"RYFRAME_RESTORE_RUNTIME_PROTOCOL": "retired"}), patch.object(
                sys, "stdin", object()
            ) as input_stream, patch.object(restore_runtime, "install_binding") as install:
                request = restore_runtime.private_protocol_request(["--request-file", str(path)])
                self.assertEqual(request.operation, "status")
                self.assertIs(sys.stdin, input_stream)
                install.assert_called_once_with(envelope["native_tool"])
            for args in ([], ["--request-file", str(path), "extra"], ["--environment", str(path)]):
                with self.assertRaises(ValueError):
                    restore_runtime.private_protocol_request(args)
            for raw in ("{}", "x" * (restore_runtime.PROTOCOL_MAX_BYTES + 1),
                        json.dumps({**runtime_protocol("status"), "environment": {}})):
                path.write_text(raw, encoding="utf-8")
                with self.assertRaises(ValueError):
                    restore_runtime.private_protocol_request(["--request-file", str(path)])

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
                None,
                runtime,
            ),
            (
                ROOT / "tools/python/restore_source.py",
                "stdin",
                source,
            ),
        )
        for script, key, protocol in cases:
            environment = private_environment()
            arguments = [sys.executable, "-X", "utf8", "-B", str(script)]
            with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
                if key is None:
                    path = Path(directory) / "request.json"
                    path.write_text(json.dumps({"request": protocol, "native_tool": json.dumps(tool_binding())}), encoding="utf-8")
                    arguments += ["--request-file", str(path)]
                else:
                    arguments += ["--private-stdin"]
                result = subprocess.run(
                    arguments, cwd=ROOT, env=environment,
                    input=json.dumps(protocol) if key == "stdin" else None,
                    stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                    text=True, encoding="utf-8", check=False,
                )
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn("Traceback", result.stderr)


if __name__ == "__main__":
    unittest.main()
