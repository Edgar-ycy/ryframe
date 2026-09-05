"""明确 WSL Redis 的运行身份、只读核验和 pidfd 回收；不读取或写入业务 key。"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PureWindowsPath
import re
import shlex
import signal
import socket
import subprocess
import sys
import time
from types import SimpleNamespace
import uuid


FIELDS = {"scope_id", "wsl", "distribution", "launcher", "executable", "sha256", "configuration", "directory",
          "previous_identity", "previous_boot_id", "previous_run_id", "port", "password_env", "timeout_seconds", "python"}


def canonical(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def read(path):
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("缓存进程证据必须是对象")
    return value


def write(path, value):
    path = Path(path)
    pending = path.with_name("." + path.name + "." + uuid.uuid4().hex + ".pending")
    with pending.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, sort_keys=True, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    # 两端只能看到完整收据；link 独占发布，不能覆盖已经登记的证据。
    os.link(pending, path)
    pending.unlink()


def bound(path):
    path = Path(path)
    with path.open("rb") as stream:
        value = hashlib.file_digest(stream, "sha256").hexdigest()
        size = os.fstat(stream.fileno()).st_size
    return {"path": str(path), "bytes": size, "sha256": value}


def plain(path):
    path = Path(path)
    if (not path.is_absolute() or not path.exists() or any(item.is_symlink() or
            getattr(item.lstat(), "st_file_attributes", 0) & 0x400 for item in (path, *path.parents))):
        raise ValueError("缓存控制只接受明确无链接的已有绝对路径")
    return path


def file_bound(value):
    path = plain(value["path"])
    if set(value) != {"path", "bytes", "sha256"} or bound(path) != value:
        raise ValueError("缓存控制文件摘要变化")
    return path


def linux_path(value):
    path = PureWindowsPath(value)
    if (not path.is_absolute() or not re.fullmatch(r"[A-Za-z]:", path.drive)
            or ".." in path.parts or any(":" in part for part in path.parts[1:])):
        raise ValueError("WSL 仅映射已登记 Windows 盘符绝对路径")
    return "/mnt/" + path.drive[0].lower() + "/" + "/".join(path.parts[1:])


def validate(request, *, files=True):
    if set(request) != FIELDS:
        raise ValueError("Redis 进程请求字段不同")
    if (not re.fullmatch(r"[a-z0-9][a-z0-9_-]{2,47}", request["scope_id"])
            or not re.fullmatch(r"[A-Za-z0-9._-]+", request["distribution"])
            or request["launcher"] != "/usr/bin/redis-server" or not re.fullmatch(r"/usr/(?:local/)?bin/[a-z0-9-]+", request["executable"])
            or type(request["port"]) is not int or not 1024 <= request["port"] <= 65535
            or type(request["timeout_seconds"]) is not int or not 1 <= request["timeout_seconds"] <= 180
            or not re.fullmatch(r"[A-Z][A-Z0-9_]+", request["password_env"])
            or not re.fullmatch(r"[a-f0-9]{64}", request["sha256"])
            or not re.fullmatch(r"[a-f0-9]{40}", request["previous_run_id"])
            or not re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", request["previous_boot_id"])):
        raise ValueError("Redis 原进程、端点或工具声明无效")
    previous = request["previous_identity"]
    if (set(previous) != {"pid", "started", "executable"} or type(previous["pid"]) is not int or previous["pid"] <= 1
            or not isinstance(previous["started"], str) or not previous["started"].isdigit() or previous["executable"] != request["executable"]):
        raise ValueError("Redis 原始 Linux 创建身份不完整")
    if set(request["directory"]) != {"path", "device", "inode"} or set(request["wsl"]) != {"path", "sha256"}:
        raise ValueError("Redis 目录或 WSL 绑定字段无效")
    python = request["python"]
    if (set(python) != {"path", "executable", "sha256"} or python["path"] != "/usr/bin/python3"
            or not re.fullmatch(r"/usr/bin/python3\.[0-9]+", python["executable"])
            or not re.fullmatch(r"[a-f0-9]{64}", python["sha256"])):
        raise ValueError("WSL Python 必须绑定明确入口、实际文件与摘要")
    directory = Path(request["directory"]["path"])
    if Path(request["configuration"]["path"]) != directory / "redis.conf":
        raise ValueError("Redis 仅使用原服务目录中的配置文件")
    if files:
        plain(directory)
        if (not directory.is_dir() or type(request["directory"]["device"]) is not int or type(request["directory"]["inode"]) is not int
                or (directory.stat().st_dev, directory.stat().st_ino) != (request["directory"]["device"], request["directory"]["inode"])):
            raise ValueError("Redis 原 Windows 数据目录身份变化")
        file_bound(request["configuration"])
        if bound(plain(request["wsl"]["path"]))["sha256"] != request["wsl"]["sha256"]:
            raise ValueError("WSL 实际工具变化")


def configuration(request):
    return {"bind": "127.0.0.1", "port": str(request["port"]), "dir": linux_path(request["directory"]["path"]),
            "databases": "1", "protected-mode": "yes", "save": "", "appendonly": "no", "daemonize": "no"}


def response(stream, depth=0):
    if depth > 2:
        raise ValueError("Redis 响应嵌套过深")
    line = stream.readline(1048577)
    if len(line) > 1048576 or not line.endswith(b"\r\n"):
        raise ValueError("Redis 响应截断或超界")
    kind, value = line[:1], line[1:-2]
    if kind == b"+":
        return value.decode("utf-8")
    if kind == b"$":
        count = int(value)
        if not 0 <= count <= 1048576:
            raise ValueError("Redis 字符串响应长度无效")
        body = stream.read(count + 2)
        if len(body) != count + 2 or not body.endswith(b"\r\n"):
            raise ValueError("Redis 字符串响应不完整")
        return body[:-2].decode("utf-8")
    if kind == b"*":
        count = int(value)
        if not 0 <= count <= 32:
            raise ValueError("Redis 配置响应条目过多")
        return [response(stream, depth + 1) for _ in range(count)]
    # 不传播可能含密码/命令正文的服务错误。
    raise ValueError("Redis 认证或固定核验命令失败")


def redis_facts(request, environment):
    password = environment.get(request["password_env"])
    if not isinstance(password, str) or not password:
        raise ValueError("Redis 核验必须有原环境中的明确认证")
    with socket.create_connection(("127.0.0.1", request["port"]), timeout=3) as connection, connection.makefile("rb") as stream:
        def send(parts):
            values = [item.encode() for item in parts]
            connection.sendall(f"*{len(values)}\r\n".encode() + b"".join(f"${len(item)}\r\n".encode() + item + b"\r\n" for item in values))
            return response(stream)
        if send(["AUTH", password]) != "OK":
            raise ValueError("Redis 原凭据认证未通过")
        raw = send(["INFO", "server"])
        facts = {}
        for line in raw.splitlines():
            if ":" in line and not line.startswith("#"):
                key, value = line.split(":", 1)
                if key in facts:
                    raise ValueError("Redis INFO 重复字段")
                facts[key] = value
        actual = {}
        for key, value in configuration(request).items():
            result = send(["CONFIG", "GET", key])
            if result != [key, value]:
                raise ValueError("Redis 实际目录、持久化或监听配置变化")
            actual[key] = value
    return {key: facts.get(key) for key in ("process_id", "run_id", "config_file")} | {"configuration_sha256": canonical(actual)}


def check_facts(request, linux_identity, facts, *, run_id=None):
    if (facts.get("process_id") != str(linux_identity["pid"])
            or facts.get("config_file") != linux_path(request["configuration"]["path"])
            or not isinstance(facts.get("run_id"), str) or not re.fullmatch(r"[a-f0-9]{40}", facts["run_id"])
            or facts["run_id"] == request["previous_run_id"] or run_id is not None and facts["run_id"] != run_id
            or facts.get("configuration_sha256") != canonical(configuration(request))):
        raise ValueError("Redis INFO、运行代次和实际配置不属于本次进程")


def boot_id():
    return Path("/proc/sys/kernel/random/boot_id").read_text(encoding="ascii").strip()


def linux_identity(kernel, pid):
    boot = boot_id()
    value = kernel.process_identity(pid)
    if boot_id() != boot:
        raise ValueError("WSL 启动代次在进程观察窗口变化")
    return None if value is None else {**value, "boot_id": boot}


def linux_stop(kernel, expected):
    if boot_id() != expected["boot_id"]:
        return False  # 原 boot 已结束；不操作新 boot 下可能复用的 PID。
    return kernel.terminate_owned_process({key: expected[key] for key in ("pid", "started", "executable")})


def linux_inputs(payload, *, startup):
    if os.name != "posix" or os.environ.get("WSL_DISTRO_NAME") != payload["request"]["distribution"]:
        raise ValueError("Redis 控制仅允许明确 WSL distribution")
    if canonical(payload["request"]) != payload["request_sha256"]:
        raise ValueError("Redis 请求摘要不同")
    output = plain(payload["linux_output"])
    if str(output) != linux_path(payload["windows_output"]):
        raise ValueError("Redis 运行证据跨 OS 路径不同")
    for value in payload["helpers"].values():
        file_bound(value)
    if str(Path(__file__).resolve()) != payload["helpers"]["cache"]["path"]:
        raise ValueError("不是本次冻结的 Redis 控制模块")
    python = payload["request"]["python"]
    if (str(Path(python["path"]).resolve(strict=True)) != python["executable"]
            or str(Path(sys.executable).resolve(strict=True)) != python["executable"]
            or bound(plain(python["executable"]))["sha256"] != python["sha256"]):
        raise ValueError("WSL Python 实际执行产物变化")
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location("cache_pidfd", payload["helpers"]["process"]["path"])
    kernel = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(kernel)
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise ValueError("WSL 缺少 pidfd 精确回收能力")
    if startup:
        request = payload["request"]
        config = plain(linux_path(request["configuration"]["path"]))
        if bound(config)["sha256"] != request["configuration"]["sha256"] or str(Path(request["launcher"]).resolve(strict=True)) != request["executable"]:
            raise ValueError("Redis Linux 配置或实际可执行文件不同")
        if bound(plain(request["executable"]))["sha256"] != request["sha256"]:
            raise ValueError("Redis 实际工具字节不同")
        plain(linux_path(request["directory"]["path"]))
        # 明确禁止后台 fork，才可用创建的子进程身份控制；不输出 requirepass 行。
        entries = {}
        for line in config.read_text(encoding="utf-8").splitlines():
            parts = shlex.split(line, comments=True)
            if parts and parts[0] in configuration(request):
                if parts[0] in entries:
                    raise ValueError("Redis 固定配置有重复指令")
                entries[parts[0]] = " ".join(parts[1:])
        if entries != configuration(request):
            raise ValueError("Redis 原配置缺少非后台、临时状态或精确目录声明")
    return output, kernel


def linux_check(payload, expected, *, stop=False):
    output, kernel = linux_inputs(payload, startup=False)
    receipt = read(output / "linux-process.json")
    intent = read(output / "linux-intent.json")
    if (receipt != {"request_sha256": payload["request_sha256"], "identity": expected, "intent": bound(output / "linux-intent.json")}
            or intent.get("request_sha256") != payload["request_sha256"]
            or intent.get("arguments") != [payload["request"]["launcher"], linux_path(payload["request"]["configuration"]["path"])]) :
        raise ValueError("Redis Linux 控制只能使用原创建收据中的明确身份")
    before = linux_identity(kernel, expected["pid"])
    if before is not None and before != expected and before["boot_id"] == expected["boot_id"]:
        raise ValueError("Redis Linux PID 已复用，拒绝操作")
    terminated = linux_stop(kernel, expected) if stop else False
    after = linux_identity(kernel, expected["pid"])
    alive = after == expected
    if not alive:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.bind(("127.0.0.1", payload["request"]["port"]))
    if stop and alive:
        raise ValueError("Redis 精确回收未完成")
    return {"identity": expected, "alive": alive, "boot_id": boot_id(), "terminated": terminated}


def linux_serve(payload):
    output, kernel = linux_inputs(payload, startup=True)
    request = payload["request"]
    authorization = sys.stdin.readline(16384)
    if not authorization or json.loads(authorization) != {"request_sha256": payload["request_sha256"], "output": payload["windows_output"]}:
        raise ValueError("WSL 启动未获得已登记 Windows 控制器授权")
    old = linux_identity(kernel, request["previous_identity"]["pid"])
    if old is not None and old["boot_id"] == request["previous_boot_id"]:
        raise ValueError("原 Redis 进程仍存在或同 boot PID 已被复用")
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
        probe.bind(("127.0.0.1", request["port"]))
    args = [request["launcher"], linux_path(request["configuration"]["path"])]
    write(output / "linux-intent.json", {"request_sha256": payload["request_sha256"], "arguments": args,
                                       "supervisor": linux_identity(kernel, os.getpid())})
    def interrupted(_signal, _frame):
        raise RuntimeError("Redis Linux supervisor interrupted")
    for watched in (signal.SIGTERM, signal.SIGHUP, signal.SIGINT):
        signal.signal(watched, interrupted)
    child, expected, popen_called = None, None, False
    try:
        with (output / "redis.stdout.log").open("xb") as stdout, (output / "redis.stderr.log").open("xb") as stderr:
            popen_called = True
            child = subprocess.Popen(args, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
                                     cwd=linux_path(request["directory"]["path"]), start_new_session=True)
        actual = linux_identity(kernel, child.pid)
        if actual is None or actual["executable"] != request["executable"]:
            raise ValueError("Redis 新子进程实际身份不同")
        expected = actual
        write(output / "linux-process.json", {"request_sha256": payload["request_sha256"], "identity": expected,
                                            "intent": bound(output / "linux-intent.json")})
        deadline = time.monotonic() + request["timeout_seconds"] + 15
        while not (output / "acknowledged.json").exists():
            if child.poll() is not None or time.monotonic() >= deadline:
                raise TimeoutError("Windows 未在期限内确认本次 Redis 进程")
            time.sleep(0.05)
        ack = read(output / "acknowledged.json")
        if ack != {"request_sha256": payload["request_sha256"], "identity": expected}:
            raise ValueError("Redis 启动确认不属于本次进程")
        write(output / "accepted.json", ack)
        child.wait()
        write(output / "linux-exited.json", {"identity": expected, "returncode": child.returncode})
    except BaseException as original:
        cleanup, completed = None, not popen_called
        try:
            if child is not None:
                if expected is not None:
                    linux_stop(kernel, expected)
                elif child.poll() is None:
                    # 尚未发布身份的自身子进程由 Popen 对象回收；不扫描进程表。
                    child.kill()
                child.wait(timeout=10)
                completed = True
        except BaseException as error:
            cleanup = type(error).__name__
        try:
            write(output / "linux-failure.json", {"error_type": type(original).__name__, "cleanup_error_type": cleanup,
                  "identity": expected, "request_sha256": payload["request_sha256"], "cleanup_completed": completed,
                  "child_pid": None if child is None else child.pid,
                  "child_returncode": None if child is None else child.returncode,
                  "popen_called": popen_called, "intent": bound(output / "linux-intent.json")})
        except BaseException as error:
            original.add_note("Redis Linux 失败收据写入失败：" + type(error).__name__)
        raise


def prepare(request, output):
    import full_stack_process
    validate(request)
    output = plain(output)
    helpers = output / "helpers"
    helpers.mkdir()
    frozen = {}
    for key, source in (("cache", Path(__file__)), ("process", Path(full_stack_process.__file__))):
        destination = helpers / ("cache.py" if key == "cache" else "full_stack_process.py")
        before = bound(source)
        with destination.open("xb") as stream:
            stream.write(source.read_bytes())
        if bound(source) != before or bound(destination)["sha256"] != before["sha256"]:
            raise ValueError("冻结 Redis 控制工具期间源码变化")
        frozen[key] = {**bound(destination), "path": linux_path(str(destination))}
    payload = {"format_version": 1, "request": request, "request_sha256": canonical(request), "windows_output": str(output),
               "linux_output": linux_path(str(output)), "helpers": frozen}
    write(output / "launch-request.json", payload)
    return payload


def command(request, output, action):
    payload = read(output / "launch-request.json")
    return [request["wsl"]["path"], "--distribution", request["distribution"], "--exec", request["python"]["path"],
            payload["helpers"]["cache"]["path"], action, "--request", linux_path(str(output / "launch-request.json")),
            "--sha256", bound(output / "launch-request.json")["sha256"]]


def frozen_payload(request, output):
    plain(output)
    payload = read(output / "launch-request.json")
    if payload["request"] != request or payload["request_sha256"] != canonical(request) or payload["windows_output"] != str(output):
        raise ValueError("Redis 冻结运行不属于明确请求及输出目录")
    for item in payload["helpers"].values():
        local = output / "helpers" / Path(item["path"]).name
        if linux_path(str(local)) != item["path"] or bound(plain(local)) != {**item, "path": str(local)}:
            raise ValueError("Redis 冻结辅助代码变化")
    if bound(plain(request["wsl"]["path"]))["sha256"] != request["wsl"]["sha256"]:
        raise ValueError("调用 WSL 的实际二进制变化")
    return payload


def inspect_start(request, output):
    output = Path(output)
    if not (output / "intent.json").exists():
        if any((output / name).exists() for name in ("launcher.json", "linux-intent.json", "linux-process.json", "runtime.json")):
            raise ValueError("Redis 进程证据缺少先行意图")
        return None
    try:
        if (output / "partial.json").exists():
            return partial_start(request, output)
        return recorded_start(request, output)
    except FileNotFoundError as error:
        raise ValueError("Redis 启动意图缺少完整创建身份，必须保留现场核实") from error


def partial_start(request, output):
    frozen_payload(request, output)
    proof = read(output / "partial.json")
    if read(output / "intent.json") != {"request_sha256": canonical(request), "arguments": command(request, output, "serve"),
                                       "launch": bound(output / "launch-request.json")}:
        raise ValueError("Redis 部分启动意图不属于原冻结请求")
    if (set(proof) != {"state", "request_sha256", "intent", "launcher_pid", "launcher_returncode", "authorization_sent", "linux_cleanup"}
            or proof["state"] not in {"not_started", "stopped"} or proof["request_sha256"] != canonical(request)
            or proof["intent"] != bound(output / "intent.json") or type(proof["launcher_pid"]) is not int
            or type(proof["launcher_returncode"]) is not int):
        raise ValueError("Redis 部分启动退出证据不完整")
    if proof["state"] == "not_started":
        if proof["authorization_sent"] is not False or proof["linux_cleanup"] is not None or (output / "linux-intent.json").exists():
            raise ValueError("Redis 已授权或已有 Linux 启动意图，不能宣称未启动")
    else:
        failure_path = file_bound(proof["linux_cleanup"])
        failure = read(failure_path)
        expected_intent = {**bound(output / "linux-intent.json"), "path": linux_path(str(output / "linux-intent.json"))}
        if (failure_path != output / "linux-failure.json" or failure.get("request_sha256") != canonical(request)
                or failure.get("cleanup_completed") is not True or failure.get("cleanup_error_type") is not None
                or failure.get("intent") != expected_intent):
            raise ValueError("Redis Linux 监督器未证明完整回收")
    return {"state": proof["state"], "request_sha256": canonical(request), "output": str(output),
            "launch": bound(output / "launch-request.json"), "intent": bound(output / "intent.json"),
            "partial_receipt": bound(output / "partial.json")}


def recorded_start(request, output):
    payload = frozen_payload(request, output)
    intent = read(output / "intent.json")
    if intent != {"request_sha256": canonical(request), "arguments": command(request, output, "serve"), "launch": bound(output / "launch-request.json")}:
        raise ValueError("WSL 启动意图与冻结请求不同")
    launcher = read(output / "launcher.json")
    identity = launcher.get("identity", {})
    if (set(launcher) != {"identity", "intent"} or set(identity) != {"pid", "started", "executable"}
            or type(identity["pid"]) is not int or identity["pid"] <= 1
            or not isinstance(identity["started"], str) or not identity["started"].isdigit()
            or launcher["intent"] != bound(output / "intent.json") or identity["executable"].lower() != request["wsl"]["path"].lower()):
        raise ValueError("WSL 启动进程缺少明确创建身份")
    linux = read(output / "linux-process.json")
    linux_intent = read(output / "linux-intent.json")
    expected_intent = {**bound(output / "linux-intent.json"), "path": linux_path(str(output / "linux-intent.json"))}
    if (linux.get("request_sha256") != canonical(request) or linux.get("intent") != expected_intent
            or linux_intent.get("request_sha256") != canonical(request)
            or linux_intent.get("arguments") != [request["launcher"], linux_path(request["configuration"]["path"])]) :
        raise ValueError("Redis Linux 身份不是同一启动意图")
    identity = linux["identity"]
    if (set(identity) != {"pid", "started", "executable", "boot_id"} or type(identity["pid"]) is not int or identity["pid"] <= 1
            or not isinstance(identity["started"], str) or not identity["started"].isdigit() or identity["executable"] != request["executable"]
            or not re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", identity["boot_id"])):
        raise ValueError("Redis Linux 内核身份字段不完整")
    return {"format_version": 1, "request_sha256": canonical(request), "output": str(output), "launch": bound(output / "launch-request.json"),
            "launcher": launcher["identity"], "linux_identity": identity, "boot_id": identity["boot_id"],
            "launcher_receipt": bound(output / "launcher.json"), "process_receipt": bound(output / "linux-process.json"),
            "helpers_sha256": canonical(payload["helpers"])}


def linux_call(request, runtime, action):
    output = Path(runtime["output"])
    observed = inspect_start(request, output)
    if observed is None or observed != {key: runtime[key] for key in observed}:
        raise ValueError("Redis 运行收据与实际创建身份不同")
    args = command(request, output, action)
    result = subprocess.run(args, input=(json.dumps(runtime["linux_identity"]) + "\n").encode(),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20,
                            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    if result.returncode != 0:
        raise RuntimeError("WSL Redis 精确身份操作失败")
    value = json.loads(result.stdout.decode("utf-8"))
    if (set(value) != {"identity", "alive", "boot_id", "terminated"} or value["identity"] != runtime["linux_identity"]
            or type(value["alive"]) is not bool or type(value["terminated"]) is not bool
            or not isinstance(value["boot_id"], str)
            or not re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", value["boot_id"])):
        raise ValueError("WSL 返回其他 Redis 进程身份")
    return value


def windows_identity(request, runtime, *, required):
    from full_stack_process import process_identity
    from devex_clone_target_storage import actual_windows_argv
    expected = runtime["launcher"]
    actual = process_identity(expected["pid"])
    if actual is None and not required:
        return False
    if actual != expected or actual_windows_argv(SimpleNamespace(runner=subprocess.run), expected) != command(request, Path(runtime["output"]), "serve"):
        raise ValueError("WSL launcher 创建身份或实际参数不符")
    return True


def observe(request, environment, runtime, output=None):
    validate(request)
    windows_identity(request, runtime, required=True)
    before = linux_call(request, runtime, "observe")
    if not before["alive"] or before["boot_id"] != runtime["boot_id"]:
        raise ValueError("Redis 原 Linux 运行代次已退出")
    facts = redis_facts(request, environment)
    check_facts(request, runtime["linux_identity"], facts, run_id=runtime.get("redis", {}).get("run_id"))
    if linux_call(request, runtime, "observe") != before:
        raise ValueError("Redis INFO 核验窗口内核代次变化")
    redis = {key: request[key] for key in ("wsl", "distribution", "port", "executable", "sha256", "configuration")}
    redis.update(pid=runtime["linux_identity"]["pid"], started=runtime["linux_identity"]["started"], run_id=facts["run_id"])
    result = {**runtime, "redis": redis, "facts": facts}
    if output is not None:
        write(Path(output) / "observed.json", result)
    return result


def status(request, runtime):
    """只读精确状态；未知身份或仍被占用的端口不能作为已停止证明。"""
    from devex_clone_source_proof import require_closed_port
    if "partial_receipt" in runtime:
        from full_stack_process import process_identity
        if inspect_start(request, runtime["output"]) != runtime:
            raise ValueError("Redis 部分退出证据变化")
        proof = read(Path(runtime["output"]) / "partial.json")
        if process_identity(proof["launcher_pid"]) is not None:
            raise ValueError("Redis 部分启动 launcher PID 当前被占用")
        result = subprocess.run(command(request, Path(runtime["output"]), "partial"),
                                input=(json.dumps({"partial_sha256": runtime["partial_receipt"]["sha256"]}) + "\n").encode(),
                                stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20,
                                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        if result.returncode != 0 or json.loads(result.stdout) != {"state": "stopped", "partial_sha256": runtime["partial_receipt"]["sha256"]}:
            raise ValueError("Redis 部分退出的 WSL 端口未闭合")
        require_closed_port(f"http://127.0.0.1:{request['port']}")
        return {"state": "stopped", "partial": runtime["state"], "launcher_alive": False}
    launcher_alive = windows_identity(request, runtime, required=False)
    linux = linux_call(request, runtime, "observe")
    if linux["alive"]:
        if linux["boot_id"] != runtime["boot_id"]:
            raise ValueError("Redis 存活状态与明确 boot 不符")
        return {"state": "running", "launcher_alive": launcher_alive, "linux": linux}
    if launcher_alive:
        raise ValueError("Redis 已退出但 WSL launcher 尚在运行，不能认定全部停止")
    require_closed_port(f"http://127.0.0.1:{request['port']}")
    return {"state": "stopped", "launcher_alive": False, "linux": linux}


def stop(request, environment, runtime, output):
    from devex_clone_source_proof import require_closed_port
    from full_stack_process import process_identity
    if "partial_receipt" in runtime:
        result = {**status(request, runtime), "status": "redis_process_stopped", "runtime": runtime, "resources_deleted": False}
        write(Path(output) / "stopped.json", result)
        return result
    windows_identity(request, runtime, required=False)
    result = linux_call(request, runtime, "stop")
    if result["alive"]:
        raise ValueError("Redis 精确回收后仍在运行")
    deadline = time.monotonic() + 10
    while process_identity(runtime["launcher"]["pid"]) is not None:
        if process_identity(runtime["launcher"]["pid"]) != runtime["launcher"] or time.monotonic() >= deadline:
            raise ValueError("Redis 退出后 WSL launcher 尚未退出或 PID 已复用")
        time.sleep(0.05)
    require_closed_port(f"http://127.0.0.1:{request['port']}")
    result = {**result, "status": "redis_process_stopped", "runtime": runtime, "resources_deleted": False}
    write(Path(output) / "stopped.json", result)
    return result


def start(request, environment, output, guard):
    from devex_clone_source_proof import require_closed_port
    from full_stack_process import process_identity
    from devex_clone_target_storage import actual_windows_argv
    output = Path(output)
    prepare(request, output)
    guard()
    require_closed_port(f"http://127.0.0.1:{request['port']}")
    args = command(request, output, "serve")
    write(output / "intent.json", {"request_sha256": canonical(request), "arguments": args, "launch": bound(output / "launch-request.json")})
    child, trusted, runtime, authorization_sent = None, None, None, False
    try:
        with (output / "wsl.stdout.log").open("xb") as stdout, (output / "wsl.stderr.log").open("xb") as stderr:
            child = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr, cwd=output,
                                     env=environment,
                                     creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        actual = process_identity(child.pid)
        if actual is None or actual["executable"].lower() != request["wsl"]["path"].lower():
            raise ValueError("WSL launcher 未通过实际产物身份核验")
        trusted = actual
        write(output / "launcher.json", {"identity": trusted, "intent": bound(output / "intent.json")})
        if actual_windows_argv(SimpleNamespace(runner=subprocess.run), trusted) != args:
            raise ValueError("WSL launcher 实际参数不符")
        authorization_sent = True
        child.stdin.write((json.dumps({"request_sha256": canonical(request), "output": str(output)}) + "\n").encode())
        child.stdin.close()
        deadline = time.monotonic() + request["timeout_seconds"]
        while not (output / "linux-process.json").exists():
            if child.poll() is not None or time.monotonic() >= deadline:
                raise TimeoutError("WSL 未在期限内登记 Redis 创建身份")
            time.sleep(0.05)
        runtime = inspect_start(request, output)
        while True:
            try:
                runtime = observe(request, environment, runtime)
                break
            except ConnectionRefusedError:
                if child.poll() is not None or time.monotonic() >= deadline:
                    raise TimeoutError("Redis 未在期限内完成认证就绪") from None
                time.sleep(0.1)
        guard()
        write(output / "acknowledged.json", {"request_sha256": canonical(request), "identity": runtime["linux_identity"]})
        while not (output / "accepted.json").exists():
            if child.poll() is not None or time.monotonic() >= deadline:
                raise TimeoutError("Linux supervisor 未确认本次运行")
            time.sleep(0.05)
        if read(output / "accepted.json") != read(output / "acknowledged.json"):
            raise ValueError("Linux supervisor 运行确认不同")
        write(output / "runtime.json", runtime)
        return runtime
    except BaseException as original:
        try:
            if child is not None and child.stdin is not None and not child.stdin.closed:
                child.stdin.close()  # 未授权时 EOF 确保 Linux 不会启动 Redis。
            if runtime is None and (output / "linux-process.json").exists():
                runtime = inspect_start(request, output)
            if runtime is not None:
                cleanup = output / "failure-stop"
                cleanup.mkdir()
                stop(request, environment, runtime, cleanup)
            elif child is not None:
                # 不单独杀 WSL 包装进程；监督器收到 EOF 或确认超时后会回收自身子进程。
                child.wait(timeout=request["timeout_seconds"] + 20)
                failure = output / "linux-failure.json"
                state = "not_started" if not authorization_sent and not (output / "linux-intent.json").exists() else "stopped"
                if state == "not_started" or failure.exists() and read(failure).get("cleanup_completed") is True:
                    write(output / "partial.json", {"state": state, "request_sha256": canonical(request),
                          "intent": bound(output / "intent.json"), "launcher_pid": child.pid, "launcher_returncode": child.returncode,
                          "authorization_sent": authorization_sent, "linux_cleanup": None if state == "not_started" else bound(failure)})
                    partial_start(request, output)
        except BaseException as error:
            original.add_note("Redis 精确回收未完成：" + type(error).__name__)
        try:
            write(output / "failure.json", {"error_type": type(original).__name__, "launcher": trusted,
                                          "request_sha256": canonical(request), "runtime_recorded": runtime is not None})
        except BaseException as error:
            original.add_note("Redis 失败收据写入失败：" + type(error).__name__)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("serve", "observe", "stop", "partial"))
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--sha256", required=True)
    args = parser.parse_args()
    if bound(plain(args.request))["sha256"] != args.sha256:
        raise ValueError("Redis Linux 控制请求文件摘要不同")
    payload = read(args.request)
    if args.action == "serve":
        linux_serve(payload)
    elif args.action == "partial":
        output, _ = linux_inputs(payload, startup=False)
        expected = json.loads(sys.stdin.readline(16384))
        if expected != {"partial_sha256": bound(output / "partial.json")["sha256"]}:
            raise ValueError("Redis 部分退出收据摘要不同")
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
            probe.bind(("127.0.0.1", payload["request"]["port"]))
        print(json.dumps({"state": "stopped", **expected}))
    else:
        expected = json.loads(sys.stdin.readline(16384))
        print(json.dumps(linux_check(payload, expected, stop=args.action == "stop")))


if __name__ == "__main__":
    try:
        main()
    except BaseException as error:
        print(json.dumps({"error_type": type(error).__name__}), file=sys.stderr)
        sys.exit(1)
