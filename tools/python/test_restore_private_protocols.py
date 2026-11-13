import json
import os
from pathlib import Path
import subprocess
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parent))
from rate_limit_tool import tool_binding

ROOT = Path(__file__).resolve().parents[2]


def private_environment() -> dict[str, str]:
    values = dict(os.environ)
    values["PYTHONIOENCODING"] = "utf-8"
    return values


class NativeBindingEntrypointProcesses(unittest.TestCase):
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
