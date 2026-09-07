"""参考环境辅助：计划、已有数据检查、数据准备和备份恢复；所有写步骤要求 --write。"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import shutil
from pathlib import Path

from full_stack_process import process_identity, read_process
from process_sockets import verify_listener
from restore_build import file_digest, source_snapshot
from restore_reference_io import ExternalTools, object_index, validate_dump
from restore_reference_plan import (dataset_timeout_seconds, identifier, plan_hash, safe_file,
                                    scope_identifier, validate_inventory, validate_plan,
                                    verify_artifacts)
from restore_runtime import read_json
from restore_source import verify_stopped_source


def now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def write_json(path: Path, value: dict, *, new=True) -> None:
    with path.open("x" if new else "w", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


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
           backend: Path, source_runtime: Path, source_quiescence: Path) -> dict:
    validate_inventory(plan, inventory)
    source_digests = verify_stopped_source(backend, plan, inventory, source_runtime, source_quiescence)
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
    if verify_stopped_source(backend, plan, inventory, source_runtime, source_quiescence) != source_digests:
        raise ValueError("备份期间来源运行证明被替换")
    write_json(root / "manifest.json", manifest)
    return {"manifest": str(root / "manifest.json"), "backup_root": str(root), "artifacts": len(artifacts),
            **source_digests}


def validate_restore_record(plan: dict, manifest: dict, record: dict) -> None:
    target = plan["target"]
    restore_plan = record["plan"]
    identifier(restore_plan["id"])
    identifier(restore_plan["backup_id"])
    scope_identifier(restore_plan["scope_id"])
    expected = {(db["key"], db["key"], db["server_uuid"], db["database"]) for db in target["databases"]}
    actual = {(db["source_key"], db["target_key"], db["server_uuid"], db["database"])
              for db in restore_plan["databases"]}
    if (record["status"] != "running" or restore_plan["backup_id"] != manifest["id"]
            or restore_plan["backup_id"] != plan["id"]
            or restore_plan["scope_id"] != target["scope_id"] or actual != expected
            or len(restore_plan["databases"]) != len(expected)
            or restore_plan["object_endpoint"] != target["s3"]["endpoint"]
            or restore_plan["object_prefix"] != target["scope_id"] + "/"):
        raise ValueError("还原必须绑定 restore-begin 的运行中记录和相同精确目标")
    started = dt.datetime.fromisoformat(record["started_at"].replace("Z", "+00:00"))
    elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds()
    if not 0 <= elapsed <= 3600:
        raise ValueError("恢复操作尚未开始、时钟回退或已超过 60 分钟")


def restore(plan: dict, tools: ExternalTools, root: Path, manifest: dict, record: dict) -> dict:
    validate_inventory(plan, manifest)
    validate_restore_record(plan, manifest, record)
    verify_artifacts(root, manifest)
    # 全部 SQL 与对象索引先校验；任一缺失或篡改不能留下部分目标写入。
    for db in manifest["databases"]:
        validate_dump(safe_file(root, f"databases/{db['key']}.sql"), {table["table"] for table in db["tables"]})
    indices = {objects["bucket"]: object_index(root, objects, manifest["artifacts"]) for objects in manifest["objects"]}
    require_stopped(plan, "target")
    tools.verify_databases("target")
    tools.verify_objects("target")
    for db in manifest["databases"]:
        target = next(value for value in plan["target"]["databases"] if value["key"] == db["key"])
        tools.restore_database(target, [table["table"] for table in db["tables"]], safe_file(root, f"databases/{db['key']}.sql"))
    for bucket, index in indices.items():
        for entry in index["entries"]:
            key = plan["target"]["scope_id"] + "/" + entry["key"].removeprefix(plan["source"]["scope_id"] + "/")
            tools.aws("target", "put-object", bucket, key, safe_file(root, entry["file"]), content_type=entry["content_type"])
    require_stopped(plan, "target")
    return {"restore_id": record["plan"]["id"], "status": "external_copy_completed",
            "next": "执行 restore-verify-data；外部复制完成不代表恢复成功"}


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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("plan", "check-dataset", "check-existing", "dataset", "backup", "restore", "copy", "damage"))
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--inventory", type=Path)
    parser.add_argument("--source-runtime", type=Path, help="backup 必须绑定停止前真实干净构建与源侧已有数据复验的运行证明")
    parser.add_argument("--source-quiescence", type=Path, help="backup 必须绑定采集清单前观察到同代次进程已停止的收据")
    parser.add_argument("--backup-root", type=Path)
    parser.add_argument("--record", type=Path)
    parser.add_argument("--copy-id")
    parser.add_argument("--artifact")
    parser.add_argument("--missing", action="store_true")
    parser.add_argument("--side", choices=("source", "target"),
                        help="仅 check-existing 可指定检查侧，默认 target；不改变数据准备或恢复目标")
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.side is not None and args.command != "check-existing":
        parser.error("--side 仅用于 check-existing；数据准备固定 source，恢复固定 target")
    if (args.source_runtime is not None or args.source_quiescence is not None) and args.command != "backup":
        parser.error("来源运行与停止观察收据仅用于 backup")
    backend, plan = args.backend_dir.resolve(), read_json(args.plan)
    validate_plan(plan, backend)
    if args.command == "plan":
        print(json.dumps({"id": plan["id"], "plan_sha256": plan_hash(plan), "work_dir": plan["work_dir"],
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
    required = {"backup": ("inventory", "source_runtime", "source_quiescence"), "restore": ("backup_root", "record"),
                "copy": ("backup_root", "copy_id"), "damage": ("backup_root", "artifact")}
    for name in required.get(args.command, ()):
        if getattr(args, name) is None:
            parser.error(f"当前阶段必须提供 --{name.replace('_', '-')}")
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
    started = {"command": args.command, "plan_sha256": plan_hash(plan), "started_at": now(), "status": "running"}
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
            source = source_snapshot(backend)
            if not source["clean"] or source["head"] != inventory["source_sha"]:
                raise ValueError("正式备份必须绑定当前精确干净源码；候选数据集不能冒充发布来源")
            result = backup(plan, tools, work, inventory, backend, args.source_runtime.resolve(), args.source_quiescence.resolve())
        else:
            root = args.backup_root.resolve()
            if not root.is_relative_to(work.resolve()):
                raise ValueError("参考备份根目录必须位于本次明确演练目录内")
            manifest = read_json(root / "manifest.json")
            if args.command == "restore":
                result = restore(plan, tools, root, manifest, read_json(args.record))
            elif args.command == "copy":
                result = copy_backup(work, manifest, root, args.copy_id)
            else:
                result = damage(work, root, manifest, args.artifact, args.missing)
        write_json(receipt, {**started, "status": "completed", "completed_at": now(), "result": result}, new=False)
        return result
    except Exception as error:
        write_json(receipt, {**started, "status": "failed", "completed_at": now(), "failure": str(error)[:1000]}, new=False)
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
