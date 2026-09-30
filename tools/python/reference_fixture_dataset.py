"""将 Device 夹具的已登记源运行时接入正式参考数据准备流程。"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

from reference_fixture_control_protocol import run_private


PROTOCOL_SCHEMAS = {
    "plan": (("environment", "runtime", "work_dir", "output", "side"), (), True),
    "prepare": (("environment", "runtime", "plan", "side"), (), True),
}


def _load_business_dependencies() -> None:
    """直接执行时只在私有协议被移除后加载会读取运行环境的业务模块。"""
    global Environments, KEYS, _bootstrap, _output, _runtime_environment
    global file_digest, linked, local_path, plan_hash, read_json
    global redact_object_diagnostic, validate_plan, validate_review, verify_runtime, write_json

    from devex_clone_capture import read_json, write_json
    from devex_clone_model import linked, local_path
    from devex_clone_target_binding import KEYS, validate_review
    from process_environment import Environments
    from reference_fixture_runtime import (
        _bootstrap,
        _output,
        _runtime_environment,
        verify as verify_runtime,
    )
    from restore_build import file_digest
    from restore_reference_io import redact_object_diagnostic
    from restore_reference_plan import plan_hash, validate_plan


if __name__ != "__main__":
    _load_business_dependencies()


def _bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("夹具参考数据输入必须是受控普通文件")
    return path, read_json(path)


def _node() -> dict:
    value = shutil.which("node")
    if value is None:
        raise ValueError("本机缺少已登记的 Node 运行时")
    path = Path(value).resolve(strict=True)
    if linked(path) or not path.is_file():
        raise ValueError("Node 运行时不是受控普通文件")
    return {"path": str(path), "sha256": file_digest(path)["sha256"]}


def _side(scope: dict) -> dict:
    required = {"scope_id", "runtime_dir", "api_url", "frontend_url", "objects", "databases"}
    if not required.issubset(scope) or set(item.get("key") for item in scope["databases"]) != set(KEYS):
        raise ValueError("夹具审阅侧缺少完整运行与数据库描述")
    databases = []
    for key in sorted(KEYS):
        item = next(value for value in scope["databases"] if value["key"] == key)
        defaults = Path(item["connection_file"])
        if not defaults.is_absolute() or linked(defaults) or not defaults.is_file():
            raise ValueError("夹具 MySQL 凭据文件无效")
        databases.append({
            "key": key,
            "kind": KEYS[key][0],
            "mode": KEYS[key][1],
            "database": item["database"],
            "server_uuid": item["expected_server_uuid"],
            "defaults_file": str(defaults),
            "defaults_sha256": file_digest(defaults)["sha256"],
        })
    return {
        "scope_id": scope["scope_id"],
        "runtime_dir": scope["runtime_dir"],
        "api_url": scope["api_url"],
        "frontend_url": scope["frontend_url"],
        "s3": {
            "endpoint": scope["objects"]["endpoint"],
            "region": scope["objects"]["region"],
            "access_key_env": "APP_OBJECT_STORAGE_ACCESS_KEY",
            "secret_key_env": "APP_OBJECT_STORAGE_SECRET_KEY",
        },
        "databases": databases,
    }


def _plan_file(execution: Path, runtime: Path, value: Path, *, new: bool) -> Path:
    path = value.resolve() if value.is_absolute() else (execution / value).resolve()
    root = (execution / ".local-tests/reference-fixture").resolve()
    if (path.parent != runtime or not path.is_relative_to(root) or linked(path)
            or new and path.exists() or not new and (not path.is_file() or linked(path))):
        raise ValueError("参考数据计划必须是源运行时目录中的明确文件")
    return path


def _dataset() -> dict:
    return {
        "timeout_seconds": 43_200,
        "request_interval_ms": 1_000,
        "api_validation_posts": 2,
        "post_batch_rows": 1_000,
        "tenant_targets": ["shared"] * 8 + ["dedicated-a", "dedicated-b"],
        "records": 100_000,
        "object_count": 256,
        "object_bytes": 4 * 1024 * 1024,
        "admin": {
            "tenant_id": "system",
            "username": "admin",
            "password_env": "RYFRAME_RESET_ADMIN_PASSWORD",
        },
        "owner_password_env": "RYFRAME_RESET_ADMIN_PASSWORD",
    }


def _write_prepare_failure(runtime: Path, plan_file: Path, error: subprocess.CalledProcessError, values: dict,
                           report_directory: Path) -> Path:
    """保存已脱敏的子进程失败证据，禁止把未知写入当作可重放阶段。"""
    output = runtime / (plan_file.stem + ".prepare-failed.json")
    if output.exists():
        raise ValueError("参考数据准备失败证据已存在，禁止覆盖或重放")
    write_json(output, {
        "format_version": 1,
        "kind": "reference-fixture-dataset-failure",
        "status": "failed_unknown_writes",
        "returncode": error.returncode,
        "stdout": redact_object_diagnostic(error.stdout, values),
        "stderr": redact_object_diagnostic(error.stderr, values),
        "fatal_report_directory": str(report_directory),
    })
    return output


def build_plan(backend: Path, environment_path: Path, runtime_path: Path, work_dir: Path,
               side: str) -> dict:
    """从同代次 bootstrap、运行时和审阅计划派生单一的参考数据计划。"""
    if side not in {"base", "candidate"}:
        raise ValueError("参考恢复目标必须显式选择 base 或 candidate")
    bootstrap_file, execution, private = _bootstrap(backend, environment_path)
    runtime = _output(execution, runtime_path, new=False)
    verified = verify_runtime(backend, environment_path, runtime)
    pair_file, pair = _read(backend, runtime / "source-pair.json")
    values = _runtime_environment(private, pair)
    review_binding = read_json(bootstrap_file).get("plan", {}).get("review")
    if not isinstance(review_binding, dict):
        raise ValueError("夹具私有环境缺少审阅计划绑定")
    review_file, review = _read(backend, Path(review_binding.get("path", "")))
    if _bound(review_file) != {key: review_binding.get(key) for key in ("path", "bytes", "sha256")}:
        raise ValueError("夹具审阅计划摘要已变化")
    validate_review(review)
    source = _side(review["scopes"]["seed"])
    target_scope = review["scopes"][side]
    target = _side(target_scope)
    target["worker_ready_url"] = target_scope["worker_ready_url"]
    source["runtime_dir"] = str(runtime)
    if source["scope_id"] != verified["scope_id"]:
        raise ValueError("夹具源运行时与审阅计划不属于同一代次")
    work = work_dir.resolve()
    allowed = (execution / ".local-tests").resolve()
    if not work.is_relative_to(allowed) or work.exists() or work.parent != runtime:
        raise ValueError("参考数据工作目录必须是源运行时下尚不存在的直接子目录")
    tools = review["tools"]
    result = {
        "format_version": 1,
        "id": "fixture-dataset-" + source["scope_id"].removeprefix("fixture-seed-"),
        "target_side": side,
        "work_dir": str(work),
        "source": source,
        "target": target,
        "tools": {
            name: {key: tools[name][key] for key in ("path", "sha256")}
            for name in ("mysql", "mysqldump", "aws")
        } | {"node": _node()},
        "dataset": _dataset(),
        "fixture_bindings": {
            "bootstrap": _bound(bootstrap_file),
            "runtime": verified["runtime"],
            "source_pair": _bound(pair_file),
            "review": _bound(review_file),
        },
    }
    validate_plan(result, execution)
    return result


def write_plan(backend: Path, environment_path: Path, runtime_path: Path, work_dir: Path,
               output: Path, side: str) -> dict:
    _, execution, _ = _bootstrap(backend, environment_path)
    runtime = _output(execution, runtime_path, new=False)
    target = _plan_file(execution, runtime, output, new=True)
    if work_dir.resolve() == target:
        raise ValueError("参考数据工作目录与计划输出必须不同")
    plan = build_plan(backend, environment_path, runtime_path, work_dir, side)
    write_json(target, plan)
    return plan


def prepare(backend: Path, environment_path: Path, runtime_path: Path, plan_path: Path,
            side: str) -> dict:
    _, execution, private = _bootstrap(backend, environment_path)
    runtime = _output(execution, runtime_path, new=False)
    plan_file = _plan_file(execution, runtime, plan_path, new=False)
    plan = read_json(plan_file)
    if plan.get("target_side") != side:
        raise ValueError("参考数据计划与显式恢复目标侧不一致")
    expected = build_plan(backend, environment_path, runtime, Path(plan.get("work_dir", "")), side)
    if plan != expected:
        raise ValueError("参考数据计划与当前夹具来源、工具或运行时不一致")
    pair = read_json(runtime / "source-pair.json")
    values = _runtime_environment(private, pair)
    reports = runtime / (plan_file.stem + ".node-reports")
    if reports.exists():
        raise ValueError("参考数据准备 Node fatal report 目录已存在，禁止覆盖或重放")
    reports.mkdir()
    node_options = " ".join(item for item in (os.environ.get("NODE_OPTIONS", ""), "--report-on-fatalerror",
                                                 f'--report-directory="{reports}"') if item)
    command = [sys.executable, "-X", "utf8", str(execution / "tools/python/restore_reference.py")]
    protocol = json.dumps({"format_version": 1, "kind": "ryframe-xtask-recovery-reference",
                           "request": {"backend_dir": str(execution), "format_version": 1,
                                       "operation": "dataset", "plan": str(plan_file),
                                       "write": True}}, separators=(",", ":"))
    try:
        stage_environment = {**values, "NODE_OPTIONS": node_options}
        with Environments(stage_environment, stage_environment).use("source"):
            child_environment = {key: value for key, value in os.environ.items()
                                 if not key.startswith("RYFRAME_XTASK_RECOVERY_")}
            child_environment["RYFRAME_XTASK_RECOVERY_REFERENCE"] = protocol
            subprocess.run(command, cwd=execution, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                           text=True, encoding="utf-8", check=True, env=child_environment,
                           creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0)
    except subprocess.CalledProcessError as error:
        diagnostic = _write_prepare_failure(runtime, plan_file, error, values, reports)
        raise RuntimeError(f"参考数据准备子进程失败；已保存脱敏诊断：{diagnostic}") from error
    return {"status": "reference_fixture_dataset_prepared", "plan": _bound(plan_file),
            "work_dir": plan["work_dir"], "dataset": _bound(Path(plan["work_dir"]) / "dataset/result.json"),
            "remote_writes": {"business_data": True, "objects": True}}


def main(arguments: list[str]) -> None:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("operation", choices=("plan", "prepare"))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--work-dir", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--plan", type=Path)
    parser.add_argument("--side", choices=("base", "candidate"), required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args(arguments)
    if args.operation == "plan":
        if not args.write or args.work_dir is None or args.output is None or args.plan is not None:
            parser.error("plan 需要 --work-dir、--output 与显式 --write，且不接受 --plan")
        result = write_plan(args.backend_dir.resolve(), args.environment, args.runtime, args.work_dir,
                            args.output, args.side)
        print(json.dumps({"status": "reference_fixture_dataset_planned", "plan_sha256": plan_hash(result), "remote_writes": 0}, ensure_ascii=False))
        return
    if not args.write or args.plan is None or args.work_dir is not None or args.output is not None:
        parser.error("prepare 需要 --plan 与显式 --write，且不接受 --work-dir 或 --output")
    print(json.dumps(prepare(args.backend_dir.resolve(), args.environment, args.runtime, args.plan,
                             args.side), ensure_ascii=False))


def _private_main(arguments: list[str]) -> None:
    _load_business_dependencies()
    main(arguments)


if __name__ == "__main__":
    raise SystemExit(
        run_private("dataset", PROTOCOL_SCHEMAS, _private_main, positional_operation=True)
    )
