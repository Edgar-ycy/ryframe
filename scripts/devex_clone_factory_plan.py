"""由已核对导出和实际库存组装离线清单；本模块不授予写入或启动权限。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_model import create_plan, digest
from devex_clone_transfer import DatabaseObservation


def declared_side(config: dict, observations: dict[str, DatabaseObservation]) -> dict:
    databases = config["databases"]
    if len(databases) != len(observations) or {item["key"] for item in databases} != set(observations):
        raise ValueError("实际库存必须完整覆盖明确侧的每个目标")
    declared = []
    for database in databases:
        observed = observations[database["key"]]
        expected = {"kind": "database", "scope_id": config["scope_id"],
                    "server_uuid": database["server_uuid"], "database": database["database"]}
        if not isinstance(observed, DatabaseObservation) or observed.resource != expected:
            raise ValueError("组装计划时实际库存的物理身份不符")
        declared.append({**{field: database[field] for field in ("key", "kind", "mode", "server_uuid", "database")},
                         "schema_sha256": observed.schema_sha256, "ownership": copy.deepcopy(list(observed.ownership))})
    return {"scope_id": config["scope_id"], "object_endpoint": config["s3"]["endpoint"], "databases": declared}


def compose_declaration(exported: dict, source: dict[str, DatabaseObservation], target: dict,
                        initialized: dict[str, DatabaseObservation], evidence: dict, *, copy_id: str, stage: str) -> dict:
    """仅组装完整当前声明；调用方必须使用离线计划验证后才可执行。"""
    fingerprint = exported["worktree_fingerprint"]
    if not isinstance(fingerprint, str) or not fingerprint.startswith("sha256:"):
        raise ValueError("源导出缺少实际 DevEx 工作区指纹")
    source_snapshot = {field: exported["source_snapshot"][field] for field in ("head", "clean")}
    source_snapshot["worktree_sha256"] = digest(fingerprint.removeprefix("sha256:"))
    value = {"format_version": 1, "copy_id": copy_id, "copy_stage": stage, "app_env": "test",
             "target_schedule_actions": [{"action": "disable_schedule_via_api", **item}
                                         for item in exported["enabled_system_schedule_rows"]],
             "artifact_root": exported["artifact_root"], "source_snapshot": source_snapshot,
             "source": declared_side(exported["source"], source),
             "target": {**declared_side(target, initialized), "state": "new_initialized", "ever_started": False},
             "evidence": copy.deepcopy(evidence), "databases": copy.deepcopy(exported["databases"]),
             "objects": [{"bucket": bucket["bucket"], "entries": [
                 {field: copy.deepcopy(item[field]) for field in ("key", "artifact", "metadata")}
                 for item in bucket["entries"]]} for bucket in exported["objects"]]}
    return value


def declaration(backend: Path, exported: dict, source: dict[str, DatabaseObservation], target: dict,
                initialized: dict[str, DatabaseObservation], evidence: dict, *, copy_id: str, stage: str) -> dict:
    """库存由 live factory 提供；末尾仍执行完整 SQL/对象/关系/调度的离线验证。"""
    value = compose_declaration(exported, source, target, initialized, evidence, copy_id=copy_id, stage=stage)
    create_plan(value, backend)
    return value
