"""严格恢复目标计划、已有数据检查、来源数据准备与备份恢复；写入阶段要求 --write。"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import argparse
import datetime as dt
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
from restore_reference_io import ExternalTools, object_index, redact_object_diagnostic, validate_dump
from restore_reference_plan import (dataset_timeout_seconds, identifier, plan_hash, safe_file,
                                    validate_inventory, validate_plan,
                                    verify_artifacts)
from restore_runtime import read_json
from restore_runtime_evidence import read_json_document
from restore_runtime_registration import registered_stopped_runtime
from restore_reference_target_cli import add_arguments, execute_plan, validate_arguments


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def write_json(path: Path, value: dict, *, new=True) -> None:
    with path.open("x" if new else "w", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


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
    work = Path(plan["work_dir"]).resolve()
    owner = {"format_version": 1, "id": plan["id"], "plan_sha256": plan_hash(plan)}
    if work.exists():
        if read_json(work / "reference-owner.json") != owner:
            raise ValueError("演练目录已存在且 ownership 不匹配")
    else:
        work.mkdir(parents=True, exist_ok=False)
        write_json(work / "reference-owner.json", owner)
    return work


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
            metadata = tools.aws("source", "get-object", objects["bucket"], entry["key"], path)
            if any(file_digest(path)[key] != entry[key] for key in ("bytes", "sha256")):
                raise ValueError("外部对象复制与静止采集清单不一致")
            value = artifact(root, path, f"objects:{objects['bucket']}")
            artifacts.append(value)
            entries.append({**entry, "file": value["relative_path"], "content_type": metadata.get("ContentType", "application/octet-stream")})
        path = directory / "index.json"
        write_json(path, {"entries": entries})
        artifacts.append(artifact(root, path, f"objects:{objects['bucket']}"))
    # 采集时间来自 Inventory；完成或登记时间不能刷新恢复点。
    captured = dt.datetime.fromisoformat(inventory["captured_at"].replace("Z", "+00:00"))
    manifest = {**inventory, "id": plan["id"], "completed_at": now(),
                "retention_until": (captured + dt.timedelta(days=7)).isoformat(), "artifacts": artifacts}
    verify_artifacts(root, manifest)
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
    validate_inventory(plan, manifest)
    verify_artifacts(root, manifest)
    # 全部 SQL 与对象索引先校验；任一缺失或篡改不能留下部分目标写入。
    for db in manifest["databases"]:
        validate_dump(safe_file(root, f"databases/{db['key']}.sql"), {table["table"] for table in db["tables"]})
    indices = {objects["bucket"]: object_index(root, objects, manifest["artifacts"]) for objects in manifest["objects"]}
    images = RestoreImages(backend, plan, current, tools)
    try:
        with images.control():
            checkpoint()
            images.capture("before", after=False)
            checkpoint()
            current.assert_unchanged()
            try:
                restore_resources(plan, tools, current, indices, checkpoint, images)
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
    return {"restore_id": record["plan"]["id"], "status": "external_copy_completed",
            **current.bindings(), "runtime": runtime, "resource_images": images.bindings,
            "next": "执行 restore-verify-data；外部复制完成不代表恢复成功"}


def restore_resources(plan: dict, tools: ExternalTools, current: RestoreInputs, indices: dict,
                      checkpoint, images: RestoreImages) -> None:
    root, manifest = current.root, current.manifest.value
    artifacts = {entry["relative_path"]: entry for entry in manifest["artifacts"]}
    def bound_artifact(relative):
        path = safe_file(root, relative)
        if any(artifacts[relative][key] != value for key, value in file_digest(path).items()):
            raise ValueError("恢复写入前当前产物字节与备份清单不同")
        return path
    for db in manifest["databases"]:
        checkpoint()
        current.assert_unchanged()
        target = next(value for value in plan["target"]["databases"] if value["key"] == db["key"])
        with images.target_environment():
            tools.restore_database(target, [table["table"] for table in db["tables"]], bound_artifact(f"databases/{db['key']}.sql"))
    for bucket, index in indices.items():
        for entry in index["entries"]:
            checkpoint()
            current.assert_unchanged()
            key = plan["target"]["scope_id"] + "/" + entry["key"].removeprefix(plan["source"]["scope_id"] + "/")
            with images.target_environment():
                tools.aws("target", "put-object", bucket, key, bound_artifact(entry["file"]), content_type=entry["content_type"])


def copy_backup(work: Path, manifest: dict, source: Path, new_id: str) -> dict:
    identifier(new_id)
    if new_id == manifest["id"]:
        raise ValueError("失败演练必须使用独立备份 ID")
    verify_artifacts(source, manifest)
    target = work / new_id
    target.mkdir(exist_ok=False)
    for item in manifest["artifacts"]:
        destination = safe_file(target, item["relative_path"], exists=False)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(safe_file(source, item["relative_path"]), destination)
    copied = {**manifest, "id": new_id}
    verify_artifacts(target, copied)
    write_json(target / "manifest.json", copied)
    return {"manifest": str(target / "manifest.json"), "backup_root": str(target),
            "next": "先登记有效副本并执行 restore-begin，然后用 damage 注入损坏，再执行 restore-verify-data"}


def damage(work: Path, root: Path, manifest: dict, relative: str, missing: bool) -> dict:
    if (root.resolve() in (work.resolve(), (work / "backup").resolve())
            or not root.resolve().is_relative_to(work.resolve())):
        raise ValueError("只允许损坏当前演练目录内的独立备份副本")
    verify_artifacts(root, manifest)
    if relative not in {entry["relative_path"] for entry in manifest["artifacts"]}:
        raise ValueError("损坏文件必须精确登记在该副本清单")
    path = safe_file(root, relative)
    if missing:
        path.unlink()
    else:
        with path.open("r+b") as stream:
            first = stream.read(1)
            stream.seek(0)
            stream.write(bytes([first[0] ^ 1]))
    return {"backup_id": manifest["id"], "artifact": relative, "injected": "missing" if missing else "corrupt"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("command", choices=("plan", "check-dataset", "check-existing", "dataset", "backup", "restore", "copy", "damage"))
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--inventory", type=Path)
    parser.add_argument("--source-generation", type=Path, help="backup 必须绑定同代 START、source verify 与完整停止后像的最终 STOP 发布结果")
    parser.add_argument("--source-export-result", type=Path, help="backup 必须绑定已发布的同一共享 source-export 结果")
    parser.add_argument("--backup-root", type=Path)
    parser.add_argument("--record", type=Path)
    parser.add_argument("--runtime-registration", type=Path, help="restore 必须绑定同一 target plan 的三端停止登记")
    parser.add_argument("--copy-id")
    parser.add_argument("--artifact")
    parser.add_argument("--missing", action="store_true")
    parser.add_argument("--side", choices=("source", "target"),
                        help="仅 check-existing 可指定检查侧，默认 target；不改变数据准备或恢复目标")
    parser.add_argument("--write", action="store_true")
    add_arguments(parser)
    options = [value.partition("=")[0] for value in sys.argv[1:] if value.startswith("--")]
    if len(options) != len(set(options)):
        parser.error("恢复验收选项不能重复")
    args = parser.parse_args()
    validate_arguments(parser, args)
    if args.side is not None and args.command != "check-existing":
        parser.error("--side 仅用于 check-existing；数据准备固定 source，恢复固定 target")
    if any(value is not None for value in (args.source_generation, args.source_export_result)) and args.command != "backup":
        parser.error("--source-generation 与 --source-export-result 仅用于 backup")
    backend, plan = args.backend_dir.resolve(), read_json(args.plan)
    validate_plan(plan, backend)
    if args.command == "plan":
        result = execute_plan(args, backend, plan)
        print(json.dumps(result if result is not None else {"id": plan["id"], "plan_sha256": plan_hash(plan), "work_dir": plan["work_dir"],
                          "targets": {side: [{key: db[key] for key in ("key", "server_uuid", "database")} for db in plan[side]["databases"]]
                                      for side in ("source", "target")}}, ensure_ascii=False))
        return
    if args.command in ("check-dataset", "check-existing"):
        work = Path(plan["work_dir"])
        if read_json(work / "reference-owner.json").get("plan_sha256") != plan_hash(plan):
            raise ValueError("参考环境目录没有对应的计划 ownership")
        side = "source" if args.command == "check-dataset" else args.side or "target"
        check_api(plan, ExternalTools(plan, work), side)
        if args.command == "check-dataset":
            require_empty_source(plan, ExternalTools(plan, work))
        print(json.dumps({"plan_sha256": plan_hash(plan), "side": side,
                          "scope_id": plan[side]["scope_id"]}))
        return
    if not args.write:
        parser.error("所有执行阶段必须显式传入 --write")
    required = {"backup": ("inventory", "source_generation", "source_export_result"),
                "restore": ("backup_root", "record", "target_plan", "runtime_registration"),
                "copy": ("backup_root", "copy_id"), "damage": ("backup_root", "artifact")}
    for name in required.get(args.command, ()):
        if getattr(args, name) is None:
            parser.error(f"当前阶段必须提供 --{name.replace('_', '-')}")
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
    name = args.command + ("-" + identifier(args.copy_id) if args.copy_id else "")
    if args.command == "damage":
        name += "-" + identifier(args.backup_root.resolve().name)
    receipt = work / f"{name}.json"
    inputs = None
    if args.command == "restore":
        inputs = getattr(args, "restore_inputs", None) or restore_inputs(
            backend, plan, args.target_plan, args.backup_root, args.record, args.runtime_registration, read_only=True)
        receipt = restore_output(backend, plan, inputs)
    started = {"command": args.command, "plan_sha256": plan_hash(plan), "started_at": now(), "status": "running"}
    if inputs is not None:
        started.update(inputs.bindings())
    write_json(receipt, started)
    try:
        if args.command == "dataset":
            timeout = dataset_timeout_seconds(plan)
            source = source_snapshot(backend)
            check_api(plan, tools, "source")
            require_empty_source(plan, tools)
            preflight = {
                "format_version": 1,
                "kind": "restore-reference-dataset-preflight",
                "plan_sha256": plan_hash(plan),
                "side": "source",
                "scope_id": plan["source"]["scope_id"],
                "actions": {"business": "empty_source_verified"},
            }
            preflight_path = work / "dataset-preflight.json"
            write_json(preflight_path, preflight)
            tools.execute([*tools.command("node"), str(backend / "scripts/restore_reference_dataset.mjs"),
                           "--plan", str(args.plan.resolve()), "--backend-dir", str(backend),
                           "--preflight", str(preflight_path), "--write"], timeout=timeout)
            result = read_json(work / "dataset/result.json")
            if read_json(preflight_path) != preflight:
                raise ValueError("数据准备预检收据在执行期间发生变化")
            if source_snapshot(backend) != source:
                raise ValueError("参考数据准备期间源码发生变化")
            result = {**result, "source": source}
        elif args.command == "backup":
            inventory = read_json(args.inventory)
            if "id" in inventory:
                raise ValueError("backup 必须直接消费共享导出的原始库存，不能手工补写 id")
            inventory = {"id": plan["id"], **inventory}
            source = source_snapshot(backend)
            if not source["clean"] or source["head"] != inventory["source_sha"]:
                raise ValueError("正式备份协调器必须是共享导出库存 source_sha 对应的干净工作树")
            result = backup(plan, tools, work, inventory, backend, args.source_generation.absolute(), args.source_export_result.absolute())
        elif args.command == "restore":
            result = restore(plan, tools, backend, inputs)
        else:
            root = args.backup_root.resolve()
            if not root.is_relative_to(work.resolve()):
                raise ValueError("参考备份根目录必须位于本次明确演练目录内")
            manifest = read_json(root / "manifest.json")
            if args.command == "copy":
                result = copy_backup(work, manifest, root, args.copy_id)
            else:
                result = damage(work, root, manifest, args.artifact, args.missing)
        write_json(receipt, {**started, "status": "completed", "completed_at": now(), "result": result}, new=False)
        return result
    except BaseException as error:
        write_json(receipt, {**started, "status": "failed", "completed_at": now(),
                             "failure": failure_diagnostic(error),
                             **({"resource_images": error.restore_evidence} if hasattr(error, "restore_evidence") else {})}, new=False)
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
    main()
