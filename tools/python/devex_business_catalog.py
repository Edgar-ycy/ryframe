"""只读受管业务目录；完整解析与校验统一由原生业务 loader 执行。"""
from pathlib import Path

from restore_reference_plan import _native


def business_tables(backend: Path) -> list[dict]:
    return _native("business-catalog", {"backend": str(backend)})["tables"]


def business_columns(backend: Path) -> dict:
    columns = _native("business-catalog", {"backend": str(backend)})["columns"]
    return {table: dict(pairs) for table, pairs in columns.items()}


def business_ledger_columns(backend: Path) -> dict:
    return dict(_native("business-ledger-columns", {"backend": str(backend)}))
