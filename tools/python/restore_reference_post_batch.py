"""以受控 SQL 批次写入恢复夹具岗位；仅供已隔离且空源已核验的数据准备阶段使用。"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys

from devex_clone_model import linked
from restore_reference_io import ExternalTools
from restore_reference_plan import plan_hash, validate_plan

I64_MAX = 2**63 - 1


def _rows(path: Path, plan: dict) -> list[dict]:
    work = Path(plan["work_dir"]).resolve()
    expected = work / "dataset" / "post-batch.ndjson"
    if path.resolve() != expected.resolve() or linked(path) or not path.is_file():
        raise ValueError("岗位批次输入必须是当前数据集目录中的受控普通文件")
    settings = plan["dataset"]
    api_rows, total = settings["api_validation_posts"], settings["records"]
    expected_tenants = ["system", *[f"{plan['source']['scope_id']}-{index:02d}" for index in range(1, 11)]]
    counts = [total // len(expected_tenants) + (index < total % len(expected_tenants)) for index in range(len(expected_tenants))]
    values = []
    for position, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
        value = json.loads(line)
        if set(value) != {"tenant_id", "index", "code", "name", "sort"}:
            raise ValueError("岗位批次输入字段不完整或包含未知字段")
        if not isinstance(value["index"], int) or not isinstance(value["sort"], int):
            raise ValueError("岗位批次索引或排序必须为整数")
        if value["sort"] != value["index"] % 1000:
            raise ValueError("岗位批次排序必须由索引确定")
        if value["tenant_id"] not in expected_tenants:
            raise ValueError("岗位批次租户不属于当前隔离来源")
        tenant_index = expected_tenants.index(value["tenant_id"])
        if not api_rows <= value["index"] < counts[tenant_index]:
            raise ValueError("岗位批次索引超出当前租户的确定范围")
        if value["code"] != f"{plan['id']}-{value['index']}" or value["name"] != f"恢复样本{value['index']}":
            raise ValueError("岗位批次内容未由当前计划确定")
        values.append(value)
    expected_rows = total - api_rows * len(expected_tenants)
    if len(values) != expected_rows:
        raise ValueError("岗位批次行数与计划规模不一致")
    for tenant, count in zip(expected_tenants, counts, strict=True):
        indexes = [value["index"] for value in values if value["tenant_id"] == tenant]
        if indexes != list(range(api_rows, count)):
            raise ValueError("岗位批次必须完整且按每个租户的确定索引排序")
    return values


def _literal(value: str) -> str:
    return "CONVERT(0x" + value.encode("utf-8").hex() + " USING utf8mb4)"


def _maximum_id(tools: ExternalTools, database: dict) -> int:
    value = tools.mysql(database, "SELECT COALESCE(MAX(`id`), 0) FROM `sys_post`;")
    if not value.isdecimal():
        raise ValueError("岗位批次无法取得唯一的现有最大 ID")
    return int(value)


def _statement(rows: list[dict], first_id: int) -> str:
    values = []
    for offset, row in enumerate(rows):
        values.append(
            "(" + ",".join((
                str(first_id + offset), _literal(row["tenant_id"]), _literal(row["name"]),
                _literal(row["code"]), str(row["sort"]), "'1'", "NULL", "'0'",
            )) + ")"
        )
    return (
        "SET SESSION time_zone = '+00:00'; START TRANSACTION;\n"
        "INSERT INTO `sys_post` (`id`,`tenant_id`,`name`,`code`,`sort`,`status`,`remark`,`del_flag`) VALUES\n"
        + ",\n".join(values)
        + ";\nCOMMIT;\n"
    )


def prepare(plan: dict, input_path: Path) -> dict:
    rows = _rows(input_path, plan)
    batch_rows = plan["dataset"]["post_batch_rows"]
    database = next(value for value in plan["source"]["databases"] if value["kind"] == "combined")
    tools = ExternalTools(plan, Path(plan["work_dir"]))
    first_id = _maximum_id(tools, database) + 1
    if first_id <= 0 or first_id + len(rows) - 1 > I64_MAX:
        raise ValueError("岗位批次 ID 范围超过已支持的有符号 64 位范围")
    for offset in range(0, len(rows), batch_rows):
        output = tools.mysql(database, _statement(rows[offset : offset + batch_rows], first_id + offset))
        if output:
            raise ValueError("岗位批次写入不得产生未登记的 MySQL 标准输出")
    return {
        "format_version": 1,
        "kind": "restore-reference-post-batch",
        "plan_sha256": plan_hash(plan),
        "input_sha256": hashlib.sha256(input_path.read_bytes()).hexdigest(),
        "rows": len(rows),
        "batch_rows": batch_rows,
        "batches": (len(rows) + batch_rows - 1) // batch_rows,
        "first_id": str(first_id),
        "last_id": str(first_id + len(rows) - 1),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--input", type=Path, required=True)
    args = parser.parse_args()
    backend = args.backend_dir.resolve()
    plan = json.loads(args.plan.read_text(encoding="utf-8"))
    validate_plan(plan, backend)
    print(json.dumps(prepare(plan, args.input), ensure_ascii=False))


if __name__ == "__main__":
    main()
