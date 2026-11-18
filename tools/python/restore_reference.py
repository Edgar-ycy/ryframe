"""离线恢复夹具共用的参考文件操作。"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import json
import subprocess
from pathlib import Path

from restore_build import file_digest
from native_s3_client import _invoke, _read_response


def _coordinate(operation: str, arguments: dict, *, write=False):
    response, _ = _read_response(*_invoke(
        "__source-evidence", {"operation": "reference-" + operation,
            "arguments": arguments, "write": write}, subprocess.run, may_write=False,
    ))
    return response["value"]


def now() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).isoformat()


def write_json(path: Path, value: dict, *, new=True) -> None:
    with path.open("x" if new else "w", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def work_directory(plan: dict) -> Path:
    return Path(_coordinate("work-directory", {"plan": plan}, write=True))


def artifact(root: Path, path: Path, resource: str) -> dict:
    return {"relative_path": path.relative_to(root).as_posix(), "resource": resource, **file_digest(path)}
