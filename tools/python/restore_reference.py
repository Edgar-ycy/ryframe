"""旧目标计划入口及其他离线夹具共用的参考文件操作。"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

from dataclasses import dataclass
import json
import subprocess
from pathlib import Path

from restore_build import file_digest
from restore_reference_plan import plan_hash, validate_plan
from restore_runtime_evidence import read_json
from restore_reference_target import execute_plan
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


class ReferenceProtocolError(ValueError):
    """Rust 与私有阶段之间的参数协议无效。"""


@dataclass
class ReferenceRequest:
    command: str
    backend_dir: Path
    plan: Path
    write: bool
    backup_root: Path | None = None
    record: Path | None = None
    runtime_registration: Path | None = None
    copy_id: str | None = None
    artifact: str | None = None
    side: str | None = None
    backup_receipt: Path | None = None
    comparison_sources: Path | None = None
    arm_input: Path | None = None
    fresh_target_verify: Path | None = None
    product_plan: Path | None = None
    target_plan: Path | None = None
    output: Path | None = None
    missing: bool = False


def private_protocol_request(argv: list[str] | None = None) -> ReferenceRequest:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) == 3 and arguments[0] == "--native-tool" and arguments[2] == "--stdin":
        from rate_limit_tool import install_binding
        try:
            install_binding(arguments[1])
        except (ValueError, TypeError) as error:
            raise ReferenceProtocolError("reference 原生工具绑定无效") from error
        arguments = ["--stdin"]
    if arguments != ["--stdin"]:
        raise ReferenceProtocolError("reference 私有脚本只接受 --stdin")
    raw = sys.stdin.buffer.read(65_537)
    try:
        return _request_from_raw(raw.decode("utf-8"))
    except (ValueError, UnicodeError) as error:
        raise ReferenceProtocolError("reference stdin 私有协议无效") from error


def _request_from_raw(raw: str) -> ReferenceRequest:
    try:
        values = _coordinate("request", {"raw": raw})
    except (ValueError, RuntimeError) as error:
        raise ReferenceProtocolError("reference 私有参数协议无效") from error
    if values["command"] in {"dataset", "backup", "restore"}:
        raise ReferenceProtocolError("参考数据准备、备份和恢复由 Rust 公共入口执行")
    paths = {name: Path(value) for name, value in values.items() if name in {
        "backend_dir", "plan",
        "backup_root", "record", "runtime_registration", "backup_receipt", "comparison_sources",
        "arm_input", "fresh_target_verify", "product_plan", "target_plan", "output"}}
    return ReferenceRequest(**{**values, **paths})


def main(args: ReferenceRequest) -> None:
    if args.command != "plan":
        raise ReferenceProtocolError("该参考阶段由 Rust 公共入口执行")
    backend, plan = args.backend_dir.resolve(strict=True), read_json(args.plan)
    validate_plan(plan, backend)
    result = execute_plan(args, backend, plan)
    print(json.dumps(result if result is not None else _coordinate("summary", {"plan": plan}), ensure_ascii=False))


if __name__ == "__main__":
    try:
        main(private_protocol_request())
    except ReferenceProtocolError:
        print("restore_reference_protocol_error：私有参数协议无效。", file=sys.stderr)
        raise SystemExit(2)
    except Exception:
        print("restore_reference_failed：阶段失败，请核对账本、前后像和未知结果；不自动重放。",
              file=sys.stderr)
        raise SystemExit(1)
