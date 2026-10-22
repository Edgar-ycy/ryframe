"""复算来源三域并实际核验前端 v3 文件配置、身份及完整生产产物。"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import sys

from source_inventory import build_source_domains
from restore_frontend_paths import json_object, ordinary
from restore_frontend_build import validate_frontend_build


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--frontend-dir", type=Path, required=True)
    arguments = parser.parse_args()
    raw = sys.stdin.buffer.read(16 * 1024 * 1024 + 1)
    if not raw or len(raw) > 16 * 1024 * 1024:
        raise ValueError("前端来源契约输入缺失或超过 16 MiB")
    root = ordinary(str(arguments.frontend_dir), directory=True)
    value = json_object(raw, "前端来源契约输入")
    if (not isinstance(value, dict)
            or set(value) != {"frontend_inventories", "backend_inventories"}
            or not isinstance(value["frontend_inventories"], list)
            or not isinstance(value["backend_inventories"], list)):
        raise ValueError("前端来源契约输入字段无效")
    result = {
        "frontend_domains": [build_source_domains(item, "frontend")
                             for item in value["frontend_inventories"]],
        "backend_domains": [build_source_domains(item, "backend")
                            for item in value["backend_inventories"]],
    }
    # 旧环境协议没有有效前端接线；本入口必须运行实际 validator，不能仅对照字段。
    _inventory, receipt = validate_frontend_build(root)
    result["frontend_build"] = {
        "configuration": receipt.value["build"]["configuration"],
        "identity": receipt.value["build"]["identity"],
        "files": receipt.value["files"],
    }
    encoded = json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")).encode("utf-8")
    sys.stdout.buffer.write(encoded + b"\n")


if __name__ == "__main__":
    main()
