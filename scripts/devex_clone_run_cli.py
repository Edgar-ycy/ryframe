"""统一开发复制入口的参数与编排；真实结果由阶段工具产生。"""
import json
from pathlib import Path
from types import SimpleNamespace

from devex_clone_capture import read_json
from devex_clone_model import local_path
from devex_clone_run import cleanup_inputs, execute, initialize, recover_copy, run_runtime, status
from devex_clone_run_state import recover_lock

COMMANDS = {"init", "status", "stage", "runtime", "recover", "recover-copy", "bridge", "post-copy", "seed-runtime", "storage", "cache", "maintenance"}
FRESH_TARGET_PROTOCOL_ENV = "RYFRAME_XTASK_RECOVERY_FRESH_TARGET"
FRESH_TARGET_PROTOCOL_KIND = "ryframe-xtask-recovery-fresh-target"
FRESH_TARGET_OPERATIONS = {
    "prepare", "resume-prepare", "initialize", "resume-initialize",
    "reconcile-preflight", "verify", "status",
}
SEED_SOURCE_PROTOCOL_ENV = "RYFRAME_XTASK_RECOVERY_SEED_SOURCE"
SEED_SOURCE_PROTOCOL_KIND = "ryframe-xtask-recovery-seed-source"
SEED_SOURCE_OPERATIONS = {
    "source-register", "source-rebind", "source-generation-start",
    "source-generation-stop", "source-generation-status",
    "source-generation-recover", "source-export", "source-export-reconcile",
}
SEED_SOURCE_REQUEST_OPERATIONS = {
    "source-rebind", "source-generation-start", "source-generation-stop",
    "source-generation-recover",
}


class FreshTargetProtocolError(ValueError):
    """表示 xtask 与私有 Python 阶段之间的协议输入无效。"""


class SeedSourceProtocolError(ValueError):
    """表示 xtask 与 seed 来源阶段之间的协议输入无效。"""


def _strict_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise FreshTargetProtocolError(f"fresh-target 私有协议字段重复：{key}")
        value[key] = item
    return value


def _exact_keys(value, expected, label):
    if not isinstance(value, dict) or set(value) != set(expected):
        raise FreshTargetProtocolError(f"fresh-target 私有协议 {label} 字段不匹配")


def _protocol_path(value, name, *, optional=False):
    if optional and value is None:
        return None
    if (not isinstance(value, str) or not value.strip()
            or any(character in value for character in ("\r", "\n", "\0"))):
        raise FreshTargetProtocolError(f"fresh-target 私有协议 {name} 必须是有效路径")
    path = Path(value)
    if not path.is_absolute():
        raise FreshTargetProtocolError(f"fresh-target 私有协议 {name} 必须是绝对路径")
    return path


def decode_fresh_target_protocol(source: str):
    """严格解码由 xtask 生成的单一版本化请求，不接收公开 argv。"""
    if not isinstance(source, str) or not source or "\0" in source:
        raise FreshTargetProtocolError("fresh-target 私有协议为空或包含 NUL")
    try:
        value = json.loads(source, object_pairs_hook=_strict_object)
    except (json.JSONDecodeError, TypeError) as error:
        raise FreshTargetProtocolError("fresh-target 私有协议不是有效 JSON") from error
    _exact_keys(value, {"format_version", "kind", "request"}, "根")
    if (type(value["format_version"]) is not int or value["format_version"] != 1
            or not isinstance(value["kind"], str)
            or value["kind"] != FRESH_TARGET_PROTOCOL_KIND):
        raise FreshTargetProtocolError("fresh-target 私有协议版本或类型不受支持")
    request = value["request"]
    fields = {
        "backend_dir", "operation", "workspace", "request", "environment",
        "storage_run", "observation_dir", "write",
    }
    _exact_keys(request, fields, "request")
    operation = request["operation"]
    if not isinstance(operation, str) or operation not in FRESH_TARGET_OPERATIONS:
        raise FreshTargetProtocolError("fresh-target 私有协议 operation 无效")
    if not isinstance(request["write"], bool):
        raise FreshTargetProtocolError("fresh-target 私有协议 write 必须是布尔值")
    backend = _protocol_path(request["backend_dir"], "backend_dir")
    arguments = SimpleNamespace(
        command="fresh-target",
        operation=operation,
        workspace=_protocol_path(request["workspace"], "workspace"),
        request=_protocol_path(request["request"], "request", optional=True),
        environment=_protocol_path(request["environment"], "environment", optional=True),
        storage_run=_protocol_path(request["storage_run"], "storage_run", optional=True),
        observation_dir=_protocol_path(
            request["observation_dir"], "observation_dir", optional=True),
        write=request["write"],
    )
    return backend, arguments


def _seed_strict_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise SeedSourceProtocolError(f"seed source 私有协议字段重复：{key}")
        value[key] = item
    return value


def _seed_exact_keys(value, expected, label):
    if not isinstance(value, dict) or set(value) != set(expected):
        raise SeedSourceProtocolError(f"seed source 私有协议 {label} 字段不匹配")


def _seed_protocol_path(value, name, *, optional=False):
    if optional and value is None:
        return None
    if (not isinstance(value, str) or not value.strip()
            or any(character in value for character in ("\r", "\n", "\0"))):
        raise SeedSourceProtocolError(f"seed source 私有协议 {name} 必须是有效路径")
    path = Path(value)
    if not path.is_absolute():
        raise SeedSourceProtocolError(f"seed source 私有协议 {name} 必须是绝对路径")
    return path


def decode_seed_source_protocol(source: str):
    """严格解码正式 seed 来源链请求；路径的最终 ownership 由 dispatch 再核验。"""
    if (not isinstance(source, str) or not source or len(source) > 32 * 1024
            or any(character in source for character in ("\r", "\n", "\0"))):
        raise SeedSourceProtocolError("seed source 私有协议为空、过长或包含换行/NUL")
    try:
        value = json.loads(source, object_pairs_hook=_seed_strict_object)
    except (json.JSONDecodeError, TypeError) as error:
        raise SeedSourceProtocolError("seed source 私有协议不是有效 JSON") from error
    _seed_exact_keys(value, {"format_version", "kind", "request"}, "根")
    if (type(value["format_version"]) is not int or value["format_version"] != 1
            or value["kind"] != SEED_SOURCE_PROTOCOL_KIND):
        raise SeedSourceProtocolError("seed source 私有协议版本或类型不受支持")
    request = value["request"]
    _seed_exact_keys(
        request,
        {"backend_dir", "operation", "run_dir", "request", "write"},
        "request",
    )
    operation = request["operation"]
    if not isinstance(operation, str) or operation not in SEED_SOURCE_OPERATIONS:
        raise SeedSourceProtocolError("seed source 私有协议 operation 无效")
    if not isinstance(request["write"], bool):
        raise SeedSourceProtocolError("seed source 私有协议 write 必须是布尔值")
    supplied = request["request"] is not None
    if supplied != (operation in SEED_SOURCE_REQUEST_OPERATIONS):
        raise SeedSourceProtocolError("seed source 私有协议 request 与操作不匹配")
    read_only = operation == "source-generation-status"
    if request["write"] == read_only:
        raise SeedSourceProtocolError("seed source 私有协议 write 与操作不匹配")
    backend = _seed_protocol_path(request["backend_dir"], "backend_dir")
    arguments = SimpleNamespace(
        command="seed-runtime",
        operation=operation,
        run_dir=_seed_protocol_path(request["run_dir"], "run_dir"),
        request=_seed_protocol_path(request["request"], "request", optional=True),
        producer_binding=None,
        write=request["write"],
    )
    return backend, arguments


def evidence_path(backend: Path, value: Path, *, new: bool = False) -> Path:
    """将用户相对路径限定到当前后端的受控证据根。"""
    requested = value if value.is_absolute() else backend / value
    return local_path(backend, str(requested), new=new)


def add_commands(commands) -> None:
    register = commands.add_parser("init", help="登记固定验收清单与运行目录，不连接服务")
    register.add_argument("--manifest", type=Path, required=True)
    check = commands.add_parser("status", help="只读查询各阶段、输入指纹和实际控制进程")
    stage = commands.add_parser("stage", help="只执行指定阶段，保留失败现场")
    stage.add_argument("--stage", choices=("export", "target-verify", "copy"), required=True)
    stage.add_argument("--mode", choices=("run", "reconcile", "resume"), default="run")
    runtime = commands.add_parser("runtime", help="核验和启停登记环境的 API、Worker")
    runtime.add_argument("--side", choices=("source", "target"), required=True)
    runtime.add_argument("--operation", choices=("start", "stop", "status", "recover"), required=True)
    runtime.add_argument("--roles", nargs="+", choices=("api", "worker"), default=["api", "worker"])
    runtime.add_argument("--write", action="store_true", help="启动或停止必须显式指定")
    storage = commands.add_parser("storage", help="同一物理目录显式重启 RustFS，保留原始初始化和旧进程收据")
    storage.add_argument("--side", choices=("source", "target"), required=True)
    storage.add_argument("--operation", choices=("restart", "status", "stop", "recover"), required=True)
    storage.add_argument("--request", type=Path, help="首次 restart 必须绑定原服务及同一数据目录")
    storage.add_argument("--write", action="store_true")
    cache = commands.add_parser("cache", help="恢复原目标的隔离 Redis 运行与归属标记，未知写入先核对")
    cache.add_argument("--operation", choices=("restart", "status", "stop", "recover", "reconcile", "resume"), required=True)
    cache.add_argument("--request", type=Path, help="首次 restart 必须固定绑定原 Redis 配置及隔离目标")
    cache.add_argument("--write", action="store_true")
    maintenance = commands.add_parser("maintenance", help="构建或只读核验开发复制维护工具收据")
    maintenance.add_argument("--operation", choices=("build", "verify"), required=True)
    maintenance.add_argument("--output", type=Path, required=True,
                             help="build 使用新的证据目录；verify 使用现有目录或其中的 build.json")
    maintenance.add_argument("--write", action="store_true", help="只有 build 需要显式指定")
    recover = commands.add_parser("recover", help="核实死亡控制进程后回收明确的本地锁，不恢复写入")
    recover.add_argument("--owner-binding", type=Path, required=True,
                         help="当前目录锁 owner 或明确 controller-NNNN 阶段持久收据的路径与摘要绑定")
    copy_recover = commands.add_parser("recover-copy", help="按实际持有人收据回收复制锁，之后仍须 reconcile")
    copy_recover.add_argument("--owner-binding", type=Path, required=True)
    bridge = commands.add_parser("bridge", help="核验历史构建与产品输入，显式登记复用证据，不编译")
    bridge.add_argument("--build", type=Path, required=True)
    bridge.add_argument("--inventory", type=Path, required=True)
    bridge.add_argument("--output", type=Path, required=True)
    post = commands.add_parser("post-copy", help="同一 run 显式登记并继续 API 准备、调度处置与既有数据读取")
    post.add_argument("--operation", choices=("register", "amend", "prepare", "schedules", "reconcile", "verify", "recover-session"), required=True)
    post.add_argument("--request", type=Path, help="register 或 amend 必须提供完整且不可变的请求")
    post.add_argument("--producer-binding", type=Path, help="recover-session 必须明确原 Node 收据路径与摘要")
    seed = commands.add_parser("seed-runtime", help="seed 运行、冻结来源、一次导出与 arm 交接；失败候选显式核对")
    seed.add_argument("--operation", choices=("register", "quotas-plan", "quotas-apply", "quotas-reconcile",
                      "departments-plan", "departments-apply", "departments-reconcile", "departments-verify",
                      "identities-apply", "identities-verify", "prepare",
                      "start", "close", "source-register", "source-rebind", "source-generation-start", "source-generation-stop",
                      "source-generation-status", "source-generation-recover", "source-export", "source-export-reconcile",
                      "arm-input", "stop", "status", "recover",
                      "recover-session"), required=True)
    seed.add_argument("--request", type=Path)
    seed.add_argument("--producer-binding", type=Path)
    seed.add_argument("--write", action="store_true")
    for command in (register, check, stage, runtime, recover, copy_recover, post, seed, storage, cache):
        command.add_argument("--run-dir", type=Path, required=True)
    for command in (register, stage, recover, copy_recover, bridge, post):
        command.add_argument("--write", action="store_true", required=True)
    for command in (register, check, stage, runtime, recover, copy_recover, bridge, post, seed, storage, cache, maintenance):
        command.add_argument("--backend-dir", type=Path, required=True)


def dispatch(args, backend: Path) -> dict:
    if args.command == "maintenance":
        from devex_clone_tools import build, verify

        output = evidence_path(backend, args.output, new=args.operation == "build")
        if args.operation == "build":
            if not args.write:
                raise ValueError("maintenance build 需要显式 --write")
            build(backend, output)
            return {"status": "maintenance_build_created", "receipt": str(output / "build.json"),
                    "resources_modified": False, "restore_qualified": False}
        if args.write:
            raise ValueError("maintenance verify 是只读操作，不接受 --write")
        if output.is_dir():
            output = evidence_path(backend, output / "build.json")
        verify(backend, output)
        return {"status": "maintenance_build_verified", "receipt": str(output),
                "resources_modified": False, "restore_qualified": False}
    if args.command == "fresh-target":
        from devex_clone_target_cli import initialize as initialize_target, reconcile_preflight
        from devex_clone_target_cli import prepare, resume_initialize, resume_prepare, status as target_status, verify

        workspace = evidence_path(backend, args.workspace, new=args.operation == "prepare")
        supplied = (args.request is not None, args.environment is not None,
                    args.storage_run is not None, args.observation_dir is not None)
        expected = {"prepare": (True, True, True, False), "resume-prepare": (False, False, False, False),
                    "initialize": (False, False, False, False), "resume-initialize": (False, False, False, False),
                    "reconcile-preflight": (False, False, False, False),
                    "verify": (False, False, False, True), "status": (False, False, False, False)}[args.operation]
        if supplied != expected:
            raise ValueError("fresh-target 参数必须严格匹配当前阶段")
        if args.operation == "status":
            if args.write:
                raise ValueError("fresh-target status 是只读操作，不接受 --write")
            return target_status(backend, workspace)
        if not args.write:
            raise ValueError("fresh-target 非 status 操作需要显式 --write")
        if args.operation == "prepare":
            return prepare(backend, workspace, evidence_path(backend, args.request),
                           evidence_path(backend, args.environment), evidence_path(backend, args.storage_run))
        if args.operation == "resume-prepare":
            return resume_prepare(backend, workspace)
        if args.operation == "initialize":
            return initialize_target(backend, workspace)
        if args.operation == "resume-initialize":
            return resume_initialize(backend, workspace)
        if args.operation == "reconcile-preflight":
            return reconcile_preflight(backend, workspace)
        return verify(backend, workspace, evidence_path(backend, args.observation_dir, new=True))
    if args.command == "bridge":
        from source_fingerprints import write_bridge

        write_bridge(backend, args.build, args.inventory, args.output)
        return {"status": "artifact_reuse_audited", "receipt": str(args.output), "compiled": False,
                "restore_qualified": False}
    directory = evidence_path(backend, args.run_dir)
    if args.command == "init":
        return initialize(backend, args.manifest, directory)
    if args.command == "status":
        return status(backend, directory)
    if args.command == "storage":
        if args.request is not None and args.operation != "restart":
            raise ValueError("仅 storage restart 可提供 --request")
        if args.operation == "status":
            from devex_clone_storage import storage_status

            return storage_status(backend, directory, args.side)
        if not args.write:
            raise ValueError("存储启停和恢复需要显式 --write")
        result = execute(backend, directory, "storage-" + args.side, args.operation, storage_request=args.request)
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    if args.command == "cache":
        if args.request is not None and args.operation != "restart":
            raise ValueError("仅 cache restart 可提供 --request")
        if args.operation == "status":
            from devex_clone_cache import cache_status

            return cache_status(backend, directory)
        if not args.write:
            raise ValueError("缓存恢复、启停和核对需要显式 --write")
        result = execute(backend, directory, "cache-target", args.operation, cache_request=args.request)
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    if args.command == "seed-runtime":
        request_operations = {"register", "arm-input", "source-rebind", "source-generation-start", "source-generation-stop", "source-generation-recover"}
        if (args.operation in request_operations) != (args.request is not None):
            raise ValueError("当前 seed 操作的 --request 缺失或不适用")
        if (args.operation == "recover-session") != (args.producer_binding is not None):
            raise ValueError("仅 seed recover-session 且必须明确提供 --producer-binding")
        if args.operation == "status":
            from devex_clone_seed_runtime import execute_seed

            return execute_seed(backend, directory, None, "status", None)
        if args.operation == "source-generation-status":
            from devex_clone_seed_generation_control import status as generation_status

            if args.write:
                raise ValueError("source-generation-status 是只读操作，不接受 --write")
            return generation_status(backend, directory)
        if not args.write:
            raise ValueError("seed 操作需要显式 --write")
        producer = read_json(local_path(backend, str(args.producer_binding))) if args.producer_binding else None
        request = evidence_path(backend, args.request) if args.request is not None else None
        result = execute(backend, directory, "seed-runtime", args.operation, seed_request=request, producer_binding=producer)
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    if args.command == "post-copy":
        if (args.operation in {"register", "amend"}) != (args.request is not None):
            raise ValueError("仅 register/amend 且必须明确提供 --request")
        if (args.operation == "recover-session") != (args.producer_binding is not None):
            raise ValueError("仅 recover-session 且必须明确提供 --producer-binding")
        producer = read_json(local_path(backend, str(args.producer_binding))) if args.producer_binding else None
        result = execute(backend, directory, "post-copy", args.operation, post_copy_request=args.request, producer_binding=producer)
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    if args.command == "stage":
        result = execute(backend, directory, args.stage, args.mode)
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    if args.command == "recover":
        value = read_json(local_path(backend, str(args.owner_binding)))
        return recover_lock(backend, directory, value, verify_results=False)
    if args.command == "recover-copy":
        return recover_copy(backend, directory, read_json(local_path(backend, str(args.owner_binding))))
    if args.command == "runtime":
        if args.operation == "status":
            value, environment = cleanup_inputs(backend, directory, args.side)
            return run_runtime(backend, value, environment, args.side, "status", tuple(args.roles), run_directory=directory)
        if not args.write:
            raise ValueError("环境启停需要显式 --write")
        result = execute(backend, directory, "runtime-" + args.side, args.operation, tuple(args.roles))
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    raise ValueError("未知统一验收命令")
