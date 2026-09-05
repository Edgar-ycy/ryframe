"""Redis 控制设施的离线边界与显式 WSL Python 子进程测试；不运行 Redis。"""
import copy
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch

import devex_clone_cache_process as process


BOOT = "5a81dc04-9e9a-416b-8eab-1b059bddf489"


class CacheProcessTests(unittest.TestCase):
    def setUp(self):
        self.backend = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        temporary = tempfile.TemporaryDirectory(dir=self.backend / ".local-tests/tmp", prefix="cp-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        data = self.root / "data"
        data.mkdir()
        config = data / "redis.conf"
        config.write_text('save ""\nappendonly no\n', encoding="utf-8")
        wsl = self.root / "wsl.exe"
        wsl.write_bytes(b"isolated stub")
        self.request = {"scope_id": "cache-test", "wsl": {"path": str(wsl), "sha256": process.bound(wsl)["sha256"]},
                        "distribution": "Ubuntu-24.04", "launcher": "/usr/bin/redis-server", "executable": "/usr/bin/redis-check-rdb",
                        "sha256": "a" * 64, "configuration": process.bound(config),
                        "directory": {"path": str(data), "device": data.stat().st_dev, "inode": data.stat().st_ino},
                        "previous_identity": {"pid": 200, "started": "50", "executable": "/usr/bin/redis-check-rdb"},
                        "previous_boot_id": BOOT, "previous_run_id": "a" * 40, "port": 16390,
                        "password_env": "APP_REDIS_PASSWORD", "timeout_seconds": 1,
                        "python": {"path": "/usr/bin/python3", "executable": "/usr/bin/python3.12", "sha256": "b" * 64}}
        self.output = self.root / "process"
        self.output.mkdir()
        self.identity = {"pid": 300, "started": "60", "executable": self.request["executable"], "boot_id": BOOT}

    def intent(self):
        process.prepare(self.request, self.output)
        process.write(self.output / "intent.json", {"request_sha256": process.canonical(self.request),
                      "arguments": process.command(self.request, self.output, "serve"), "launch": process.bound(self.output / "launch-request.json")})

    def recorded(self):
        self.intent()
        launcher = {"pid": 400, "started": "70", "executable": self.request["wsl"]["path"]}
        process.write(self.output / "launcher.json", {"identity": launcher, "intent": process.bound(self.output / "intent.json")})
        process.write(self.output / "linux-intent.json", {"request_sha256": process.canonical(self.request),
                      "arguments": [self.request["launcher"], process.linux_path(self.request["configuration"]["path"])], "supervisor": {}})
        process.write(self.output / "linux-process.json", {"request_sha256": process.canonical(self.request), "identity": self.identity,
                      "intent": {**process.bound(self.output / "linux-intent.json"), "path": process.linux_path(str(self.output / "linux-intent.json"))}})
        return process.inspect_start(self.request, self.output)

    def facts(self):
        return {"process_id": "300", "run_id": "b" * 40, "config_file": process.linux_path(self.request["configuration"]["path"]),
                "configuration_sha256": process.canonical(process.configuration(self.request))}

    def test_original_files_directory_and_exact_fields(self):
        process.validate(self.request)
        for update in ({"port": True}, {"previous_boot_id": "unknown"}, {"extra": True}):
            with self.subTest(update=tuple(update)), self.assertRaises(ValueError):
                process.validate({**self.request, **update})
        bad = copy.deepcopy(self.request)
        bad["directory"]["inode"] += 1
        with self.assertRaises(ValueError):
            process.validate(bad)
        Path(self.request["configuration"]["path"]).write_bytes(b"changed")
        with self.assertRaises(ValueError):
            process.validate(self.request)

    def test_atomic_publication_never_exposes_partial_json_and_never_overwrites(self):
        target = self.output / "receipt.json"
        original = json.dump
        entered, finish = threading.Event(), threading.Event()
        errors = []
        def slow(value, stream, **kwargs):
            stream.write(" ")
            stream.flush()
            entered.set()
            if not finish.wait(5):
                raise TimeoutError()
            original(value, stream, **kwargs)
        def publish():
            try:
                process.write(target, {"identity": self.identity})
            except BaseException as error:
                errors.append(error)
        with patch.object(process.json, "dump", side_effect=slow):
            worker = threading.Thread(target=publish)
            worker.start()
            self.assertTrue(entered.wait(5))
            self.assertFalse(target.exists())
            finish.set()
            worker.join(5)
        self.assertFalse(worker.is_alive())
        self.assertFalse(errors)
        self.assertEqual(process.read(target), {"identity": self.identity})
        before = target.read_bytes()
        with self.assertRaises(FileExistsError):
            process.write(target, {"replacement": True})
        self.assertEqual(target.read_bytes(), before)

    def test_no_intent_is_not_started_but_popen_unknown_is_not_inferred(self):
        self.assertIsNone(process.inspect_start(self.request, self.output))
        self.intent()
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)

    def test_inspection_is_local_and_frozen_helper_changes_rejected(self):
        runtime = self.recorded()
        with patch.object(process.subprocess, "run", side_effect=AssertionError("not local")):
            self.assertEqual(process.inspect_start(self.request, self.output), runtime)
        (self.output / "helpers/cache.py").write_bytes(b"changed")
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)

    def test_other_request_or_launcher_identity_is_rejected(self):
        self.recorded()
        with self.assertRaises(ValueError):
            process.inspect_start({**self.request, "scope_id": "another-run"}, self.output)
        path = self.output / "launcher.json"
        value = process.read(path)
        value["identity"]["started"] = False
        path.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)

    def test_info_identity_config_and_run_are_all_required(self):
        process.check_facts(self.request, self.identity, self.facts())
        for key, value in (("process_id", "999"), ("run_id", self.request["previous_run_id"]),
                           ("config_file", "/another/redis.conf"), ("configuration_sha256", "0" * 64)):
            with self.subTest(key=key), self.assertRaises(ValueError):
                process.check_facts(self.request, self.identity, {**self.facts(), key: value})
        with self.assertRaises(ValueError):
            process.check_facts(self.request, self.identity, self.facts(), run_id="c" * 40)

    def test_resp_truncation_error_invalid_utf8_and_excess_are_rejected(self):
        for raw in (b"-ERR secret password\r\n", b"$2\r\na\r\n", b"$1\r\n\xff\r\n", b"*33\r\n", b"+OK\n"):
            with self.subTest(raw=raw[:1]), self.assertRaises((ValueError, UnicodeError)) as result:
                process.response(io.BytesIO(raw))
            self.assertNotIn("secret password", str(result.exception))

    def test_protocol_uses_only_auth_info_and_fixed_configuration(self):
        def bulk(value):
            raw = value.encode()
            return f"${len(raw)}\r\n".encode() + raw + b"\r\n"
        raw = b"+OK\r\n" + bulk("process_id:300\r\nrun_id:" + "b" * 40 + "\r\nconfig_file:" + self.facts()["config_file"])
        for key, value in process.configuration(self.request).items():
            raw += b"*2\r\n" + bulk(key) + bulk(value)
        connection = Mock()
        connection.__enter__ = Mock(return_value=connection)
        connection.__exit__ = Mock(return_value=False)
        connection.makefile.return_value = io.BytesIO(raw)
        with patch.object(process.socket, "create_connection", return_value=connection):
            self.assertEqual(process.redis_facts(self.request, {"APP_REDIS_PASSWORD": "private-value"}), self.facts())
        commands = [call.args[0] for call in connection.sendall.call_args_list]
        self.assertEqual(len(commands), 10)
        self.assertIn(b"AUTH", commands[0])
        self.assertIn(b"INFO", commands[1])
        self.assertTrue(all(b"CONFIG" in item and b"GET" in item for item in commands[2:]))

    def test_boot_change_never_signals_reused_pid(self):
        kernel = Mock()
        with patch.object(process, "boot_id", return_value="another-boot"):
            self.assertFalse(process.linux_stop(kernel, self.identity))
        kernel.terminate_owned_process.assert_not_called()
        with patch.object(process, "boot_id", return_value=BOOT):
            process.linux_stop(kernel, self.identity)
        kernel.terminate_owned_process.assert_called_once_with({key: self.identity[key] for key in ("pid", "started", "executable")})

    def test_linux_control_refuses_unrecorded_identity_before_signal(self):
        self.recorded()
        payload = process.read(self.output / "launch-request.json")
        kernel = Mock()
        with patch.object(process, "linux_inputs", return_value=(self.output, kernel)), self.assertRaises(ValueError):
            process.linux_check(payload, {**self.identity, "pid": 999}, stop=True)
        kernel.terminate_owned_process.assert_not_called()

    def test_status_requires_both_processes_and_closed_port_without_files(self):
        runtime = self.recorded()
        before = set(self.output.rglob("*"))
        stopped = {"identity": self.identity, "alive": False, "boot_id": BOOT, "terminated": False}
        with patch.object(process, "windows_identity", return_value=False), patch.object(process, "linux_call", return_value=stopped), \
                patch("devex_clone_source_proof.require_closed_port") as port:
            self.assertEqual(process.status(self.request, runtime)["state"], "stopped")
            port.assert_called_once()
        self.assertEqual(before, set(self.output.rglob("*")))
        with patch.object(process, "windows_identity", return_value=True), patch.object(process, "linux_call", return_value=stopped), self.assertRaises(ValueError):
            process.status(self.request, runtime)

    def test_launch_constructor_exception_remains_unknown(self):
        with patch("devex_clone_source_proof.require_closed_port"), patch.object(process.subprocess, "Popen", side_effect=OSError("injected")):
            with self.assertRaises(OSError):
                process.start(self.request, {}, self.output, lambda: None)
        with self.assertRaises(ValueError):
            process.inspect_start(self.request, self.output)
        self.assertFalse((self.output / "partial.json").exists())

    def test_guard_before_popen_has_no_intent(self):
        with patch.object(process.subprocess, "Popen") as popen, self.assertRaises(RuntimeError):
            process.start(self.request, {}, self.output, lambda: (_ for _ in ()).throw(RuntimeError("guard")))
        popen.assert_not_called()
        self.assertIsNone(process.inspect_start(self.request, self.output))

    def test_authorization_eof_and_wrong_run_never_launch_redis(self):
        kernel = Mock()
        payload = {"request": self.request, "request_sha256": process.canonical(self.request), "windows_output": str(self.output)}
        for data in ("", '{}\n'):
            with self.subTest(data=bool(data)), patch.object(process, "linux_inputs", return_value=(self.output, kernel)), \
                    patch.object(process.sys, "stdin", io.StringIO(data)), patch.object(process.subprocess, "Popen") as popen, self.assertRaises(ValueError):
                process.linux_serve(payload)
            popen.assert_not_called()

    def test_launcher_exits_before_authorization_has_explicit_partial_proof(self):
        child = Mock(pid=777, stdin=io.BytesIO(), returncode=1)
        child.wait.return_value = 1
        with patch("devex_clone_source_proof.require_closed_port"), patch.object(process.subprocess, "Popen", return_value=child), \
                patch("full_stack_process.process_identity", return_value=None), self.assertRaises(ValueError):
            process.start(self.request, {}, self.output, lambda: None)
        result = process.inspect_start(self.request, self.output)
        self.assertEqual(result["state"], "not_started")
        self.assertFalse(process.read(self.output / "partial.json")["authorization_sent"])

    @unittest.skipUnless(os.environ.get("RYFRAME_WSL_STUB_TEST") == "1", "显式 WSL Python stub 验证，不启动 Redis")
    def test_wsl_supervisor_crash_keeps_child_identity_for_pidfd_recovery(self):
        script = self.root / "orphan.py"
        child = self.root / "child.py"
        child.write_text("import time; time.sleep(30)\n", encoding="utf-8")
        helper = process.linux_path(str(Path(process.__file__).resolve()))
        kernel = process.linux_path(str(self.backend / "scripts/full_stack_process.py"))
        output = process.linux_path(str(self.output))
        script.write_text("\n".join([
            "import importlib.util,json,os,sys,subprocess,time,socket", "from pathlib import Path", "sys.dont_write_bytecode=True",
            "def load(name,path):", " s=importlib.util.spec_from_file_location(name,path); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); return m",
            f"p=load('cache',{helper!r}); k=load('kernel',{kernel!r}); out=Path({output!r})",
            f"request=json.loads({json.dumps(self.request)!r})",
            f"request.update(launcher=sys.executable,executable=str(Path(sys.executable).resolve()),configuration={{'path':{str(child)!r}}})",
            "request['previous_identity']['pid']=2147483646",
            "with socket.socket() as sock: sock.bind(('127.0.0.1',0)); request['port']=sock.getsockname()[1]",
            f"payload={{'request':request,'request_sha256':p.canonical(request),'windows_output':{str(self.output)!r}}}",
            "if len(sys.argv)>1:", " payload=p.read(out/'payload.json'); p.linux_inputs=lambda *_args,**_kwargs:(out,k); p.linux_serve(payload); sys.exit()",
            "p.write(out/'payload.json',payload)",
            "parent=subprocess.Popen([sys.executable,__file__,'serve'],stdin=subprocess.PIPE,stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)",
            "expected=None",
            "try:", " parent.stdin.write((json.dumps({'request_sha256':payload['request_sha256'],'output':payload['windows_output']})+'\\n').encode()); parent.stdin.close()",
            " deadline=time.monotonic()+8",
            " while not (out/'linux-process.json').exists():",
            "  assert parent.poll() is None, parent.stderr.read().decode()",
            "  assert time.monotonic()<deadline; time.sleep(.01)",
            " expected=p.read(out/'linux-process.json')['identity']",
            " p.write(out/'acknowledged.json',{'request_sha256':payload['request_sha256'],'identity':expected})",
            " while not (out/'accepted.json').exists(): assert time.monotonic()<deadline; time.sleep(.01)",
            " parent.kill(); parent.wait(timeout=5)", " assert p.linux_identity(k,expected['pid'])==expected",
            " p.linux_inputs=lambda *_args,**_kwargs:(out,k)",
            " result=p.linux_check(payload,expected,stop=True)", " assert result['terminated'] and not result['alive']",
            " print(json.dumps({'orphan_registered':True,'pidfd_recovered':True}))",
            "finally:", " if parent.poll() is None: parent.kill(); parent.wait(timeout=5)",
            " if expected is not None: p.linux_stop(k,expected)",
        ]), encoding="utf-8")
        result = subprocess.run(["C:/Windows/System32/wsl.exe", "--distribution", "Ubuntu-24.04", "--exec", "/usr/bin/python3",
                                 process.linux_path(str(script))], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", errors="replace"))
        self.assertEqual(json.loads(result.stdout), {"orphan_registered": True, "pidfd_recovered": True})

    @unittest.skipUnless(os.environ.get("RYFRAME_WSL_STUB_TEST") == "1", "显式 WSL Python stub 验证，不启动 Redis")
    def test_wsl_atomic_publish_and_pidfd_exact_cleanup(self):
        script = self.root / "native.py"
        helper = process.linux_path(str(Path(process.__file__).resolve()))
        kernel = process.linux_path(str(self.backend / "scripts/full_stack_process.py"))
        output = process.linux_path(str(self.output))
        script.write_text("\n".join([
            "import importlib.util,json,os,sys,subprocess,time", "from pathlib import Path",
            "sys.dont_write_bytecode=True", "def load(name,path):",
            " s=importlib.util.spec_from_file_location(name,path); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); return m",
            f"p=load('cache',{helper!r}); k=load('kernel',{kernel!r}); out=Path({output!r})",
            "p.write(out/'wsl-published.json',{'complete':True})",
            "try: p.write(out/'wsl-published.json',{'replaced':True}); raise AssertionError('overwritten')",
            "except FileExistsError: pass", "child=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)'])",
            "try:", " identity=p.linux_identity(k,child.pid)",
            " try: p.linux_stop(k,{**identity,'started':str(int(identity['started'])+1)}); raise AssertionError('mismatch accepted')",
            " except ValueError: pass", " assert child.poll() is None", " assert p.linux_stop(k,identity)", " child.wait(timeout=10)",
            " assert p.linux_identity(k,child.pid) is None", " print(json.dumps({'atomic_publish':True,'pidfd_cleanup':True}))",
            "finally:", " if child.poll() is None: child.kill(); child.wait(timeout=10)",
        ]), encoding="utf-8")
        result = subprocess.run(["C:/Windows/System32/wsl.exe", "--distribution", "Ubuntu-24.04", "--exec", "/usr/bin/python3",
                                 process.linux_path(str(script))], stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr.decode("utf-8", errors="replace"))
        self.assertEqual(json.loads(result.stdout), {"atomic_publish": True, "pidfd_cleanup": True})
        self.assertEqual(process.read(self.output / "wsl-published.json"), {"complete": True})


if __name__ == "__main__":
    unittest.main()
