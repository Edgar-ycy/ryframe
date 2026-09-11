"""由消费者契约对同一份前端 inventory 复算来源三域及 Vite 环境文件。"""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import sys

from source_inventory import (
    FRONTEND_ENVIRONMENT_PATHS,
    build_source_domains,
    frontend_environment_files,
)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--frontend-dir", type=Path, required=True)
    arguments = parser.parse_args()
    raw = sys.stdin.buffer.read(16 * 1024 * 1024 + 1)
    if not raw or len(raw) > 16 * 1024 * 1024:
        raise ValueError("前端来源契约输入缺失或超过 16 MiB")
    value = json.loads(raw.decode("utf-8"))
    if (not isinstance(value, dict) or set(value) != {"inventories", "environment_fixtures"}
            or not isinstance(value["inventories"], list)
            or not isinstance(value["environment_fixtures"], list)):
        raise ValueError("前端来源契约输入字段无效")
    fixtures = []
    allowed = set(FRONTEND_ENVIRONMENT_PATHS[1:])
    for item in value["environment_fixtures"]:
        if (not isinstance(item, dict) or set(item) != {"path", "content"}
                or item["path"] not in allowed or not isinstance(item["content"], str)):
            raise ValueError("前端忽略环境文件契约输入无效")
        fixtures.append({
            "path": item["path"],
            "sha256": hashlib.sha256(item["content"].encode("utf-8")).hexdigest(),
        })
    result = {
        "domains": [build_source_domains(item, "frontend") for item in value["inventories"]],
        "environment_names": list(FRONTEND_ENVIRONMENT_PATHS),
        "environment_files": frontend_environment_files(arguments.frontend_dir.resolve(strict=True)),
        "environment_fixture_files": fixtures,
    }
    print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    main()
