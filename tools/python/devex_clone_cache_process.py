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
from subprocess import Popen as launch_process
import sys
import time
import uuid


FIELDS = {"scope_id", "wsl", "distribution", "launcher", "executable", "sha256", "configuration", "directory",
          "previous_identity", "previous_boot_id", "previous_run_id", "port", "password", "timeout_seconds", "python"}


def validate_password(value):
    from devex_clone_cache_native import invoke
    invoke("password", value=value)


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
    from devex_clone_cache_native import invoke
    invoke("validate", request=request, files=files)
    if files:
        directory = plain(request["directory"]["path"])
        identity = request["directory"]
        if (not directory.is_dir() or type(identity["device"]) is not int
                or type(identity["inode"]) is not int
                or (directory.stat().st_dev, directory.stat().st_ino) != (identity["device"], identity["inode"])):
            raise ValueError("Redis 原 Windows 数据目录身份变化")


def configuration(request):
    return {"bind": "127.0.0.1", "port": str(request["port"]), "dir": linux_path(request["directory"]["path"]),
            "databases": "1", "protected-mode": "yes", "save": "", "appendonly": "no", "daemonize": "no"}


def redis_facts(request, environment):
    from devex_clone_cache_native import invoke
    return invoke("facts", request=request)


def check_facts(request, linux_identity, facts, *, run_id=None):
    from devex_clone_cache_native import invoke
    invoke("check-facts", request=request, identity=linux_identity, facts=facts, run_id=run_id)


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
    return linux_process_check(payload, expected, output, kernel, stop=stop)


def linux_process_check(payload, expected, output, kernel, *, stop=False):
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
    if request["previous_identity"] is not None:
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
            child = launch_process(args, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr,
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


def inspect_start(request, output):
    from devex_clone_cache_native import invoke
    return invoke("inspect", request=request, output=str(output))


def linux_call(request, runtime, action):
    from devex_clone_cache_native import invoke
    validate(request)
    return invoke("call", request=request, runtime=runtime, action=action)


def windows_identity(backend, request, runtime, *, required):
    from full_stack_process import process_identity
    from devex_clone_target_evidence import evidence
    expected = runtime["launcher"]
    actual = process_identity(expected["pid"])
    if actual is None and not required:
        return False
    directory = Path(runtime["output"])
    if actual != expected or evidence("process-argv", backend, directory, receipt=bound(directory / "launcher.json"),
                                      identity=expected)["actual_arguments_sha256"] != canonical(command(request, directory, "serve")):
        raise ValueError("WSL launcher 创建身份或实际参数不符")
    return True


def observe(backend, request, environment, runtime, output=None):
    validate(request)
    windows_identity(backend, request, runtime, required=True)
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


def stop(backend, request, environment, runtime, output):
    from devex_clone_source_proof import require_closed_port
    from full_stack_process import process_identity
    validate(request)
    if "partial_receipt" in runtime:
        result = {**status(request, runtime), "status": "redis_process_stopped", "runtime": runtime, "resources_deleted": False}
        write(Path(output) / "stopped.json", result)
        return result
    windows_identity(backend, request, runtime, required=False)
    observed_after_failed_stop = False
    try:
        result = linux_call(request, runtime, "stop")
    except RuntimeError as error:
        # 精确停止调用失败时用同一冻结身份做一次只读复核：记录中的 Linux 进程已经不
        # 存在且端口空闲时，停止目标已经达成；仍在运行或复核失败则保留原始错误。
        try:
            observation = linux_call(request, runtime, "check")
        except BaseException:
            raise error
        if observation["alive"]:
            raise error
        result = observation
        observed_after_failed_stop = True
    if result["alive"]:
        raise ValueError("Redis 精确回收后仍在运行")
    deadline = time.monotonic() + 10
    while process_identity(runtime["launcher"]["pid"]) is not None:
        if process_identity(runtime["launcher"]["pid"]) != runtime["launcher"] or time.monotonic() >= deadline:
            raise ValueError("Redis 退出后 WSL launcher 尚未退出或 PID 已复用")
        time.sleep(0.05)
    require_closed_port(f"http://127.0.0.1:{request['port']}")
    result = {**result, "status": "redis_process_stopped", "runtime": runtime, "resources_deleted": False}
    if observed_after_failed_stop:
        result["observed_after_failed_stop"] = True
    write(Path(output) / "stopped.json", result)
    return result


def start(backend, request, environment, output, guard):
    from devex_clone_source_proof import require_closed_port
    from full_stack_process import process_identity
    from devex_clone_target_evidence import evidence
    output = Path(output)
    from devex_clone_cache_native import invoke
    private = invoke("environment-select", private={"environment": environment})
    host_names = {"PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC", "USERPROFILE", "HOME",
                  "HOMEDRIVE", "HOMEPATH", "TEMP", "TMP"}
    private_names = {key.upper() for key in private}
    child_environment = {key: value for key, value in os.environ.items()
                         if key.upper() in host_names and key.upper() not in private_names}
    child_environment.update(private)
    prepare(request, output)
    guard()
    require_closed_port(f"http://127.0.0.1:{request['port']}")
    args = command(request, output, "serve")
    write(output / "intent.json", {"request_sha256": canonical(request), "arguments": args, "launch": bound(output / "launch-request.json")})
    child, trusted, runtime, authorization_sent = None, None, None, False
    try:
        with (output / "wsl.stdout.log").open("xb") as stdout, (output / "wsl.stderr.log").open("xb") as stderr:
            child = launch_process(args, stdin=subprocess.PIPE, stdout=stdout, stderr=stderr, cwd=output,
                                     env=child_environment,
                                     creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        actual = process_identity(child.pid)
        if actual is None or actual["executable"].lower() != request["wsl"]["path"].lower():
            raise ValueError("WSL launcher 未通过实际产物身份核验")
        trusted = actual
        write(output / "launcher.json", {"identity": trusted, "intent": bound(output / "intent.json")})
        if evidence("process-argv", backend, output, receipt=bound(output / "launcher.json"),
                    identity=trusted)["actual_arguments_sha256"] != canonical(args):
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
                runtime = observe(backend, request, environment, runtime)
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
                stop(backend, request, environment, runtime, cleanup)
            elif child is not None:
                # 不单独杀 WSL 包装进程；监督器收到 EOF 或确认超时后会回收自身子进程。
                child.wait(timeout=request["timeout_seconds"] + 20)
                failure = output / "linux-failure.json"
                state = "not_started" if not authorization_sent and not (output / "linux-intent.json").exists() else "stopped"
                if state == "not_started" or failure.exists() and read(failure).get("cleanup_completed") is True:
                    write(output / "partial.json", {"state": state, "request_sha256": canonical(request),
                          "intent": bound(output / "intent.json"), "launcher_pid": child.pid, "launcher_returncode": child.returncode,
                          "authorization_sent": authorization_sent, "linux_cleanup": None if state == "not_started" else bound(failure)})
                    inspect_start(request, output)
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
    parser.add_argument("action", choices=("serve", "observe", "check", "stop"))
    parser.add_argument("--request", type=Path, required=True)
    parser.add_argument("--sha256", required=True)
    args = parser.parse_args()
    if bound(plain(args.request))["sha256"] != args.sha256:
        raise ValueError("Redis Linux 控制请求文件摘要不同")
    payload = read(args.request)
    if args.action == "serve":
        linux_serve(payload)
    else:
        expected = json.loads(sys.stdin.readline(16384))
        print(json.dumps(linux_check(payload, expected, stop=args.action == "stop")))


if __name__ == "__main__":
    try:
        main()
    except BaseException as error:
        print(json.dumps({"error_type": type(error).__name__}), file=sys.stderr)
        sys.exit(1)
