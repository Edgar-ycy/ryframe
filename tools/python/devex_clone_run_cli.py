"""统一开发复制入口的参数与编排；真实结果由阶段工具产生。"""
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import local_path
from devex_clone_run import execute

SEED_SOURCE_OPERATIONS = {
    "source-generation-start",
    "source-generation-stop", "source-generation-status",
    "source-generation-reboot-status",
    "source-generation-reboot-close",
    "source-generation-recover",
}
SEED_RUNTIME_OPERATIONS = SEED_SOURCE_OPERATIONS | {
    "register", "quotas-plan", "quotas-apply", "quotas-reconcile",
    "departments-plan", "departments-apply", "departments-reconcile", "departments-verify",
    "identities-apply", "identities-verify", "prepare", "close",
    "recover-session",
}


def evidence_path(backend: Path, value: Path, *, new: bool = False) -> Path:
    """将用户相对路径限定到当前后端的受控证据根。"""
    requested = value if value.is_absolute() else backend / value
    return local_path(backend, str(requested), new=new)


def dispatch(args, backend: Path) -> dict:
    if args.command == "stage" and args.stage != "copy":
        raise ValueError("Python 阶段入口仅接受 copy")
    if args.command == "seed-runtime" and args.operation not in SEED_RUNTIME_OPERATIONS:
        raise ValueError("Python seed-runtime 不接受此操作；请使用 Rust 正式入口")
    if args.command == "runtime":
        raise ValueError("复制运行由 Rust 正式入口执行")
    directory = evidence_path(backend, args.run_dir)
    if args.command == "seed-runtime":
        request_operations = {"register", "source-generation-start", "source-generation-stop", "source-generation-recover", "source-generation-reboot-close"}
        if (args.operation in request_operations) != (args.request is not None):
            raise ValueError("当前 seed 操作的 --request 缺失或不适用")
        if (args.operation == "recover-session") != (args.producer_binding is not None):
            raise ValueError("仅 seed recover-session 且必须明确提供 --producer-binding")
        if args.operation == "source-generation-status":
            from devex_clone_seed_generation_control import status as generation_status

            if args.write:
                raise ValueError("source-generation-status 是只读操作，不接受 --write")
            return generation_status(backend, directory)
        if args.operation == "source-generation-reboot-status":
            from devex_clone_seed_generation_reboot import observe as generation_reboot_observe

            if args.write:
                raise ValueError("source-generation-reboot-status 是只读操作，不接受 --write")
            return generation_reboot_observe(backend, directory)
        if not args.write:
            raise ValueError("seed 操作需要显式 --write")
        producer = read_json(local_path(backend, str(args.producer_binding))) if args.producer_binding else None
        request = evidence_path(backend, args.request) if args.request is not None else None
        result = execute(backend, directory, "seed-runtime", args.operation, seed_request=request, producer_binding=producer)
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    if args.command == "post-copy":
        if args.operation in {"register", "prepare", "amend"}:
            raise ValueError("登记、准备和环境修订必须使用原生持锁正式入口")
        if not args.write:
            raise ValueError("post-copy 需要显式 --write")
        if args.request is not None:
            raise ValueError("已登记业务阶段不能替换固定请求")
        if (args.operation == "recover-session") != (args.producer_binding is not None):
            raise ValueError("仅 recover-session 且必须明确提供 --producer-binding")
        producer = read_json(local_path(backend, str(args.producer_binding))) if args.producer_binding else None
        result = execute(backend, directory, "post-copy", args.operation, producer_binding=producer)
        return {key: result[key] for key in ("status", "stage", "mode", "attempt", "restore_qualified")}
    raise ValueError("未知统一验收命令")
