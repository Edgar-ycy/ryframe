"""Rust 生命周期使用的有界私有上下文与 OS 原语；不决定阶段、恢复或重试。"""
from __future__ import annotations
import json
from pathlib import Path
import sys

LIMIT = 4 * 1024 * 1024


def _unique(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("私有原语字段重复")
        value[key] = item
    return value


def _context(value, backend):
    from devex_clone_model import local_path
    if Path(value["backend"]) != backend:
        raise ValueError("服务上下文属于其他后端")
    local_path(backend, value["run"])
    for key in ("backend", "run"):
        value[key] = Path(value[key])
    return value


def dispatch(backend, operation, args):
    from reference_fixture_service_context import guard
    from devex_clone_run_state import recover_run_lock
    from process_environment import configured
    from full_stack_process_tree import terminate_owned_process_tree
    from devex_clone_cache_process import start, stop
    schemas = {
        "tree-stop": {"tree", "crash"}, "recover-controller": {"run", "owner"},
        "cache-stop": {"request", "private", "runtime", "output"},
        "cache-start": {"request", "private", "output", "context", "running"},
        "storage-start": {"request", "private", "output", "manifest", "controller", "number", "context", "running"},
    }
    if operation not in schemas or not isinstance(args, dict) or set(args) != schemas[operation]:
        raise ValueError("服务原语字段或操作无效")
    from devex_clone_model import local_path
    for key in ("output", "run"):
        if key in args:
            local_path(backend, args[key])
    if operation == "tree-stop":
        return terminate_owned_process_tree(args["tree"], crash=args["crash"])
    if operation == "recover-controller":
        return recover_run_lock(backend, Path(args["run"]), args["owner"], verify_results=True)
    if operation == "cache-stop":
        return stop(args["request"], configured(args["private"]), args["runtime"], Path(args["output"]))
    value = _context(args["context"], backend)
    def checked():
        guard(value, args["running"])
    if operation == "cache-start":
        return start(args["request"], configured(args["private"]), Path(args["output"]), checked)
    from devex_clone_storage_process import start as storage_start
    return storage_start(backend, args["request"], configured(args["private"]), Path(args["output"]),
                         args["manifest"], args["controller"], args["number"], checked, supervised=True)


def main():
    try:
        if sys.argv[1:]:
            raise ValueError("服务原语只接受 stdin")
        raw = sys.stdin.buffer.read(LIMIT + 1)
        if not raw or len(raw) > LIMIT:
            raise ValueError("服务原语请求大小无效")
        value = json.loads(raw.decode("utf-8"), object_pairs_hook=_unique)
        if (not isinstance(value, dict) or set(value) != {"format_version", "kind", "backend", "native_tool", "operation", "arguments"}
                or type(value["format_version"]) is not int or value["format_version"] != 1
                or value["kind"] != "ryframe-fixture-service-primitive"):
            raise ValueError("服务原语协议无效")
        from rate_limit_tool import install_binding
        install_binding(json.dumps(value["native_tool"]))
        backend = Path(value["backend"])
        if backend.resolve(strict=True) != Path(__file__).resolve().parents[2]:
            raise ValueError("服务原语后端来源不同")
        result = dispatch(backend, value["operation"], value["arguments"])
        encoded = json.dumps({"ok": True, "result": result}, ensure_ascii=False, default=str).encode("utf-8")
        if len(encoded) > LIMIT:
            raise ValueError("服务原语输出超限")
        sys.stdout.buffer.write(encoded)
        return 0
    except BaseException as error:
        category = type(error).__name__
        if category not in {"ValueError", "PermissionError", "TimeoutError"}:
            category = "PrimitiveUnknown"
        sys.stdout.buffer.write(json.dumps({"ok": False, "error": category}).encode("utf-8"))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
