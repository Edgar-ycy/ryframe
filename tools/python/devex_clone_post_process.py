"""复制后处理原生协议的最小 Python 调用适配。"""
import json
import os
from pathlib import Path
import subprocess
import uuid

from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding, load_state
from full_stack_process import process_identity

KINDS = {"session": "xtask/src/recovery/clone/native/session.rs",
         "existing": "xtask/src/recovery/dataset_prepare/existing_native.rs",
         "identity-apply": "xtask/src/data/performance_identities/producer.rs",
         "identity-verify": "xtask/src/data/performance_identities/producer.rs",
         "quota-plan": "xtask/src/data/performance_identities/clone_bridge.rs",
         "quota-apply": "xtask/src/data/performance_identities/clone_bridge.rs",
         "quota-reconcile": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-plan": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-apply": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-reconcile": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-verify": "xtask/src/data/performance_identities/clone_bridge.rs"}
PHASES = {"session": ("post-copy", {"schedules", "reconcile"}),
          "existing": ("post-copy", {"verify"}),
          "identity-apply": ("seed-runtime", {"identities-apply"}),
          "identity-verify": ("seed-runtime", {"identities-verify"}),
          "quota-plan": ("seed-runtime", {"quotas-plan"}),
          "quota-apply": ("seed-runtime", {"quotas-apply"}),
          "quota-reconcile": ("seed-runtime", {"quotas-reconcile"}),
          "department-plan": ("seed-runtime", {"departments-plan"}),
          "department-apply": ("seed-runtime", {"departments-apply"}),
          "department-reconcile": ("seed-runtime", {"departments-reconcile"}),
          "department-verify": ("seed-runtime", {"departments-verify"})}
STAGES = {"post-copy", "seed-runtime"}
_TEST_TOOL = None


def install_test_tool(value):
    """测试夹具显式绑定本次构建；产品调用仍只接受协调器绑定。"""
    global _TEST_TOOL
    if _TEST_TOOL is not None:
        raise ValueError("后处理测试工具已经绑定")
    _TEST_TOOL = dict(value)


def _native(payload, operation):
    from rate_limit_tool import tool_binding

    tool = tool_binding() or _TEST_TOOL
    if tool is None:
        raise ValueError("clone 后处理缺少当前 xtask 工具绑定")
    content = json.dumps({"operation": operation, "payload": payload},
                         ensure_ascii=False, separators=(",", ":")).encode()
    backend = next(path for path in Path(__file__).resolve().parents
                   if (path / "Cargo.toml").is_file())
    result = subprocess.run([tool["path"], "__clone-post-process"], cwd=backend,
        env={"APP_ENV": os.environ.get("APP_ENV", "test"), "PATH": os.environ.get("PATH", "")},
        input=content, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, timeout=60,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    try:
        response = json.loads(result.stdout)
    except Exception as error:
        raise ValueError("clone 后处理原生协议没有返回有效 JSON") from error
    if result.returncode or response.get("ok") is not True:
        raise ValueError("clone 后处理原生协议失败：" + str(response.get("error_type", "PostProcessError")))
    return response["result"]


def producer_registration(backend, directory, stage, *, cleanup=False):
    if stage == "post-copy":
        from devex_clone_post import registration
        active = registration(backend, directory, cleanup=cleanup)
        return active.request, active.descriptor
    if stage == "seed-runtime":
        from devex_clone_seed import registered
        return registered(backend, directory, cleanup=cleanup), binding(directory / "seed-runtime.json")
    raise ValueError("原生生产者阶段未登记")


def registered_producer(backend, directory, stage, *, cleanup=False):
    return producer_registration(backend, directory, stage, cleanup=cleanup)[0]


def historical_producer_request(backend, directory, stage, descriptor):
    if stage == "post-copy":
        from devex_clone_post import registration
        return registration(backend, directory, descriptor=descriptor, cleanup=True).request
    return registered_producer(backend, directory, stage, cleanup=True)


def cleanup_failure(context, label, error, original):
    try:
        _native({"backend": str(context.backend), "output": str(context.output),
                 "label": label, "error_type": type(error).__name__},
                "record-cleanup-failure")
    except Exception as recording_error:
        original.add_note("清理诊断未保存：" + type(recording_error).__name__)


def producer_tool(request, kind):
    if kind in {"session", "existing"}:
        from rate_limit_tool import tool_binding
        value = tool_binding()
        if value is None:
            raise ValueError("原生生产者缺少协调器工具绑定")
        return {key: value[key] for key in ("path", "sha256")}
    return request["identity_tool"]


def producer_source(backend, kind):
    return Path(backend) / KINDS[kind]


def _command_payload(backend, request, directory, number, kind, output, tool, configuration):
    return {"backend": str(backend), "run_dir": str(directory), "attempt": number,
            "kind": kind, "output": str(output), "tool": tool, "registration": request,
            "configuration": None if configuration is None else str(configuration)}


def producer_command(backend, request, directory, number, kind, output, *, tool_override=None,
                     configuration=None):
    tool = producer_tool(request, kind) if tool_override is None else tool_override
    return _native(_command_payload(backend, request, directory, number, kind, output,
                                   tool, configuration), "command")["argv"]


def inspect_producer(backend, directory, attempt):
    request, descriptor = producer_registration(backend, directory, attempt["stage"], cleanup=True)
    return _native({"backend": str(backend), "run_dir": str(directory), "attempt": attempt,
                    "registration": request, "registration_binding": descriptor}, "inspect")


def require_quiet(backend, directory, current_number=None):
    registrations = {}
    for attempt in load_state(directory, verify_results=False)["attempts"]:
        if attempt["stage"] in STAGES and attempt["number"] != current_number:
            request, descriptor = producer_registration(backend, directory, attempt["stage"], cleanup=True)
            registrations[str(attempt["number"])] = {"request": request, "binding": descriptor}
    _native({"backend": str(backend), "run_dir": str(directory),
             "current_attempt": current_number, "registrations": registrations}, "quiet")


def recover_identity_lock(backend, request, identity, owner_token):
    result = _native({"backend": str(backend), "identity_state": request["identity_state"],
        "identity": identity, "owner_token": owner_token,
        "plan_sha256": request["identity_plan"]["sha256"]}, "recover-identity-lock")
    return result["released"]


def recover_session(backend, directory, number, descriptor):
    state = load_state(directory, verify_results=False)
    current = state["attempts"][-1]
    candidates = [item for item in state["attempts"] if item["number"] < number
                  and item["stage"] == current["stage"] and
                  directory / item["stage"] / f"attempt-{item['number']:04d}" / "session-process.json"
                  == Path(descriptor["path"])]
    if len(candidates) != 1:
        raise ValueError("只能恢复明确绑定的历史阶段生产者")
    attempt = candidates[0]
    request, registration_binding = producer_registration(backend, directory, attempt["stage"], cleanup=True)
    process = read_json(Path(descriptor["path"]))
    identity_lock = None
    if attempt["mode"].startswith("identities-") and "identity_lock_owner" in process:
        identity_lock = {"backend": str(backend), "identity_state": request["identity_state"],
            "identity": process["identity"], "owner_token": process["identity_lock_owner"],
            "plan_sha256": request["identity_plan"]["sha256"]}
    return _native({"backend": str(backend), "run_dir": str(directory),
        "current_attempt": number, "descriptor": descriptor, "attempt": attempt,
        "registration": request, "registration_binding": registration_binding,
        "identity_lock": identity_lock, "controller_identity": process_identity(os.getpid())},
        "recover-session")


class Producer:
    def __init__(self, context, kind):
        self.context, self.child, self.log, self.identity = context, None, None, None
        self.identity_lock_owner = str(uuid.uuid4()) if kind.startswith("identity-") else None
        self.producer_owner = str(uuid.uuid4()) if kind != "existing" and self.identity_lock_owner is None else None
        self.owner = self.identity_lock_owner or self.producer_owner
        number = int(context.output.name.removeprefix("attempt-"))
        require_quiet(context.backend, context.directory_root, number)
        tool, configuration = producer_tool(context.request, kind), None
        if kind == "existing":
            from restore_reference_plan import configuration_binding
            contract = read_json(context.runtime / "runtime.json")
            configuration = context.output / "existing-configuration.json"
            write_json(configuration, configuration_binding(
                context.backend, Path(contract["api_config_dir"]), "test"))
        prepared = _native({**_command_payload(context.backend, context.request,
            context.directory_root, number, kind, context.output, tool, configuration),
            "registration_binding": context.request_binding, "owner": self.owner,
            "controller_identity": process_identity(os.getpid())}, "prepare-launch")
        self.log = (context.output / "session.stderr.log").open("xb")
        try:
            self.child = subprocess.Popen(prepared["command"], cwd=context.backend,
                env={"APP_ENV": "test", "PATH": os.environ.get("PATH", "")},
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            self.identity = process_identity(self.child.pid)
            if self.identity is None:
                raise ValueError("原生生产者缺少内核身份")
            _native({"output": str(context.output), "identity": self.identity,
                     "kind": kind, "owner": self.owner}, "record-process")
        except BaseException as original:
            try:
                self.close()
            except Exception as cleanup_error:
                cleanup_failure(context, "session-cleanup", cleanup_error, original)
            raise

    def close(self):
        try:
            if self.child is not None:
                if self.child.poll() is None:
                    if self.identity is None:
                        raise ValueError("缺少原生创建身份，保留现场，不按 PID 终止")
                    _native({"identity": self.identity}, "terminate-process")
                self.child.wait(timeout=10)
        finally:
            if self.child is not None and self.child.poll() is not None:
                self.child.stdin.close()
                self.child.stdout.close()
            if self.log is not None:
                self.log.close()

    def communicate(self, timeout):
        number = int(self.context.output.name.removeprefix("attempt-"))
        release = {"operation": "start", "run_dir": str(self.context.directory_root),
                   "attempt": number}
        if self.owner is not None:
            release.update(launch=binding(self.context.output / "session-launch.json"),
                           process=binding(self.context.output / "session-process.json"))
            release.update({"identity_lock_owner" if self.identity_lock_owner else "producer_owner": self.owner})
        stdout, _ = self.child.communicate((json.dumps(release) + "\n").encode(), timeout=timeout)
        if self.child.returncode:
            raise ValueError("既有数据原生验证失败，保留独立日志")
        return stdout


def runtime_proof(request):
    return _native(request, "runtime-proof")


if __name__ == "__main__":
    raise SystemExit("该文件只提供固定 Rust stdin 协议适配，不可独立执行")
