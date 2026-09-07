"""从已审阅模板派生新的隔离参考夹具计划，不创建任何资源。"""

from __future__ import annotations

import argparse
import copy
from datetime import UTC, datetime
import json
from pathlib import Path
import re

from devex_clone_capture import read_json, write_json
from devex_clone_model import linked, local_path
from devex_clone_target_binding import KEYS, validate_review
from reference_fixture_environment import _fixture
from restore_build import file_digest
from source_inventory import snapshot


SIDES = ("seed", "base", "candidate")


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("审阅续签输入必须是受控普通文件")
    return path, read_json(path)


def _identifier(value: str) -> str:
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,31}", value):
        raise ValueError("新审阅计划 ID 只能使用小写字母、数字和连字符")
    return value


def _port(value: int, label: str) -> int:
    if not 1024 <= value <= 65533:
        raise ValueError(f"{label} 端口基数必须在 1024 至 65533 之间")
    return value


def _fixture_execution(backend: Path, fixture_path: Path) -> tuple[Path, dict, Path]:
    receipt_path, fixture = _read(backend, fixture_path)
    _fixture(fixture)
    execution = Path(fixture["paths"]["backend"])
    generated = fixture.get("generated", {}).get("backend")
    if (not isinstance(generated, dict) or not execution.is_dir()
            or snapshot(execution)[0] != generated):
        raise ValueError("新 Device 工作树与生成快照不一致")
    return receipt_path, fixture, execution


def _future_root(backend: Path, execution: Path, value: Path) -> Path:
    requested = value if value.is_absolute() else backend / value
    root = local_path(backend, str(requested), new=True)
    allowed = execution / ".local-tests/reference-fixture"
    if (root.exists() or linked(root) or not root.is_relative_to(allowed)
            or not root.parent.is_dir()):
        raise ValueError("新运行根目录必须是 Device 参考夹具目录下尚不存在的直接子目录")
    return root


def _scope(template: dict, side: str, plan_id: str, execution: Path, future_root: Path,
           api_port: int, worker_port: int, frontend_port: int, rustfs_api_port: int,
           redis_port: int) -> dict:
    current = template["scopes"][side]
    scope_id = f"fixture-{side}-{plan_id}"
    database_token = plan_id.replace("-", "_")
    secrets = execution / ".local-tests/reference-fixture/secrets/mysql-client.cnf"
    databases = []
    for key in ("shared-control", "shared", "dedicated-a", "dedicated-b"):
        original = next(item for item in current["databases"] if item["key"] == key)
        suffix = key.replace("-", "_")
        databases.append({
            "key": key,
            "database": f"ryframe_fx_{database_token}_{side}_{suffix}",
            "mode": KEYS[key][1],
            "expected_server_uuid": original["expected_server_uuid"],
            "connection_file": str(secrets),
            "host": original["host"],
            "port": original["port"],
        })
    return {
        "scope_id": scope_id,
        "runtime_dir": str(future_root / f"runtime-{side}"),
        "backend_dir": str(execution),
        "identity_ledger": str(future_root / f"identities-{side}"),
        "api_url": f"http://127.0.0.1:{api_port}",
        "worker_ready_url": f"http://127.0.0.1:{worker_port}/readyz",
        "frontend_url": f"http://127.0.0.1:{frontend_port}",
        "objects": {"endpoint": f"http://127.0.0.1:{rustfs_api_port}", "region": current["objects"]["region"]},
        "redis": {
            "url": f"redis://127.0.0.1:{redis_port}/0",
            "namespace": f"ryframe:{{{scope_id}}}:",
            "ownership_key": f"ryframe:{{{scope_id}}}:.ryframe-owner",
            "ownership_value": f"ryframe-owner:v1:{scope_id}:redis",
        },
        "databases": databases,
    }


def renew(backend: Path, template_path: Path, fixture_path: Path, future_root: Path, plan_id: str,
          api_port: int, worker_port: int, frontend_port: int, rustfs_api_port: int,
          rustfs_console_port: int, redis_port: int) -> dict:
    """派生未预检的新审阅计划；调用方必须显式保存并重新预检。"""
    backend = backend.resolve(strict=True)
    template_file, template = _read(backend, template_path)
    validate_review(template)
    if any(template["reference"][role]["databases"] for role in ("source", "protected_target")):
        raise ValueError("续签模板不得复用历史来源或受保护目标数据库")
    plan_id = _identifier(plan_id)
    api_port, worker_port, frontend_port, rustfs_api_port, rustfs_console_port, redis_port = (
        _port(api_port, "API"), _port(worker_port, "Worker"), _port(frontend_port, "前端"),
        _port(rustfs_api_port, "RustFS API"), _port(rustfs_console_port, "RustFS 控制台"), _port(redis_port, "Redis"))
    all_ports = ([base + offset for base in (api_port, worker_port, frontend_port) for offset in range(len(SIDES))]
                 + [rustfs_api_port, rustfs_console_port, redis_port])
    if len(set(all_ports)) != len(all_ports):
        raise ValueError("三侧 API、Worker 与前端端口不能重叠")
    fixture_file, fixture, execution = _fixture_execution(backend, fixture_path)
    root = _future_root(backend, execution, future_root)
    revised = copy.deepcopy(template)
    revised.update({
        "id": f"fixture-{plan_id}",
        "created_at": datetime.now(UTC).isoformat(),
        "future_root": str(root),
        "workspace": str(backend),
        "sources": {
            "backend_head": fixture["sources"]["backend"]["head"],
            "frontend_head": fixture["sources"]["frontend"]["head"],
            "renewed_from": {"path": str(template_file), **file_digest(template_file)},
        },
        "ready_for_execution": False,
        "scopes": {
            side: _scope(revised, side, plan_id, execution, root,
                         api_port + index, worker_port + index, frontend_port + index,
                         rustfs_api_port, redis_port)
            for index, side in enumerate(SIDES)
        },
    })
    revised.pop("preflight", None)
    revised["services"]["rustfs"].update(
        api=f"http://127.0.0.1:{rustfs_api_port}", console=f"http://127.0.0.1:{rustfs_console_port}",
        data_dir=str(root / "rustfs"))
    revised["services"]["rustfs"].pop("scope_id", None)
    revised["services"]["rustfs"].pop("process_receipt", None)
    revised["services"]["redis"].update(endpoint=f"127.0.0.1:{redis_port}", directory=str(root / "redis"))
    structural = copy.deepcopy(revised)
    structural["ready_for_execution"] = True
    validate_review(structural)
    return revised


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--template", type=Path, required=True)
    parser.add_argument("--fixture", type=Path, required=True)
    parser.add_argument("--future-root", type=Path, required=True)
    parser.add_argument("--id", required=True)
    parser.add_argument("--api-port", type=int, required=True)
    parser.add_argument("--worker-port", type=int, required=True)
    parser.add_argument("--frontend-port", type=int, required=True)
    parser.add_argument("--rustfs-api-port", type=int, required=True)
    parser.add_argument("--rustfs-console-port", type=int, required=True)
    parser.add_argument("--redis-port", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if not args.write:
        parser.error("续签审阅计划需要显式 --write")
    backend = args.backend_dir.resolve()
    result = renew(backend, args.template, args.fixture, args.future_root, args.id,
                   args.api_port, args.worker_port, args.frontend_port, args.rustfs_api_port,
                   args.rustfs_console_port, args.redis_port)
    requested_output = args.output if args.output.is_absolute() else backend / args.output
    output = local_path(backend, str(requested_output), new=True)
    if not output.parent.is_dir():
        raise ValueError("新审阅计划输出父目录不存在")
    write_json(output, result)
    print(json.dumps({"status": "renewed", "ready_for_execution": False, "remote_writes": 0}, ensure_ascii=False))


if __name__ == "__main__":
    main()
