import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from rate_limit_tool import tool_binding
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]


def private_environment() -> dict[str, str]:
    values = dict(os.environ)
    values["PYTHONIOENCODING"] = "utf-8"
    return values


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

    def test_reference_entry_rejects_missing_or_bad_tools_before_children_and_writes(self):
        registered = tool_binding()
        self.assertIsNotNone(registered, "本测试须由正式 run_checks 绑定工具")
        with WorkspaceDirectory(dir=ROOT / ".local-tests/python-unit") as directory:
            root = Path(directory)
            evidence = root / "input.json"
            evidence.write_text("{}", encoding="utf-8")
            bad = {**registered, "sha256": "0" * 64}
            payload = json.dumps({
                "format_version": 1,
                "kind": "ryframe-xtask-recovery-reference",
                "request": {
                    "backend_dir": str(ROOT), "format_version": 1, "operation": "check-existing",
                    "plan": str(evidence), "side": "source", "write": False,
                },
            })
            script = ROOT / "tools/python/restore_reference.py"
            audit = 'NATIVE_BINDING_AUDIT={"subprocesses": 0}'
            for binding in (None, bad):
                with self.subTest(missing=binding is None):
                    tool_args = [] if binding is None else ["--native-tool", json.dumps(binding)]
                    before = {str(path.relative_to(root)):(None if path.is_dir() else path.read_bytes()) for path in root.rglob("*")}
                    result = subprocess.run(
                        [sys.executable, "-X", "utf8", "-B", "-c", self.DRIVER, str(script), *tool_args, "--stdin"],
                        input=payload, cwd=ROOT, env=private_environment(),
                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8",
                        timeout=60, check=False,
                    )
                    self.assertNotIn("SyntaxError", result.stderr)
                    diagnostic = result.stderr.replace(audit + "\n", "").strip()
                    self.assertEqual(result.returncode, 2)
                    self.assertEqual(diagnostic, "restore_reference_protocol_error：私有参数协议无效。")
                    self.assertEqual(result.stdout, "")
                    self.assertIn(audit, result.stderr)
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
