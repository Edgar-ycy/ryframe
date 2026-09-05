"""参考恢复演练的显式资源计划与文件边界；只描述已初始化的隔离资源。"""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

from process_sockets import endpoint
from restore_build import file_digest

BUCKETS = {"uploads", "avatar", "exports", "imports", "config-packages"}
EXCLUDED_TABLES = {"ryframe_resource_ownership", "sys_backup_set", "sys_backup_resource", "sys_restore_run"}


def identifier(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,63}", value):
        raise ValueError("计划资源名称无效")
    return value


def safe_file(root: Path, relative: str, *, exists: bool = True) -> Path:
    if (not isinstance(relative, str) or not relative or "\\" in relative or ":" in relative
            or any(part in ("", ".", "..") for part in relative.split("/"))
            or any(ord(character) < 32 for character in relative)):
        raise ValueError("备份文件路径越界或格式无效")
    path = root / relative
    if path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
        raise ValueError("备份文件通过链接越界")
    if exists and not path.is_file():
        raise ValueError("备份文件缺失")
    return path


def plan_hash(plan: dict) -> str:
    return hashlib.sha256(json.dumps(plan, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def dataset_timeout_seconds(plan: dict) -> int:
    settings = plan.get("dataset")
    value = settings.get("timeout_seconds") if isinstance(settings, dict) else None
    if type(value) is not int or not 1 <= value <= 604800:
        raise ValueError("数据准备必须显式设置 1 至 604800 秒的阶段时限，独立于恢复时限")
    return value


def validate_plan(plan: dict, backend: Path) -> None:
    if plan.get("format_version") != 1:
        raise ValueError("参考演练计划版本无效")
    identifier(plan["id"])
    if "dataset" in plan:
        dataset_timeout_seconds(plan)
    local = (backend / ".local-tests").resolve()
    work = Path(plan["work_dir"])
    if not work.is_absolute() or work.resolve() == local or not work.resolve().is_relative_to(local):
        raise ValueError("演练目录必须是后端 .local-tests 下的明确独立目录")
    identities = {}
    for name in ("source", "target"):
        side = plan[name]
        if len(identifier(side["scope_id"])) > 48:
            raise ValueError("演练 scope 过长")
        endpoint(side["s3"]["endpoint"])
        for key in ("api_url", "frontend_url"):
            endpoint(side[key])
        runtime = Path(side["runtime_dir"])
        if not runtime.is_absolute() or not runtime.resolve().is_relative_to(local):
            raise ValueError("运行进程收据必须显式保存在当前 .local-tests")
        if not re.fullmatch(r"[a-z0-9-]+", side["s3"]["region"]):
            raise ValueError("S3 region 无效")
        for key in ("access_key_env", "secret_key_env"):
            if not re.fullmatch(r"[A-Z][A-Z0-9_]+", side["s3"][key]):
                raise ValueError("凭据必须通过命名环境变量提供")
        keys, databases = set(), set()
        for db in side["databases"]:
            key = identifier(db["key"])
            database = identifier(db["database"])
            uuid = identifier(db["server_uuid"])
            if (key in keys or (uuid, database.lower()) in databases or db["kind"] not in ("combined", "tenant")
                    or db["mode"] not in ("shared", "dedicated")
                    or db["kind"] == "combined" and db["mode"] != "shared"):
                raise ValueError("逻辑目标、物理数据库重复或类型无效")
            defaults = Path(db["defaults_file"])
            if not defaults.is_absolute() or not defaults.resolve().is_relative_to(local) or not defaults.is_file():
                raise ValueError("MySQL 凭据文件必须显式保存在当前 .local-tests")
            if file_digest(defaults)["sha256"] != db["defaults_sha256"]:
                raise ValueError("MySQL 连接文件与计划摘要不一致")
            keys.add(key)
            databases.add((uuid, database.lower()))
        if not keys or sum(db["kind"] == "combined" for db in side["databases"]) != 1:
            raise ValueError("参考环境必须恰有一个 combined 控制目标")
        identities[name] = keys, databases
    if (plan["source"]["scope_id"] == plan["target"]["scope_id"]
            or identities["source"][0] != identities["target"][0]
            or identities["source"][1] & identities["target"][1]):
        raise ValueError("源与恢复目标必须同逻辑键、不同 scope 且物理数据库完全分离")
    target_modes = {db["key"]: (db["kind"], db["mode"]) for db in plan["target"]["databases"]}
    if any(target_modes[db["key"]] != (db["kind"], db["mode"]) for db in plan["source"]["databases"]):
        raise ValueError("源与恢复目标的逻辑类型和共享模式必须相同")
    for name in ("mysql", "mysqldump", "aws", "node"):
        tool = plan["tools"][name]
        path = Path(tool["path"])
        if not path.is_absolute() or file_digest(path)["sha256"] != tool["sha256"]:
            raise ValueError("外部工具文件不存在或与计划摘要不一致")


def validate_inventory(plan: dict, inventory: dict) -> None:
    if inventory["scope_id"] != plan["source"]["scope_id"]:
        raise ValueError("备份清单 scope 与资源计划不一致")
    expected = {db["key"]: db for db in plan["source"]["databases"]}
    if len(inventory["databases"]) != len(expected) or {db["key"] for db in inventory["databases"]} != set(expected):
        raise ValueError("备份清单必须完整覆盖精确数据库目标")
    for db in inventory["databases"]:
        if any(db[key] != expected[db["key"]][key] for key in ("database", "server_uuid", "kind")):
            raise ValueError("备份数据库与计划中的物理身份不匹配")
        if db["shared"] != (expected[db["key"]]["mode"] == "shared"):
            raise ValueError("备份数据库共享模式与计划不匹配")
        tables = [identifier(table["table"]) for table in db["tables"]]
        if not tables or len(tables) != len(set(tables)) or EXCLUDED_TABLES & set(tables):
            raise ValueError("备份表缺失、重复或包含必须保留的目标元数据")
    if len(inventory["objects"]) != len(BUCKETS) or {item["bucket"] for item in inventory["objects"]} != BUCKETS:
        raise ValueError("备份清单必须恰好包含五个业务对象桶")
    for objects in inventory["objects"]:
        prefix = inventory["scope_id"] + "/"
        keys = [item["key"] for item in objects["entries"]]
        if (objects["prefix"] != prefix or len(keys) != len(set(keys))
                or any(not key.startswith(prefix) or key == prefix + ".ryframe-owner"
                       or any(ord(char) < 32 for char in key) for key in keys)):
            raise ValueError("对象清单越过明确 scope、重复或包含 ownership 标记")
        for entry in objects["entries"]:
            if (not isinstance(entry["bytes"], int) or entry["bytes"] <= 0
                    or not re.fullmatch(r"[a-f0-9]{64}", entry["sha256"])):
                raise ValueError("对象清单大小或校验摘要无效")


def verify_artifacts(root: Path, manifest: dict) -> None:
    paths = set()
    required = {f"db:{db['key']}" for db in manifest["databases"]} | {f"objects:{bucket}" for bucket in BUCKETS}
    resources = set()
    for artifact in manifest["artifacts"]:
        relative = artifact["relative_path"]
        if relative in paths or artifact["resource"] not in required:
            raise ValueError("备份产物重复或引用未知目标")
        actual = file_digest(safe_file(root, relative))
        if any(artifact[key] != value for key, value in actual.items()) or actual["bytes"] <= 0:
            raise ValueError("备份文件大小或摘要被篡改")
        paths.add(relative)
        resources.add(artifact["resource"])
    if resources != required:
        raise ValueError("备份产物未覆盖全部数据库和对象范围")
