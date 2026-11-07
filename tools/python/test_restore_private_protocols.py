import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_runtime
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

    def test_runtime_rejects_duplicates_unknown_fields_and_wrong_write(self):
        payload = runtime_protocol("status")
        for case, raw in (
            ("unknown", json.dumps({**payload, "unknown": "value"})),
            ("write", json.dumps({**payload, "write": not payload["write"]})),
            ("duplicate", json.dumps(payload)[:-1] + ',"operation":"status"}'),
        ):
            with self.subTest(case=case), self.assertRaises(ValueError):
                restore_runtime.decode_request(raw)

    def test_runtime_rejects_invalid_adapter_generation_and_timeout(self):
        start = runtime_protocol("start")
        for changes in (
            {"adapter_contract": "unknown"},
            {"adapter_contract": "legacy-stable-readiness-b0-v1"},
        ):
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                restore_runtime.decode_request(json.dumps({**start, **changes}))
        for operation, name, value in (("stop", "generation", 0), ("start", "timeout", 0)):
            payload = runtime_protocol(operation)
            payload[name] = value
            with self.assertRaises(ValueError):
                restore_runtime.decode_request(json.dumps(payload))

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
        result = subprocess.run(
            [sys.executable, "-B", str(ROOT / "tools/python/restore_runtime.py"), "verify"],
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
        with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
            path = Path(directory) / "request.json"
            path.write_text(json.dumps({"request": runtime, "native_tool": json.dumps(tool_binding())}), encoding="utf-8")
            result = subprocess.run(
                [sys.executable, "-X", "utf8", "-B", str(ROOT / "tools/python/restore_runtime.py"),
                 "--request-file", str(path)],
                cwd=ROOT, env=private_environment(),
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

    def test_formal_entries_reject_missing_or_bad_tools_before_children_and_writes(self):
        registered = tool_binding()
        self.assertIsNotNone(registered, "本测试须由正式 run_checks 绑定工具")
        with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
            root = Path(directory)
            evidence = root / "input.json"
            evidence.write_text("{}", encoding="utf-8")
            bad = {**registered, "sha256": "0" * 64}
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
                    ("restore_reference.py", [*tool_args,"--stdin"], json.dumps({"format_version":1,
                        "kind":"ryframe-xtask-recovery-reference","request":{"backend_dir":str(ROOT),
                        "format_version":1,"operation":"check-existing","plan":str(evidence),"side":"source","write":False}})),
                    ("reference_fixture_successor.py", [], envelope({"format_version":1,
                        "kind":"ryframe-reference-fixture-control","domain":"successor","operation":"relationship",
                        "backend_dir":str(ROOT),"write":True,"source_result":str(evidence),
                        "predecessor_review":str(evidence),"predecessor_request":str(evidence),
                        "successor_review":str(evidence),"seed_request":str(evidence),"base_request":str(evidence),
                        "candidate_request":str(evidence),"id":"binding-test","output":str(root / "output.json")})),
                    ("restore_monitoring_authority.py", [], envelope(monitoring)),
                    ("devex_provenance.py", [], envelope({})),
                ]
                diagnostics = {
                    "restore_reference.py": (2, "restore_reference_protocol_error：私有参数协议无效。"),
                    "restore_monitoring_authority.py": (1, "正式恢复预检失败：ValueError；请核对 runtime verify 的来源与代次证据"),
                }
                for script, arguments, payload in cases:
                    with self.subTest(script=script, missing=binding is None):
                        before = {str(path.relative_to(root)):(None if path.is_dir() else path.read_bytes()) for path in root.rglob("*")}
                        result = subprocess.run([sys.executable,"-X","utf8","-B","-c",self.DRIVER,
                            str(ROOT / "tools/python" / script),*arguments], input=payload,
                            cwd=ROOT, env=private_environment(), stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, encoding="utf-8", timeout=60, check=False)
                        self.assertNotIn("SyntaxError", result.stderr)
                        audit = 'NATIVE_BINDING_AUDIT={"subprocesses": 0}'
                        diagnostic = result.stderr.replace(audit + "\n", "").strip()
                        digest_error = ("持有保护句柄前二进制摘要与登记不符" if os.name == "nt"
                            else "限流工具字节或摘要与当前协调器绑定不同")
                        if script == "devex_provenance.py":
                            self.assertEqual(result.returncode, 1)
                            self.assertEqual(json.loads(result.stdout), {"ok": False, "stage": "bindings"})
                            self.assertEqual(diagnostic, "")
                        elif script == "reference_fixture_successor.py":
                            self.assertEqual(result.returncode, 2 if binding is None else 1)
                            self.assertEqual(diagnostic,
                                "reference_fixture_control_protocol_error：私有控制请求无效。" if binding is None else
                                "reference_fixture_successor_failed：ValueError: " + digest_error
                                + "；请核对原账本和失败证据，不自动重放。")
                        else:
                            code, expected = diagnostics[script]
                            self.assertEqual(result.returncode, code)
                            self.assertEqual(diagnostic, expected)
                        if script != "devex_provenance.py":
                            self.assertEqual(result.stdout, "")
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
