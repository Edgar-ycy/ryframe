"""开发性能复制的离线计划模型；校验导出证据，不授予或执行资源写入。"""
from __future__ import annotations

import os
from pathlib import Path
import re

from artifact_digests import filesystem_path
from devex_clone_job_relations import validate_job_relations
from devex_clone_rows import EXCLUDED, reject_physical, rows, schema_catalog, validate_state
from devex_clone_schedule import inspect_schedule, schedule_actions
from process_sockets import endpoint
from restore_build import file_digest
from restore_reference_io import copy_object_metadata
from restore_reference_plan import BUCKETS, plan_hash, safe_file


def exact(value: dict, fields: set[str]) -> None:
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError("开发复制字段缺失或包含未知字段")


def digest(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[a-f0-9]{64}", value):
        raise ValueError("必须绑定真实 SHA-256")
    return value


def schema_fingerprints(value: dict) -> dict:
    """保持实际编译契约：控制基线为 64 位指纹，租户目录为 SHA-256。"""
    result = {}
    for field, size in (("control_schema_fingerprint", 16), ("tenant_schema_fingerprint", 64)):
        fingerprint = value.get(field)
        if not isinstance(fingerprint, str) or re.fullmatch(rf"[a-f0-9]{{{size}}}", fingerprint) is None:
            raise ValueError("编译 schema 指纹格式与当前控制库或租户目录契约不同")
        result[field] = fingerprint
    return result


def name(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{2,47}", value):
        raise ValueError("开发复制名称无效")
    return value


def linked(path: Path) -> bool:
    native = filesystem_path(path)
    return os.path.islink(native) or (os.path.exists(native)
                                      and bool(getattr(os.lstat(native), "st_file_attributes", 0) & 0x400))


def local_path(backend: Path, filename: str, *, new=False) -> Path:
    root = backend.resolve() / ".local-tests"
    path = Path(filename)
    if not path.is_absolute() or path == root or not path.is_relative_to(root):
        raise ValueError("复制证据必须使用当前后端 .local-tests 内的绝对路径")
    cursor = root
    for part in (".", *path.relative_to(root).parts):
        cursor /= part
        if linked(cursor) or not cursor.resolve().is_relative_to(root.resolve()):
            raise ValueError("复制证据路径包含链接或越界")
    if new and path.exists():
        raise ValueError("新计划不得覆盖已有文件")
    return path


def bound_file(root: Path, binding: dict) -> Path:
    exact(binding, {"file", "bytes", "sha256"})
    digest(binding["sha256"])
    if type(binding["bytes"]) is not int or binding["bytes"] <= 0:
        raise ValueError("证据大小无效")
    path = safe_file(root, binding["file"])
    cursor = root
    for part in path.relative_to(root).parts:
        cursor /= part
        if linked(cursor):
            raise ValueError("证据文件经过链接")
    if file_digest(path) != {key: binding[key] for key in ("bytes", "sha256")}:
        raise ValueError("复制证据已变化或未完整写入")
    return path


def side(value: dict, *, target=False) -> dict:
    exact(value, {"scope_id", "object_endpoint", "databases"} | ({"state", "ever_started"} if target else set()))
    scope = name(value["scope_id"])
    endpoint(value["object_endpoint"])
    if target and (value["state"] != "new_initialized" or value["ever_started"] is not False):
        raise ValueError("目标必须声明为本次新初始化且从未运行的资源")
    if not isinstance(value["databases"], list) or not value["databases"]:
        raise ValueError("必须明确列出源和目标数据库")
    keys, physical = {}, set()
    for db in value["databases"]:
        exact(db, {"key", "kind", "mode", "server_uuid", "database", "schema_sha256", "ownership"})
        name(db["key"])
        if (not re.fullmatch(r"[a-z0-9_]{1,64}", db["database"]) or
                not re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", db["server_uuid"]) or
                db["kind"] not in {"combined", "tenant"} or db["mode"] not in {"shared", "dedicated"} or
                (db["kind"] == "combined" and db["mode"] != "shared")):
            raise ValueError("数据库物理身份或类型无效")
        digest(db["schema_sha256"])
        identity = db["server_uuid"], db["database"].lower()
        if db["key"] in keys or identity in physical:
            raise ValueError("数据库键或物理身份重复")
        physical.add(identity)
        keys[db["key"]] = db
        kinds = ["control", "tenant-data"] if db["kind"] == "combined" else ["tenant-data"]
        expected = [{"resource_kind": kind, "scope_id": scope, "marker": f"ryframe-owner:v1:{scope}:{kind}"} for kind in kinds]
        if db["ownership"] != expected:
            raise ValueError("必须完整声明各侧自己的精确 ownership，不能复制来源 owner")
    if sum(db["kind"] == "combined" for db in keys.values()) != 1:
        raise ValueError("复制必须恰有一个完整 combined 控制库")
    return keys


def validate_input(value: dict, backend: Path) -> tuple[dict, dict, dict, dict]:
    exact(value, {"format_version", "copy_id", "copy_stage", "target_schedule_actions", "app_env", "artifact_root", "source_snapshot", "source", "target", "evidence", "databases", "objects"})
    if value["format_version"] != 1 or value["app_env"] != "test":
        raise ValueError("开发复制仅接受 test 环境当前结构")
    name(value["copy_id"])
    source = value["source_snapshot"]
    exact(source, {"head", "clean", "worktree_sha256"})
    if not re.fullmatch(r"[a-f0-9]{40}", source["head"]) or type(source["clean"]) is not bool:
        raise ValueError("必须准确记录开发源码快照与 dirty 状态")
    digest(source["worktree_sha256"])
    old, new = side(value["source"]), side(value["target"], target=True)
    if value["source"]["scope_id"] == value["target"]["scope_id"] or set(old) != set(new):
        raise ValueError("必须同逻辑目标、不同物理 scope")
    if {(db["server_uuid"], db["database"]) for db in old.values()} & {(db["server_uuid"], db["database"]) for db in new.values()}:
        raise ValueError("新目标不能包含任何来源数据库")
    for key, db in old.items():
        if any(db[field] != new[key][field] for field in ("kind", "mode", "schema_sha256")):
            raise ValueError("两侧 schema、逻辑目标和模式必须相同")
    local_path(backend, value["artifact_root"])
    control, tenant = schema_catalog(backend)
    return old, new, control, tenant


def object_plan(value: dict, root: Path) -> tuple[list, dict]:
    objects, found = [], {}
    if not isinstance(value["objects"], list) or len(value["objects"]) != len(BUCKETS):
        raise ValueError("必须完整列出五类对象桶，包括空桶")
    seen = set()
    for bucket in value["objects"]:
        exact(bucket, {"bucket", "entries"})
        name_ = bucket["bucket"]
        if name_ not in BUCKETS or name_ in seen or not isinstance(bucket["entries"], list):
            raise ValueError("对象桶缺失或重复")
        seen.add(name_)
        for item in bucket["entries"]:
            exact(item, {"key", "artifact", "metadata"})
            prefix = value["source"]["scope_id"] + "/"
            key = item["key"]
            logical = key.removeprefix(prefix) if isinstance(key, str) else ""
            target_key = value["target"]["scope_id"] + "/" + logical
            if (not isinstance(key, str) or not key.startswith(prefix) or not logical or
                    logical == ".ryframe-owner" or "\\" in logical or
                    any(part in {"", ".", ".."} for part in logical.split("/")) or
                    any(ord(char) < 32 or ord(char) == 127 for char in logical)
                    or len(key.encode("utf-8")) > 1024 or len(target_key.encode("utf-8")) > 1024
                    or (name_, logical) in found):
                raise ValueError("对象 key 越过精确来源前缀、包含 owner 或重复")
            metadata = copy_object_metadata(item["metadata"])
            bound_file(root, item["artifact"])
            found[name_, logical] = item["artifact"]
            objects.append({"bucket": name_, "source_key": key,
                            "target_key": target_key,
                            "artifact": item["artifact"], "metadata": metadata})
    return objects, found


def state() -> dict:
    return {"tenants": set(), "placements": {}, "fences": {}, "slots": {}, "jobs": {}, "attempts": set(),
            "schedules": {}, "executions": [], "files": []}


def collect(state_: dict, table: str, row: dict, key: str) -> None:
    if table == "sys_tenant":
        state_["tenants"].add(row["tenant_id"])
    elif table in {"sys_tenant_data_placement", "biz_tenant_fence"}:
        field = "current_target_key" if table == "sys_tenant_data_placement" else "target_key"
        values = row[field], str(row["placement_generation"]), row["switch_token"]
        group = "placements" if table == "sys_tenant_data_placement" else "fences"
        if row["state"] != "active" or row["placement_generation"] <= 0 or not row["switch_token"] or row["tenant_id"] in state_[group]:
            raise ValueError("租户 placement/fence 非 active、重复或缺少代次令牌")
        if group == "fences" and row[field] != key:
            raise ValueError("fence 与当前物理逻辑目标不一致")
        state_[group][row["tenant_id"]] = values
    elif table == "biz_tenant_target_slot":
        if key in state_["slots"] or row["slot_id"] != 1:
            raise ValueError("独立目标 slot 重复或不是固定槽位")
        state_["slots"][key] = (row["tenant_id"], str(row["placement_generation"]), row["switch_token"])
    elif table == "sys_background_job":
        if str(row["id"]) in state_["jobs"]:
            raise ValueError("完整后台任务 ID 重复")
        state_["jobs"][str(row["id"])] = row
    elif table == "sys_background_job_attempt":
        state_["attempts"].add(str(row["job_id"]))
    elif table == "sys_job_schedule_execution" and row["background_job_id"] is not None:
        state_["executions"].append(row)
    elif table == "sys_job_schedule":
        if str(row["id"]) in state_["schedules"]:
            raise ValueError("完整调度计划 ID 重复")
        state_["schedules"][str(row["id"])] = row
    elif table == "sys_file" and row["del_flag"] == "0":
        state_["files"].append((row["bucket"], row["storage_path"], row["file_size"], row["file_sha256"]))


def validate_relations(state_: dict, databases: dict, objects: dict) -> None:
    if state_["tenants"] != set(state_["placements"]) or state_["placements"] != state_["fences"]:
        raise ValueError("租户、placement 与 fence 必须完整且代次令牌一致")
    for tenant, (key, _, _) in state_["placements"].items():
        if key not in databases:
            raise ValueError("placement 引用未登记的目标")
    for key, db in databases.items():
        residents = [(tenant, generation, token) for tenant, (target, generation, token) in state_["placements"].items() if target == key]
        slot = state_["slots"].get(key)
        if db["mode"] == "dedicated":
            if len(residents) > 1 or slot != (residents[0] if residents else (None, "None", None)):
                raise ValueError("dedicated slot 与唯一驻留租户不一致")
        elif slot != (None, "None", None):
            raise ValueError("共享目标必须保留当前基线初始化的空 slot")
    validate_job_relations(state_)
    for bucket, key, size, sha in state_["files"]:
        artifact = objects.get((bucket, key))
        if not artifact or artifact["bytes"] != size or artifact["sha256"] != sha:
            raise ValueError("有效文件的逻辑 key、字节数或内容摘要与对象清单不一致")


def declared_tenants(value: dict, root: Path, old: dict, control: dict) -> set[str]:
    combined = next(key for key, db in old.items() if db["kind"] == "combined")
    dumps = [dump for dump in value["databases"] if dump.get("key") == combined]
    if len(dumps) != 1:
        raise ValueError("必须恰有一个完整控制库转储")
    declared = set()
    for _, row in rows(bound_file(root, dumps[0]["artifact"]), control, only_table="sys_tenant"):
        tenant = row["tenant_id"]
        ambiguous = {value["source"]["scope_id"], *[db["database"] for db in old.values()]}
        if (not isinstance(tenant, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", tenant)
                or tenant in declared or tenant in ambiguous):
            raise ValueError("来源逻辑租户标识无效或重复")
        declared.add(tenant)
    if "system" not in declared:
        raise ValueError("完整来源必须包含系统租户")
    return declared


def database_plan(value: dict, root: Path, old: dict, new: dict, control: dict, tenant: dict, objects: dict) -> tuple[list, list]:
    if not isinstance(value["databases"], list) or len(value["databases"]) != len(old):
        raise ValueError("必须完整列出每个数据库的数据转储")
    seen, result, observed = set(), [], state()
    actions, pending = schedule_actions(value), []
    logical_tenants = declared_tenants(value, root, old, control)
    forbidden = [value["source"]["scope_id"], value["source"]["object_endpoint"], *[db["database"] for db in old.values()]]
    for dump in value["databases"]:
        exact(dump, {"key", "tables", "artifact"})
        key = dump["key"]
        if key not in old or key in seen:
            raise ValueError("数据转储逻辑目标重复或未知")
        seen.add(key)
        catalog = control | tenant if old[key]["kind"] == "combined" else tenant
        if not isinstance(dump["tables"], dict) or set(dump["tables"]) != set(catalog):
            raise ValueError("白名单必须完整覆盖当前业务表，排除 owner、迁移和备份恢复元数据")
        if any(type(count) is not int or count < 0 for count in dump["tables"].values()):
            raise ValueError("表行数必须明确且非负")
        counts = dict.fromkeys(catalog, 0)
        for table, row in rows(bound_file(root, dump["artifact"]), catalog):
            if row.get("tenant_id") is not None and row["tenant_id"] not in logical_tenants:
                raise ValueError("数据引用未登记的逻辑租户")
            reject_physical(row, forbidden, logical_tenants)
            if table == "sys_job_schedule":
                action = inspect_schedule(row, actions)
                if action:
                    pending.append({**action, "target_scope_id": value["target"]["scope_id"]})
            else:
                validate_state(table, row)
            counts[table] += 1
            collect(observed, table, row, key)
        if counts != dump["tables"]:
            raise ValueError("导出实际行数不符合完整表清单")
        result.append({"key": key, "source_database": old[key]["database"], "target_database": new[key]["database"],
                       "tables": counts, "artifact": dump["artifact"], "preserve_target_ownership": True})
    validate_relations(observed, old, objects)
    if actions:
        raise ValueError("目标调度处置清单包含未命中的启用源记录")
    return result, pending


def create_plan(value: dict, backend: Path) -> dict:
    old, new, control, tenant = validate_input(value, backend)
    root = local_path(backend, value["artifact_root"])
    if not root.is_dir():
        raise ValueError("导出证据根目录不存在")
    exact(value["evidence"], {"source_stopped", "target_initialized", "target_stopped"})
    for binding in value["evidence"].values():
        bound_file(root, binding)
    objects, index = object_plan(value, root)
    databases, pending = database_plan(value, root, old, new, control, tenant, index)
    bindings = [*value["evidence"].values(), *[item["artifact"] for item in value["databases"]],
                *[item["artifact"] for bucket in value["objects"] for item in bucket["entries"]]]
    for binding in bindings:
        bound_file(root, binding)
    result = {"format_version": 1, "kind": "devex-clone-offline-plan", "copy_id": value["copy_id"], "copy_stage": value["copy_stage"],
              "input_sha256": plan_hash(value), "source_snapshot": value["source_snapshot"],
              "source_scope": value["source"]["scope_id"], "target_scope": value["target"]["scope_id"],
              "catalog_sha256": plan_hash({"control": control, "tenant": tenant}),
              "databases": databases, "objects": objects, "evidence": value["evidence"],
              "excluded_tables": sorted(EXCLUDED), "status": "offline_verified_pending_target_actions" if pending else "offline_verified",
              "pending_target_actions": pending, "target_ready": False, "worker_must_remain_stopped": True, "execution_authorized": False,
              "live_preconditions": "离线仅绑定已提供证据字节；执行前仍须重新验证停止状态、新目标创建事实、UUID、ownership、完整schema及源未变化；该计划本身不授予写入权限"}
    return {**result, "plan_sha256": plan_hash(result)}
