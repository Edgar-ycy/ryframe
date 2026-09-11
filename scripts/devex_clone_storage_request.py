"""同一复制运行的 RustFS 重启请求；只绑定原资源，不创建或转换资源。"""
from __future__ import annotations

import copy
from pathlib import Path
from urllib.parse import urlsplit

from devex_clone_capture import read_json
from devex_clone_model import exact, local_path, name
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file, producer_identities, require_closed_port
from full_stack_process import process_identity
from process_sockets import endpoint
from restore_reference_plan import plan_hash

FIELDS = {"format_version", "kind", "side", "manifest", "original", "environment", "scope_id", "executable",
          "data_directory", "api_url", "console_url", "credential_files", "previous", "timeout_seconds"}


def directory_identity(backend: Path, value: dict) -> Path:
    exact(value, {"path", "device", "inode"})
    path = local_path(backend, value["path"])
    if (not path.is_dir() or any(type(value[key]) is not int for key in ("device", "inode"))
            or value["inode"] <= 0 or (path.stat().st_dev, path.stat().st_ino) != (value["device"], value["inode"])):
        raise ValueError("存储数据目录的实际文件身份变化")
    return path


def identity(value: dict) -> dict:
    exact(value, {"pid", "started", "executable"})
    if (type(value["pid"]) is not int or value["pid"] <= 1 or not isinstance(value["started"], str)
            or not value["started"].isdigit() or not Path(value["executable"]).is_absolute()):
        raise ValueError("存储进程须绑定完整内核创建身份")
    return value


def arguments(request: dict) -> list[str]:
    urls = [urlsplit(request[key]) for key in ("api_url", "console_url")]
    return [request["executable"]["path"], "server", "--address", f"{urls[0].hostname}:{urls[0].port}",
            "--console-address", f"{urls[1].hostname}:{urls[1].port}", request["data_directory"]["path"]]


def configuration(request: dict) -> dict:
    return {"RUSTFS_CONSOLE_ENABLE": "true", **{"RUSTFS_" + key.upper() + "_FILE": item["path"]
            for key, item in request["credential_files"].items()}}


def original_inputs(backend: Path, value: dict, request: dict) -> dict:
    side = request["side"]
    original = read_json(bound_file(backend, request["original"]))
    previous = request["previous"]
    process = read_json(bound_file(backend, previous["process_receipt"]))
    launch = read_json(bound_file(backend, previous["launch_receipt"]))
    ready = read_json(bound_file(backend, previous["ready_receipt"]))
    common = {"identity": previous["identity"], "scope_id": request["scope_id"], "data_dir": request["data_directory"]["path"]}
    if any(process.get(key) != item for key, item in common.items()):
        raise ValueError("原存储进程不属于明确 scope、数据目录及创建代次")
    if side == "source":
        selected = original["source"]
        if (ready.get("status") != "ready" or ready.get("identity") != previous["identity"]
                or ready.get("process_receipt") != previous["process_receipt"]
                or ready.get("data_dir") != common["data_dir"]
                or ready.get("listeners") != [request["api_url"], request["console_url"]]
                or process.get("launch_plan") != previous["launch_receipt"]
                or process.get("command") != arguments(request)
                or launch.get("executable") != request["executable"]
                or launch.get("data_dir") != common["data_dir"] or launch.get("scope_id") != request["scope_id"]):
            raise ValueError("来源服务缺少同一原始启动计划和已完成就绪证据")
        for key in ("api_url", "console_url"):
            if launch.get(key) != request[key]:
                raise ValueError("来源存储端口不属于原启动计划")
        for key in ("access_key", "secret_key"):
            if (launch.get(key + "_file") != request["credential_files"][key]["path"]
                    or process.get("secret_files", {}).get(key + "_file") != request["credential_files"][key]):
                raise ValueError("来源凭据不是原实际启动文件")
    else:
        # 此处只读取原始输入绑定；完整初始化历史仍由复制/目标入口原有门禁验证。
        root = Path(value["initialized"]["path"]).parent
        prepared = read_json(root / "prepare.json")
        if original.get("status") != "fresh_target_initialized" or original.get("prepare_sha256") != binding(root / "prepare.json")["sha256"]:
            raise ValueError("目标原始初始化发布与准备记录不同")
        target = read_json(bound_file(backend, prepared["request"]))
        if prepared.get("request_sha256") != plan_hash(target) or read_json(root / "request.json") != target:
            raise ValueError("目标原始初始化请求不同")
        review = read_json(bound_file(backend, {key: item for key, item in target["review"].items() if key != "canonical_sha256"}))
        if plan_hash(review) != target["review"]["canonical_sha256"]:
            raise ValueError("目标原始审阅计划摘要不同")
        selected = target["target"]
        old = target["storage"]["rustfs"]
        expected = review["services"]["rustfs"]
        if (old != {"identity": previous["identity"], "sha256": request["executable"]["sha256"],
                    "process_receipt": previous["process_receipt"], "launch_receipt": previous["launch_receipt"]}
                or previous["ready_receipt"] != previous["process_receipt"] or process.get("lifecycle") != "running"
                or expected["scope_id"] != request["scope_id"] or expected["data_dir"] != common["data_dir"]
                or expected["api"] != request["api_url"] or expected["console"] != request["console_url"]
                or launch.get("identity") != previous["identity"] or launch.get("arguments") != arguments(request)
                or launch.get("credential_files") != request["credential_files"] or launch.get("environment") != configuration(request)):
            raise ValueError("目标重启不是原初始化中完整相同存储的代次过渡")
    if selected["s3"]["endpoint"] != request["api_url"]:
        raise ValueError("存储端口不是原业务侧对象端点")
    return selected


def validate_request(backend: Path, directory: Path, value: dict, request: dict, side: str) -> dict:
    exact(request, FIELDS)
    if (request["format_version"] != 1 or request["kind"] != "devex-clone-storage-restart"
            or side not in {"source", "target"} or request["side"] != side
            or request["manifest"] != binding(directory / "manifest.json")
            or request["original"] != value["source_request" if side == "source" else "initialized"]
            or request["environment"] != value[side + "_environment"]):
        raise ValueError("存储重启必须属于同一复制运行和原始侧输入")
    name(request["scope_id"])
    directory_identity(backend, request["data_directory"])
    executable = bound_file(backend, request["executable"])
    exact(request["previous"], {"identity", "process_receipt", "launch_receipt", "ready_receipt"})
    if Path(identity(request["previous"]["identity"])["executable"]) != executable:
        raise ValueError("存储重启不能替换原可执行文件")
    for key in ("api_url", "console_url"):
        endpoint(request[key])
        url = urlsplit(request[key])
        if url.scheme != "http" or url.hostname != "127.0.0.1" or url.path or url.query or url.fragment:
            raise ValueError("RustFS 仅允许两个明确本机 HTTP 根端口")
    if request["api_url"] == request["console_url"] or type(request["timeout_seconds"]) is not int or not 1 <= request["timeout_seconds"] <= 180:
        raise ValueError("存储监听端口或就绪期限无效")
    exact(request["credential_files"], {"access_key", "secret_key"})
    selected = original_inputs(backend, value, request)
    private = read_json(bound_file(backend, request["environment"]))
    exact(private, {"environment"})
    if not isinstance(private["environment"], dict) or any(type(k) is not str or type(v) is not str for k, v in private["environment"].items()):
        raise ValueError("存储环境必须是明确文本映射")
    for key, descriptor in request["credential_files"].items():
        contents = bound_file(backend, descriptor).read_text(encoding="utf-8").strip()
        if not contents or contents != private["environment"].get(selected["s3"][key + "_env"]):
            raise ValueError("存储凭据文件与原业务认证不符")
    return copy.deepcopy(private["environment"])


def producers_stopped(backend: Path, directory: Path, value: dict) -> None:
    if any((directory / name).exists() for name in ("post-copy.json", "seed-runtime.json")):
        from devex_clone_run_state import load_state
        from devex_clone_seed_rebind import published_restart_guard

        attempts = load_state(directory)["attempts"]
        if not attempts:
            raise ValueError("已发布 seed 存储重启缺少当前持锁阶段")
        attempt = attempts[-1]
        if (attempt["stage"], attempt["mode"], attempt["status"]) != ("storage-target", "restart", "running"):
            raise ValueError("已发布 seed 仅允许当前持锁的 target 存储重启")
        published_restart_guard(backend, directory, attempt["number"])
        return
    source = read_json(bound_file(backend, value["source_request"]))
    processes = {role: read_json(bound_file(backend, item))["identity"] for role, item in source["processes"].items()}
    # 此启动证明没有外部查询 runner；权限不足时仍只接受原生身份结果。
    producer_identities(source, read_json(bound_file(backend, source["producers_registry"])), processes, run=None)
    runtime = read_json(bound_file(backend, source["runtime"]))
    for url in (source["source"]["api_url"], runtime["worker_ready_url"]):
        require_closed_port(url)
    from devex_clone_factory_context import initialization_history
    from devex_clone_target_binding import request_binding

    _, target = initialization_history(backend, bound_file(backend, value["initialized"]))
    _, selected = request_binding(backend, target)
    runtime = local_path(backend, selected["runtime_dir"])
    for role in ("api", "worker"):
        path = runtime / (role + ".json")
        if path.exists():
            process = read_json(path)
            if process.get("scope_id") != target["target"]["scope_id"] or process.get("role") != role:
                raise ValueError("目标运行收据角色或 scope 变化")
            if process_identity(identity(process["identity"])["pid"]) is not None:
                raise ValueError("目标业务生产者仍在运行或身份不明")
    for url in (selected["api_url"], selected["worker_ready_url"]):
        require_closed_port(url)
