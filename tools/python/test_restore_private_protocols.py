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
        "native_tool": tool_binding(),
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


class NativeBindingEntrypointProcesses(unittest.TestCase):
    DRIVER = """
import atexit,json,runpy,sys
from pathlib import Path
calls=[]
expected=json.loads(sys.argv.pop(1))
rejected=[]
def trace(frame,event,arg):
    if event == 'exception':
        kind,error,_=arg
        observed=[Path(frame.f_code.co_filename).name,frame.f_code.co_name,kind.__name__,str(error)]
        if observed == expected:
            rejected.append(True)
    return trace
sys.settrace(trace)
atexit.register(lambda: print('NATIVE_BINDING_REJECTION='+json.dumps({'matched':bool(rejected)}),file=sys.stderr))
def audit(event,args):
    if event == 'subprocess.Popen':
        calls.append(event)
        raise RuntimeError('unexpected nested subprocess before binding validation')
sys.addaudithook(audit)
atexit.register(lambda: print('NATIVE_BINDING_AUDIT='+json.dumps({'subprocesses':len(calls)}),file=sys.stderr))
sys.argv=sys.argv[1:]
sys.path.insert(0,str(Path(sys.argv[0]).parent))
from rate_limit_tool import tool_binding
assert tool_binding() is None
runpy.run_path(sys.argv[0],run_name='__main__')
"""

    def test_seven_formal_entries_reject_missing_or_bad_tools_before_children_and_writes(self):
        registered = tool_binding()
        self.assertIsNotNone(registered, "本测试须由正式 run_checks 绑定工具")
        with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
            root = Path(directory)
            evidence = root / "input.json"
            evidence.write_text("{}", encoding="utf-8")
            bad = {**registered, "sha256": "0" * 64}
            source = source_protocol("comparison-verify")
            source["receipt"] = str(evidence)
            runtime = {"format_version":1,"operation":"verify","backend_dir":str(ROOT),
                       "environment":str(evidence),"output":str(root),"write":False}
            monitoring = {"format_version":1,"kind":"monitoring-preflight","backend":str(ROOT),
                          "runtime":str(evidence),"target":str(evidence)}
            for binding in (None, bad):
                def envelope(value):
                    result = {key:item for key,item in value.items() if key != "native_tool"}
                    if binding is not None:
                        result["native_tool"] = json.dumps(binding) if "domain" in result else binding
                    return json.dumps(result)
                tool_args = [] if binding is None else ["--native-tool", json.dumps(binding)]
                cases = [
                    ("restore_source.py", ["--private-stdin"], envelope(source)),
                    ("restore_reference.py", [*tool_args,"--stdin"], json.dumps({"format_version":1,
                        "kind":"ryframe-xtask-recovery-reference","request":{"backend_dir":str(ROOT),
                        "format_version":1,"operation":"check-existing","plan":str(evidence),"side":"source","write":False}})),
                    ("restore_reference_post_batch.py", [*tool_args,"--backend-dir",str(ROOT),"--plan",str(evidence),"--input",str(evidence)], ""),
                    ("reference_fixture_runtime.py", ["--protocol-json",envelope(runtime)], ""),
                    ("reference_fixture_successor.py", [], envelope({"format_version":1,
                        "kind":"ryframe-reference-fixture-control","domain":"successor","operation":"relationship",
                        "backend_dir":str(ROOT),"write":True,"source_result":str(evidence),
                        "predecessor_review":str(evidence),"predecessor_request":str(evidence),
                        "successor_review":str(evidence),"seed_request":str(evidence),"base_request":str(evidence),
                        "candidate_request":str(evidence),"id":"binding-test","output":str(root / "output.json")})),
                    ("restore_monitoring_authority.py", [], envelope(monitoring)),
                    ("devex_provenance.py", [], envelope({})),
                ]
                missing_rejections = {
                    "restore_source.py": ["restore_source.py", "decode_request", "SourceProtocolError",
                        "恢复来源私有协议字段不完整或含未知字段"],
                    "restore_reference.py": ["native_s3_client.py", "_invoke", "NativeS3InvocationError",
                        "原生对象操作未确认：native_tool_unavailable；禁止降级或自动重试"],
                    "restore_reference_post_batch.py": ["argparse.py", "exit", "SystemExit", "2"],
                    "reference_fixture_runtime.py": ["reference_fixture_runtime.py", "_exact_protocol",
                        "RuntimeProtocolError", "私有协议字段不完整或含未知字段"],
                    "reference_fixture_successor.py": ["reference_fixture_control_protocol.py", "private_arguments",
                        "FixtureControlProtocolError", "参考夹具控制私有协议字段不完整或含未知字段"],
                    "restore_monitoring_authority.py": ["restore_runtime_evidence.py", "exact_fields",
                        "ValueError", "监控预检协议字段必须精确匹配当前格式"],
                    "devex_provenance.py": ["devex_provenance.py", "private_request", "KeyError", "'native_tool'"],
                }
                for script, arguments, payload in cases:
                    bad_rejection = (
                        ["artifact_digests.py", "__init__", "ValueError", "持有保护句柄前二进制摘要与登记不符"]
                        if os.name == "nt" else
                        ["rate_limit_tool.py", "verified_tool", "ValueError", "限流工具字节或摘要与当前协调器绑定不同"]
                    )
                    expected = missing_rejections[script] if binding is None else bad_rejection
                    with self.subTest(script=script, missing=binding is None):
                        before = {str(path.relative_to(root)):(None if path.is_dir() else path.read_bytes()) for path in root.rglob("*")}
                        result = subprocess.run([sys.executable,"-X","utf8","-B","-c",self.DRIVER,
                            json.dumps(expected), str(ROOT / "tools/python" / script),*arguments], input=payload,
                            cwd=ROOT, env=private_environment(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, encoding="utf-8", timeout=60, check=False)
                        self.assertNotEqual(result.returncode, 0)
                        self.assertIn('NATIVE_BINDING_REJECTION={"matched": true}', result.stderr,
                            f"expected binding rejection {expected!r}: {result.stderr}")
                        if script == "restore_reference_post_batch.py" and binding is None:
                            self.assertIn("the following arguments are required: --native-tool", result.stderr)
                        self.assertNotIn("SyntaxError", result.stderr)
                        self.assertIn('NATIVE_BINDING_AUDIT={"subprocesses": 0}', result.stderr)
                        after = {str(path.relative_to(root)):(None if path.is_dir() else path.read_bytes()) for path in root.rglob("*")}
                        self.assertEqual(before, after)
                        self.assertFalse((root / "output.json").exists())

    def test_fresh_process_uses_only_explicit_binding_for_a_real_readonly_native_request(self):
        registered = tool_binding()
        self.assertIsNotNone(registered, "本测试须由正式 run_checks 绑定工具")
        program = """
import json,sys
from pathlib import Path
sys.path.insert(0,str(Path.cwd()/'tools/python'))
from rate_limit_tool import tool_binding,install_verified_binding
from native_s3_client import _invoke,_read_response
import subprocess
assert tool_binding() is None
install_verified_binding(sys.stdin.read())
value,_=_read_response(*_invoke('__source-evidence',{'operation':'reference-plan-hash','arguments':{'value':{'read_only':True}},'write':False},subprocess.run,may_write=False))
print(json.dumps({'value':value['value']}))
"""
        environment = private_environment()
        environment["RYFRAME_RATE_LIMIT_TOOL_BINDING"] = "ignored inherited binding"
        result = subprocess.run([sys.executable,"-X","utf8","-B","-c",program],
            input=json.dumps(registered), cwd=ROOT, env=environment, text=True, encoding="utf-8",
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        import hashlib
        expected = hashlib.sha256(b'{"read_only":true}').hexdigest()
        self.assertEqual(json.loads(result.stdout), {"value":expected})


if __name__ == "__main__":
    unittest.main()
