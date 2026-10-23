"""严格恢复目标计划、已有数据检查、来源数据准备与备份恢复；写入阶段要求 --write。"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

from dataclasses import dataclass
import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path

from full_stack_process import process_identity, read_process
from process_sockets import verify_listener
from restore_build import file_digest, source_snapshot
from restore_reference_backup import backup_source, document_binding, validate_backup_result
from restore_reference_execution import RestoreInputs, restore_inputs, restore_output
from restore_reference_images import RestoreImages
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import (dataset_timeout_seconds, plan_hash,
                                    validate_inventory, validate_plan,
                                    verify_artifacts)
from restore_runtime import read_json
from restore_runtime_evidence import read_json_document
from restore_runtime_registration import registered_stopped_runtime
from restore_reference_target import execute_plan
from source_inventory import build_source_domains, capture_inventory
from native_s3_client import _invoke, _read_response

DATASET_PROTOCOL_KIND = "ryframe-xtask-recovery-dataset-prepare"


def _coordinate(operation: str, arguments: dict, *, write=False):
    response, _ = _read_response(*_invoke(
        "__source-evidence", {"operation": "reference-" + operation,
            "arguments": arguments, "write": write}, subprocess.run, may_write=False,
    ))
    return response["value"]


def now() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).isoformat()


def write_json(path: Path, value: dict, *, new=True) -> None:
    with path.open("x" if new else "w", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def dataset_protocol(backend: Path, plan: Path, preflight: Path) -> str:
    return _coordinate("dataset-protocol", {
        "backend": str(backend), "plan": str(plan), "preflight": str(preflight),
    })


def failure_diagnostic(error: BaseException) -> dict:
    """记录外部命令的脱敏输出，保留未知写入阶段的定位证据。"""
    result = {"error_type": type(error).__name__, "message": str(error)[:1000]}
    if getattr(error, "__notes__", None):
        result["notes"] = [redact_object_diagnostic(note[:1000], os.environ) for note in error.__notes__[:20]]
    if isinstance(error, subprocess.CalledProcessError):
        result["stdout"] = redact_object_diagnostic(error.stdout, os.environ)
        result["stderr"] = redact_object_diagnostic(error.stderr, os.environ)
    return result


def work_directory(plan: dict) -> Path:
    return Path(_coordinate("work-directory", {"plan": plan}, write=True))


def require_stopped(plan: dict, side: str) -> None:
    config = plan[side]
    for role in ("api", "worker"):
        identity = read_process(Path(config["runtime_dir"]), role, config["scope_id"])
        actual = process_identity(identity["pid"])
        if actual is not None:
            raise ValueError("复制和还原期间已登记 API/Worker 必须停止；PID 重用也须重新确认")


def artifact(root: Path, path: Path, resource: str) -> dict:
    return {"relative_path": path.relative_to(root).as_posix(), "resource": resource, **file_digest(path)}


def backup(plan: dict, tools: ExternalTools, work: Path, inventory: dict,
           backend: Path, source_generation: Path,
           source_export_result: Path) -> dict:
    validate_inventory(plan, inventory)
    inputs = backup_source(backend, plan, inventory, source_generation, source_export_result, live=True)
    require_stopped(plan, "source")
    tools.verify_databases("source")
    tools.verify_objects("source")
    root = work / "backup"
    root.mkdir(exist_ok=False)
    artifacts = []
    for database in inventory["databases"]:
        path = root / "databases" / f"{database['key']}.sql"
        path.parent.mkdir(exist_ok=True)
        settings = next(db for db in plan["source"]["databases"] if db["key"] == database["key"])
        tools.dump(settings, [table["table"] for table in database["tables"]], path)
        artifacts.append(artifact(root, path, f"db:{database['key']}"))
    for objects in inventory["objects"]:
        directory = root / "objects" / objects["bucket"]
        directory.mkdir(parents=True, exist_ok=False)
        entries = []
        for entry in objects["entries"]:
            path = directory / (hashlib.sha256(entry["key"].encode()).hexdigest() + ".bin")
            metadata = tools.read_object("source", objects["bucket"], entry["key"], path,
                                         max_bytes=max(1, entry["bytes"]))
            if any(file_digest(path)[key] != entry[key] for key in ("bytes", "sha256")):
                raise ValueError("外部对象复制与静止采集清单不一致")
            value = artifact(root, path, f"objects:{objects['bucket']}")
            artifacts.append(value)
            entries.append({**entry, "file": value["relative_path"], "content_type": metadata.get("ContentType", "application/octet-stream")})
        path = directory / "index.json"
        write_json(path, {"entries": entries})
        artifacts.append(artifact(root, path, f"objects:{objects['bucket']}"))
    manifest = _coordinate("backup-manifest", {
        "plan": plan, "inventory": inventory, "artifacts": artifacts, "root": str(root),
    })
    require_stopped(plan, "source")
    if backup_source(backend, plan, inventory, source_generation, source_export_result, live=True) != inputs:
        raise ValueError("备份期间共享导出或来源绑定被替换")
    write_json(root / "manifest.json", manifest)
    result = {"format_version": 2, "kind": "restore-reference-backup", "reference_plan": plan,
            "manifest": document_binding(read_json_document(root / "manifest.json")),
            "backup_root": str(root), "artifacts": len(artifacts), **inputs}
    validate_backup_result(result)
    return result


def restore(plan: dict, tools: ExternalTools, backend: Path, inputs: RestoreInputs) -> dict:
    inputs.assert_unchanged()
    result = None
    try:
        with registered_stopped_runtime(backend, inputs.registration.path, document_binding(inputs.target)) as checkpoint:
            result = restore_locked(plan, tools, backend, inputs, checkpoint)
        return result
    except BaseException as error:
        if result is not None:
            error.restore_evidence = result["resource_images"]
        raise


def restore_locked(plan: dict, tools: ExternalTools, backend: Path, inputs: RestoreInputs, checkpoint) -> dict:
    runtime = checkpoint()
    current = restore_inputs(backend, plan, inputs.target.path, inputs.root, inputs.record.path,
                             inputs.registration.path, read_only=False)
    if current.bindings() != inputs.bindings():
        raise ValueError("恢复执行前目标计划或运行记录发生变化")
    root, manifest, record = current.root, current.manifest.value, current.record.value
    tasks = _coordinate("restore-resources", {"plan": plan, "root": str(root), "manifest": manifest})
    images = RestoreImages(backend, plan, current, tools)
    try:
        with images.control():
            checkpoint()
            images.capture("before", after=False)
            checkpoint()
            current.assert_unchanged()
            try:
                restore_resources(plan, tools, current, tasks, checkpoint, images)
                checkpoint()
                verify_artifacts(root, manifest)
                images.capture("after", after=True)
            except BaseException as error:
                if "after" not in images.bindings:
                    try:
                        checkpoint()
                        images.capture("failure-after", after=True)
                    except BaseException as failure:
                        error.add_note("恢复失败后完整资源像复核也失败：" + type(failure).__name__)
                raise
            checkpoint()
            final = restore_inputs(backend, plan, current.target.path, current.root, current.record.path,
                                   current.registration.path, read_only=True)
            if final.bindings() != current.bindings():
                raise ValueError("恢复完成复核时来源或输入绑定发生变化")
    except BaseException as error:
        error.restore_evidence = dict(images.bindings)
        raise
    return _coordinate("restore-result", {
        "record": record, "bindings": current.bindings(), "runtime": runtime, "images": images.bindings,
    })


def restore_resources(plan: dict, tools: ExternalTools, current: RestoreInputs, tasks: list,
                      checkpoint, images: RestoreImages) -> None:
    for task in tasks:
        checkpoint()
        current.assert_unchanged()
        bound = _coordinate("bound-artifact", {
            "root": str(current.root), "manifest": current.manifest.value, "relative": task["relative"],
        })
        source = Path(bound["path"])
        with images.target_environment():
            if task["kind"] == "database":
                tools.restore_database(task["target"], task["tables"], source)
            else:
                tools.create_object_if_absent(task["bucket"], task["key"], source,
                                               task["metadata"], {key: bound[key] for key in ("bytes", "sha256")})



class ReferenceProtocolError(ValueError):
    """Rust 与私有阶段之间的参数协议无效。"""


@dataclass
class ReferenceRequest:
    command: str
    backend_dir: Path
    plan: Path
    write: bool
    inventory: Path | None = None
    source_generation: Path | None = None
    source_export_result: Path | None = None
    backup_root: Path | None = None
    record: Path | None = None
    runtime_registration: Path | None = None
    copy_id: str | None = None
    artifact: str | None = None
    side: str | None = None
    backup_receipt: Path | None = None
    comparison_sources: Path | None = None
    arm_input: Path | None = None
    fresh_target_verify: Path | None = None
    product_plan: Path | None = None
    target_plan: Path | None = None
    output: Path | None = None
    missing: bool = False
    restore_inputs: object | None = None


def private_protocol_request(argv: list[str] | None = None) -> ReferenceRequest:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) == 3 and arguments[0] == "--native-tool" and arguments[2] == "--stdin":
        from rate_limit_tool import install_binding
        try:
            install_binding(arguments[1])
        except (ValueError, TypeError) as error:
            raise ReferenceProtocolError("reference 原生工具绑定无效") from error
        arguments = ["--stdin"]
    if arguments != ["--stdin"]:
        raise ReferenceProtocolError("reference 私有脚本只接受 --stdin")
    raw = sys.stdin.buffer.read(65_537)
    try:
        return _request_from_raw(raw.decode("utf-8"))
    except (ValueError, UnicodeError) as error:
        raise ReferenceProtocolError("reference stdin 私有协议无效") from error


def _request_from_raw(raw: str) -> ReferenceRequest:
    try:
        values = _coordinate("request", {"raw": raw})
    except (ValueError, RuntimeError) as error:
        raise ReferenceProtocolError("reference 私有参数协议无效") from error
    paths = {name: Path(value) for name, value in values.items() if name in {
        "backend_dir", "plan", "inventory", "source_generation", "source_export_result",
        "backup_root", "record", "runtime_registration", "backup_receipt", "comparison_sources",
        "arm_input", "fresh_target_verify", "product_plan", "target_plan", "output"}}
    return ReferenceRequest(**{**values, **paths})


def main(args: ReferenceRequest) -> None:
    backend, plan = args.backend_dir.resolve(strict=True), read_json(args.plan)
    validate_plan(plan, backend)
    if args.command == "plan":
        result = execute_plan(args, backend, plan)
        print(json.dumps(result if result is not None else _coordinate("summary", {"plan": plan}), ensure_ascii=False))
        return
    if args.command in ("check-dataset", "check-existing"):
        work = Path(plan["work_dir"])
        _coordinate("work-verify", {"plan": plan})
        side = "source" if args.command == "check-dataset" else args.side or "target"
        check_api(plan, ExternalTools(plan, work), side)
        if args.command == "check-dataset":
            require_empty_source(plan, ExternalTools(plan, work))
        print(json.dumps({"plan_sha256": plan_hash(plan), "side": side,
                          "scope_id": plan[side]["scope_id"]}))
        return
    if not args.write:
        raise ReferenceProtocolError("所有执行阶段必须显式授权写入")
    if args.command == "restore":
        args.restore_inputs = restore_inputs(backend, plan, args.target_plan, args.backup_root, args.record,
                                             args.runtime_registration, read_only=True)
        restore_output(backend, plan, args.restore_inputs)
    work = work_directory(plan)
    tools = ExternalTools(plan, work)
    lock = work / ".reference-lock"
    lock.mkdir(exist_ok=False)
    try:
        result = execute(args, plan, backend, tools, work)
        print(json.dumps(result, ensure_ascii=False))
    finally:
        lock.rmdir()


def execute(args, plan: dict, backend: Path, tools: ExternalTools, work: Path) -> dict:
    inputs = None
    output = None
    if args.command == "restore":
        inputs = getattr(args, "restore_inputs", None) or restore_inputs(
            backend, plan, args.target_plan, args.backup_root, args.record, args.runtime_registration, read_only=True)
        output = restore_output(backend, plan, inputs)
    receipt = _coordinate("receipt-start", {
        "plan": plan, "command": args.command, "copy_id": args.copy_id,
        "root": str(args.backup_root.resolve()) if args.command == "damage" else None,
        "receipt": str(output) if output is not None else None,
        "bindings": inputs.bindings() if inputs is not None else None,
    }, write=True)
    try:
        if args.command == "dataset":
            timeout = dataset_timeout_seconds(plan)
            source = source_snapshot(backend)
            tool_root = Path(__file__).resolve().parents[2]
            tool_inventory = capture_inventory(tool_root)
            runner = tool_root / "xtask/src/recovery/dataset_prepare.rs"
            write_json(work / "dataset-tools.json", {
                "format_version": 1, "kind": "restore-reference-dataset-tools",
                "root": str(tool_root),
                "sources": build_source_domains(tool_inventory, "backend"),
                "runner": {"path": str(runner), **file_digest(runner)},
            })
            check_api(plan, tools, "source")
            require_empty_source(plan, tools)
            preflight = _coordinate("dataset-preflight", {"plan": plan})
            preflight_path = work / "dataset-preflight.json"
            write_json(preflight_path, preflight)
            from rate_limit_tool import tool_binding, verified_tool
            from process_environment import configured
            with verified_tool(tool_binding()) as executable:
                command = ([executable, "__recovery-dataset"] if executable else [
                    shutil.which("cargo") or "cargo", "run", "--quiet", "--locked", "--manifest-path",
                    str(tool_root / "Cargo.toml"), "-p", "xtask", "--",
                    "__recovery-dataset",
                ])
                tools.execute(command, data=dataset_protocol(
                    backend, args.plan.resolve(), preflight_path.resolve()).encode("utf-8"),
                    env=configured({}), timeout=timeout)
            result = read_json(work / "dataset/result.json")
            result = _coordinate("dataset-result", {
                "preflight": preflight, "actual_preflight": read_json(preflight_path),
                "source": source, "actual_source": source_snapshot(backend),
                "tools": tool_inventory, "actual_tools": capture_inventory(tool_root), "result": result,
            })
        elif args.command == "backup":
            inventory = read_json(args.inventory)
            if "id" in inventory:
                raise ValueError("backup 必须直接消费共享导出的原始库存，不能手工补写 id")
            inventory = _coordinate("backup-inventory", {
                "plan": plan, "inventory": inventory, "source": source_snapshot(backend),
            })
            result = backup(plan, tools, work, inventory, backend, args.source_generation.absolute(), args.source_export_result.absolute())
        elif args.command == "restore":
            result = restore(plan, tools, backend, inputs)
        else:
            raise ReferenceProtocolError("本地 copy/damage 阶段必须通过 Rust 公共入口运行")
        _coordinate("receipt-finish", {"receipt": receipt, "result": result,
            "failure": None, "resource_images": None}, write=True)
        return result
    except BaseException as error:
        try:
            _coordinate("receipt-finish", {"receipt": receipt, "result": None,
                "failure": failure_diagnostic(error),
                "resource_images": getattr(error, "restore_evidence", None)}, write=True)
        except BaseException as failure:
            error.add_note("失败收据发布未确认，保留运行收据和未知结果：" + type(failure).__name__)
        raise


def check_api(plan: dict, tools: ExternalTools, side: str) -> None:
    tools.verify_databases(side)
    config = plan[side]
    identity = read_process(Path(config["runtime_dir"]), "api", config["scope_id"])
    if process_identity(identity["pid"]) != identity:
        raise ValueError("业务验收 API 进程不属于本次隔离环境")
    verify_listener(identity["pid"], config["api_url"])


def require_empty_source(plan: dict, tools: ExternalTools) -> None:
    database = next(db for db in plan["source"]["databases"] if db["kind"] == "combined")
    if tools.mysql(database, "SELECT COUNT(*) FROM sys_tenant WHERE tenant_id <> 'system';") != "0":
        raise ValueError("参考数据只允许准备到没有普通租户的明确隔离环境")


if __name__ == "__main__":
    try:
        main(private_protocol_request())
    except ReferenceProtocolError:
        print("restore_reference_protocol_error：私有参数协议无效。", file=sys.stderr)
        raise SystemExit(2)
    except Exception:
        print("restore_reference_failed：阶段失败，请核对账本、前后像和未知结果；不自动重放。",
              file=sys.stderr)
        raise SystemExit(1)
