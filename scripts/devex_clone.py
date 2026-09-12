"""开发复制统一入口；plan/verify/status 只读，资源操作使用显式阶段与 --write。"""
from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import sys
from weakref import WeakKeyDictionary

import devex_clone_model
from devex_clone_model import bound_file, create_plan, exact, local_path
from restore_build import file_digest
from artifact_digests import filesystem_path
from restore_reference_plan import plan_hash


@dataclass(frozen=True)
class _PlanState:
    backend: Path
    filename: Path
    wrapper: bytes
    declaration: bytes
    plan_binding: tuple[int, str]
    policy: tuple


_VERIFIED = WeakKeyDictionary()
CATALOG_INPUTS = ("sql/ryframe_config.sql", "crates/ryframe-tenant-db/src/generated/catalog.rs",
                  "crates/ryframe-tenant-db/src/migration/m20260820_000000_tenant_baseline.rs")


class VerifiedPlan:
    """完整验证在本进程签发的不可变结果；不从 JSON、复制或反序列化取得信任。"""
    __slots__ = ("__weakref__",)

    def __new__(cls):
        raise TypeError("计划验证结果只能由正式验证入口产生")

    @property
    def plan(self) -> dict:
        return json.loads(_plan_state(self).wrapper)["plan"]


def _plan_state(value: VerifiedPlan) -> _PlanState:
    if type(value) is not VerifiedPlan or value not in _VERIFIED:
        raise ValueError("缺少当前进程正式签发的完整计划验证结果")
    return _VERIFIED[value]


def _policy(backend: Path) -> tuple:
    # 目录集合和内容一起绑定；新增辅助模块不能在复用期间静默改变验证策略。
    roots = {Path(__file__).resolve().parent, Path(devex_clone_model.__file__).resolve().parent}
    paths = {backend / item for item in CATALOG_INPUTS}
    for root in roots:
        paths.update(path for path in root.rglob("*.py")
                     if "tests" not in path.relative_to(root).parts and not path.name.startswith("test_"))
    return tuple((str(path), tuple(file_digest(path).items())) for path in sorted(paths))


def _issue(backend: Path, filename: Path, wrapper: dict, policy: tuple) -> VerifiedPlan:
    source = local_path(backend, wrapper["input_path"])
    declaration = read_json(source)
    before = file_digest(filename)
    if (read_json(filename) != wrapper or file_digest(source) != wrapper["input_file"]
            or plan_hash(declaration) != wrapper["plan"]["input_sha256"]
            or file_digest(filename) != before or _policy(backend) != policy):
        raise ValueError("完整验证结果签发前计划、输入或源码策略变化")
    def encode(value: dict) -> bytes:
        return json.dumps(value, ensure_ascii=False, sort_keys=True).encode("utf-8")

    result = object.__new__(VerifiedPlan)
    _VERIFIED[result] = _PlanState(backend.resolve(), filename, encode(wrapper), encode(declaration),
                                   (before["bytes"], before["sha256"]), policy)
    return result


def reuse_plan(backend: Path, filename: str, verified: VerifiedPlan) -> tuple[dict, dict, dict]:
    """只复用语义推导；内容、完整证据、目录及辅助源码仍须逐次核验。"""
    state = _plan_state(verified)
    path = local_path(backend, filename)
    if backend.resolve() != state.backend or path != state.filename or _policy(backend) != state.policy:
        raise ValueError("计划验证结果不属于当前后端、文件或源码策略")
    wrapper, declaration = json.loads(state.wrapper), json.loads(state.declaration)
    source = local_path(backend, wrapper["input_path"])
    bindings = {path: dict(zip(("bytes", "sha256"), state.plan_binding, strict=True)), source: wrapper["input_file"]}
    if any(file_digest(local_path(backend, str(item))) != binding for item, binding in bindings.items()):
        raise ValueError("已验证计划或完整输入文件变化")
    root = local_path(backend, declaration["artifact_root"])
    for binding in (*declaration["evidence"].values(), *(item["artifact"] for item in declaration["databases"]),
                    *(item["artifact"] for bucket in declaration["objects"] for item in bucket["entries"])):
        bound_file(root, binding)
    if (_policy(backend) != state.policy
            or any(file_digest(local_path(backend, str(item))) != binding for item, binding in bindings.items())):
        raise ValueError("复用完整计划期间输入或源码策略变化")
    return wrapper, declaration, bindings


def unique_pairs(pairs: list) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError("复制清单包含重复 JSON 字段")
        result[key] = value
    return result


def read_json(path: Path) -> dict:
    native = filesystem_path(path)
    if os.stat(native).st_size > 16 * 1024 * 1024:
        raise ValueError("复制清单超过 16 MiB")
    with open(native, encoding="utf-8") as stream:
        value = json.load(stream, object_pairs_hook=unique_pairs)
    if not isinstance(value, dict):
        raise ValueError("复制清单必须为对象")
    return value


def inspect_input(backend: Path, filename: str) -> dict:
    source = local_path(backend, filename)
    before = file_digest(source)
    value = read_json(source)
    result = create_plan(value, backend)
    if file_digest(source) != before:
        raise ValueError("离线核验期间输入清单变化")
    return {"format_version": 1, "input_path": str(source), "input_file": before, "plan": result}


def write_plan_result(backend: Path, filename: str, output: str) -> VerifiedPlan:
    target = local_path(backend, output, new=True)
    value = read_json(local_path(backend, filename))
    artifacts = local_path(backend, value["artifact_root"])
    if target.is_relative_to(artifacts):
        raise ValueError("计划输出不得写入来源导出目录")
    policy = _policy(backend)
    result = inspect_input(backend, filename)
    with target.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(result, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    return _issue(backend, target, result, policy)


def verify_plan_result(backend: Path, filename: str) -> VerifiedPlan:
    source = local_path(backend, filename)
    before = file_digest(source)
    policy = _policy(backend)
    value = read_json(source)
    exact(value, {"format_version", "input_path", "input_file", "plan"})
    if value["format_version"] != 1 or inspect_input(backend, value["input_path"]) != value:
        raise ValueError("已写计划与当前完整输入、源码策略或导出证据不一致")
    if file_digest(source) != before:
        raise ValueError("核验期间计划文件变化")
    return _issue(backend, source, value, policy)


def write_plan(backend: Path, filename: str, output: str) -> dict:
    """公开离线输出普通计划，不暴露可反序列化的执行许可。"""
    return write_plan_result(backend, filename, output).plan


def verify_plan(backend: Path, filename: str) -> dict:
    """公开只读核验始终完整解析；复用只通过进程内明确组合参数传递。"""
    return verify_plan_result(backend, filename).plan


def _run_private_protocol(protocol, actual_argv, decoder, protocol_error, label, dispatch) -> int:
    if actual_argv:
        print(f"{label} 私有协议不接受 argv", file=sys.stderr)
        return 2
    try:
        backend, args = decoder(protocol)
    except protocol_error as error:
        print(f"{label} 私有协议无效：{error}", file=sys.stderr)
        return 2
    try:
        backend = backend.resolve(strict=True)
        print(json.dumps(dispatch(args, backend), ensure_ascii=False))
        return 0
    except (OSError, ValueError, KeyError, TypeError) as error:
        print(f"开发复制未完成：{type(error).__name__}", file=sys.stderr)
        return 1


def _direct_seed_source(arguments, operations) -> bool:
    if (not arguments or arguments[0] != "seed-runtime"
            or any(value in {"--help", "-h"} for value in arguments)):
        return False
    return any(
        arguments[index] == "--operation" and arguments[index + 1] in operations
        for index in range(1, len(arguments) - 1)
    )


def main(argv=None) -> int:
    from devex_clone_run_cli import (
        COMMANDS, FRESH_TARGET_PROTOCOL_ENV, FreshTargetProtocolError,
        SEED_SOURCE_OPERATIONS, SEED_SOURCE_PROTOCOL_ENV, SeedSourceProtocolError,
        add_commands, decode_fresh_target_protocol, decode_seed_source_protocol, dispatch,
    )

    fresh_protocol = os.environ.pop(FRESH_TARGET_PROTOCOL_ENV, None)
    seed_protocol = os.environ.pop(SEED_SOURCE_PROTOCOL_ENV, None)
    actual_argv = sys.argv[1:] if argv is None else list(argv)
    if fresh_protocol is not None and seed_protocol is not None:
        print("开发复制私有协议不能同时指定", file=sys.stderr)
        return 2
    if fresh_protocol is not None:
        return _run_private_protocol(
            fresh_protocol, actual_argv, decode_fresh_target_protocol,
            FreshTargetProtocolError, "fresh-target", dispatch,
        )
    if seed_protocol is not None:
        return _run_private_protocol(
            seed_protocol, actual_argv, decode_seed_source_protocol,
            SeedSourceProtocolError, "seed source", dispatch,
        )
    if argv is None and _direct_seed_source(actual_argv, SEED_SOURCE_OPERATIONS):
        print("seed source 是私有实现，不接受公开 argv", file=sys.stderr)
        return 2

    parser = argparse.ArgumentParser(description=__doc__, epilog="plan/verify 仅检查离线计划；stage 每次只执行明确阶段，未知写入先 reconcile 后 resume。调度处置未完成时 Worker 必须保持停止；本工具的复制结果不代表正式恢复通过。")
    commands = parser.add_subparsers(dest="command", required=True)
    prepare = commands.add_parser("plan", help="离线核对全部导出证据并显式写入新计划")
    prepare.add_argument("--input", required=True, help="当前后端忽略目录中的完整源/目标、导出证据及调度处置清单")
    prepare.add_argument("--output", required=True)
    prepare.add_argument("--write", required=True, action="store_true")
    verify = commands.add_parser("verify", help="只读重新核对已写计划与全部输入证据")
    verify.add_argument("--plan", required=True)
    for command in (prepare, verify):
        command.add_argument("--backend-dir", required=True, type=Path)
    add_commands(commands)
    args = parser.parse_args(actual_argv)
    try:
        backend = args.backend_dir.resolve(strict=True)
        if args.command in COMMANDS:
            print(json.dumps(dispatch(args, backend), ensure_ascii=False))
            return 0
        result = (write_plan(backend, args.input, args.output) if args.command == "plan"
                  else verify_plan(backend, args.plan))
        print(json.dumps({"status": result["status"], "plan_sha256": result["plan_sha256"],
                          "pending_target_actions": len(result["pending_target_actions"]), "target_ready": False,
                          "worker_must_remain_stopped": True,
                          "execution_authorized": False, "resources_modified": False}, ensure_ascii=False))
        return 0
    except (OSError, ValueError, KeyError, TypeError) as error:
        # 私有环境与外部工具错误留在隔离证据中，不把连接信息输出到调用端。
        print(f"开发复制未完成：{type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
