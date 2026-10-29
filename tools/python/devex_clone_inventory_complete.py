"""合并正式逐目标库存，原始文件绑定独立于导出的完整逻辑库存。"""
from __future__ import annotations

import copy
from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import bound_file, exact


def database_image(value: dict) -> dict:
    return {**value, "tables": sorted(value["tables"], key=lambda item: item["table"]),
            "placements": sorted(value["placements"], key=lambda item: item["tenant_id"])}


def complete_inventory(runtime: dict, targets: list[dict], request: dict) -> dict:
    declared = {item["key"] for item in request["source"]["databases"]}
    databases = {}
    for target in targets:
        exact(target, {"scope_id", "control_schema_fingerprint", "tenant_schema_fingerprint", "target"})
        if any(target[field] != runtime[field] for field in
               ("scope_id", "control_schema_fingerprint", "tenant_schema_fingerprint")):
            raise ValueError("逐目标库存的 scope 或 schema 与常规库存不同")
        exact(target["target"], {"database", "preserved_tables"})
        database = target["target"]["database"]
        key = database["key"]
        if key not in declared or key in databases:
            raise ValueError("逐目标库存含未登记或重复目标")
        databases[key] = database
    if set(databases) != declared:
        raise ValueError("逐目标库存未覆盖全部登记目标")
    seen = set()
    for database in runtime["databases"]:
        key = database["key"]
        if key not in databases or key in seen or database_image(database) != database_image(databases[key]):
            raise ValueError("常规库存与逐目标实际库存不一致")
        seen.add(key)
    projections = {"biz_data_scope_owner", "biz_data_scope_dept", "biz_data_scope_projection"}
    for target in targets:
        key = target["target"]["database"]["key"]
        database = databases[key]
        # 常规库存先与原始业务部分逐项比较；逻辑复制像再纳入必须恢复的授权投影。
        databases[key] = {**database, "tables": database["tables"] + [
            row for row in target["target"]["preserved_tables"] if row["table"] in projections]}
    for key in declared - seen:
        database = databases[key]
        # 空目标保留初始化的唯一空 slot；其实际 NULL 语义由 SQL 全量关系核验确认。
        if database["placements"] or any(item["rows"] != (1 if item["table"] == "biz_tenant_target_slot" else 0)
                                         for item in database["tables"]):
            raise ValueError("常规库存遗漏了非空目标，不能补作空目标")
    return {**copy.deepcopy(runtime), "databases": [copy.deepcopy(databases[key]) for key in sorted(declared)]}


def inventory_proofs(root: Path, evidence: dict, request: dict, phase: str) -> list[Path]:
    """重验原始文件摘要和完整合成结果，不接受调用方替换的逻辑摘要。"""
    manifest = evidence["inventory-" + phase + "-targets"]
    exact(manifest, {"runtime", "targets"})
    if manifest["runtime"]["file"] != f"inventory-{phase}-runtime.json":
        raise ValueError("常规库存原始文件名不符")
    runtime_path = bound_file(root, manifest["runtime"])
    targets, paths = [], [runtime_path]
    if not isinstance(manifest["targets"], list):
        raise ValueError("逐目标库存绑定必须为列表")
    expected = {f"inventory-{phase}-target-{item['key']}.json" for item in request["source"]["databases"]}
    if len(manifest["targets"]) != len(expected) or {item["file"] for item in manifest["targets"]} != expected:
        raise ValueError("逐目标原始文件未精确覆盖登记目标")
    for binding in manifest["targets"]:
        path = bound_file(root, binding)
        target = read_json(path)
        if binding["file"] != f"inventory-{phase}-target-{target['target']['database']['key']}.json":
            raise ValueError("逐目标原始文件名与实际目标不同")
        paths.append(path)
        targets.append(target)
    combined = complete_inventory(read_json(runtime_path), targets, request)
    if combined != evidence["inventory-" + phase]:
        raise ValueError("完整库存与已绑定的原始逐目标库存不同")
    return paths
