"""验证同代来源与严格比较来源；正式来源运行由唯一 generation 入口控制。"""
from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import sys
from types import SimpleNamespace

from restore_build import validate_new_output, write_new
from restore_comparison_source import capture_comparison_sources, verify_comparison_sources
from restore_runtime_evidence import read_json_document
from restore_source_runtime import execute_source_verification


def _comparison_result_binding(path: Path) -> tuple[dict, object]:
    document = read_json_document(path)
    return {
        "path": str(document.path),
        "bytes": len(document.raw),
        "sha256": document.sha256,
    }, document


def _capture_comparison(args, backend: Path) -> dict:
    output = validate_new_output(args.output, backend)
    export, document = _comparison_result_binding(args.source_export_result)
    result = capture_comparison_sources(
        backend,
        b0_backend=args.b0_backend,
        b0_adapter_backend=args.b0_adapter_backend,
        b0_frontend=args.b0_frontend,
        b0_backend_build=args.b0_backend_build,
        b0_frontend_build=args.b0_frontend_build,
        b1_backend=args.b1_backend,
        b1_frontend=args.b1_frontend,
        b1_backend_build=args.b1_backend_build,
        b1_frontend_build=args.b1_frontend_build,
        source_export_result=export,
    )
    document.assert_unchanged()
    write_new(output, result, backend)
    return {
        "output": str(output),
        "status": "comparison_sources_captured",
        "restore_success": False,
    }


def _verify_comparison(args, backend: Path) -> dict:
    document = read_json_document(args.receipt)
    result = verify_comparison_sources(backend, document.value)
    document.assert_unchanged()
    return {
        "receipt": str(document.path),
        "status": "comparison_sources_verified",
        "source_export_identity_sha256": result["source_export"]["identity_sha256"],
        "restore_success": False,
    }


PROTOCOL_KEY = "RYFRAME_RESTORE_SOURCE_PROTOCOL"
PROTOCOL_PREFIX = "RYFRAME_RESTORE_SOURCE_"
FOREIGN_PROTOCOL_KEY = "RYFRAME_RESTORE_RUNTIME_PROTOCOL"
PROTOCOL_KIND = "ryframe-xtask-restore-source"
PROTOCOL_MAX_BYTES = 16 * 1024
OPERATIONS = frozenset({"verify", "comparison-capture", "comparison-verify"})
WRITING_OPERATIONS = frozenset({"verify", "comparison-capture"})
BASE_FIELDS = {"backend_dir", "format_version", "kind", "operation", "write"}
OPERATION_FIELDS = {
    "verify": {"source_generation", "output"},
    "comparison-capture": {
        "b0_backend", "b0_adapter_backend", "b0_frontend", "b0_backend_build",
        "b0_frontend_build", "b1_backend", "b1_frontend", "b1_backend_build",
        "b1_frontend_build", "source_export_result", "output",
    },
    "comparison-verify": {"receipt"},
}


class SourceProtocolError(ValueError):
    """私有来源协议错误；公开 Rust 参数解析应在启动前阻止同类输入。"""


@dataclass(frozen=True)
class SourceRequest:
    operation: str
    values: dict

    def namespace(self) -> SimpleNamespace:
        return SimpleNamespace(command=self.operation, **self.values)


def _unique_protocol_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise SourceProtocolError(f"私有协议字段重复：{key}")
        result[key] = value
    return result


def _reject_protocol_constant(value: str) -> None:
    raise SourceProtocolError(f"恢复来源私有协议包含非标准数值：{value}")


def _protocol_path(value: object, label: str) -> Path:
    if not isinstance(value, str) or not value or not Path(value).is_absolute():
        raise SourceProtocolError(f"{label}必须是非空绝对路径")
    if any(character in value for character in ("\n", "\r", "\0")):
        raise SourceProtocolError(f"{label}不能包含换行符或 NUL")
    return Path(value)


def private_protocol_request(
    argv: list[str] | None = None,
    environment: dict[str, str] | None = None,
) -> SourceRequest:
    arguments = sys.argv[1:] if argv is None else list(argv)
    values = os.environ if environment is None else environment
    if arguments:
        raise SourceProtocolError("恢复来源脚本是私有实现，不接受命令行参数")
    unknown = sorted(
        name for name in values
        if (name.startswith(PROTOCOL_PREFIX) and name != PROTOCOL_KEY)
        or name == FOREIGN_PROTOCOL_KEY
    )
    if unknown:
        raise SourceProtocolError("恢复来源私有环境包含未知字段")
    raw = values.get(PROTOCOL_KEY)
    if (
        not isinstance(raw, str)
        or not raw
        or len(raw.encode("utf-8")) > PROTOCOL_MAX_BYTES
    ):
        raise SourceProtocolError("恢复来源私有协议缺失、为空或过长")
    if any(character in raw for character in ("\n", "\r", "\0")):
        raise SourceProtocolError("恢复来源私有协议不能包含换行符或 NUL")
    try:
        protocol = json.loads(
            raw,
            object_pairs_hook=_unique_protocol_object,
            parse_constant=_reject_protocol_constant,
        )
    except SourceProtocolError:
        raise
    except (TypeError, json.JSONDecodeError) as error:
        raise SourceProtocolError("恢复来源私有协议不是严格 JSON") from error
    if not isinstance(protocol, dict):
        raise SourceProtocolError("恢复来源私有协议必须是 JSON 对象")
    if (
        type(protocol.get("format_version")) is not int
        or protocol.get("format_version") != 1
        or protocol.get("kind") != PROTOCOL_KIND
    ):
        raise SourceProtocolError("恢复来源私有协议版本或类型无效")
    operation = protocol.get("operation")
    if not isinstance(operation, str) or operation not in OPERATIONS:
        raise SourceProtocolError("恢复来源私有协议操作无效")
    if set(protocol) != BASE_FIELDS | OPERATION_FIELDS[operation]:
        raise SourceProtocolError("恢复来源私有协议字段不完整或含未知字段")
    write = protocol["write"]
    if type(write) is not bool or write != (operation in WRITING_OPERATIONS):
        raise SourceProtocolError("恢复来源私有协议写入授权与操作不一致")
    decoded = {
        name: _protocol_path(value, name)
        for name, value in protocol.items()
        if name not in {"format_version", "kind", "operation", "write"}
    }
    decoded["write"] = write
    return SourceRequest(operation, decoded)


def execute(request: SourceRequest) -> dict:
    args = request.namespace()
    backend = args.backend_dir.resolve()
    if args.command == "comparison-capture":
        return _capture_comparison(args, backend)
    if args.command == "comparison-verify":
        return _verify_comparison(args, backend)
    return execute_source_verification(
        backend, args.source_generation, args.output
    )


def main(request: SourceRequest | None = None) -> None:
    if request is None:
        request = private_protocol_request()
    os.environ.pop(PROTOCOL_KEY, None)
    print(json.dumps(execute(request), ensure_ascii=False, sort_keys=True, separators=(",", ":")))


if __name__ == "__main__":
    try:
        main()
    except SourceProtocolError:
        print("restore_source_protocol_error：恢复来源私有协议无效。", file=sys.stderr)
        raise SystemExit(2)
