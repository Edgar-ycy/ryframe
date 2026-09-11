"""验证同代来源与严格比较来源；正式来源运行由唯一 generation 入口控制。"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from restore_build import validate_new_output, write_new
from restore_comparison_source import capture_comparison_sources, verify_comparison_sources
from restore_runtime_evidence import read_json_document
from restore_source_runtime import execute_source_verification


def _add_comparison_capture_parser(commands) -> None:
    command = commands.add_parser("comparison-capture")
    for name in (
        "backend-dir", "b0-backend", "b0-adapter-backend", "b0-frontend",
        "b0-backend-build", "b0-frontend-build", "b1-backend", "b1-frontend",
        "b1-backend-build", "b1-frontend-build", "source-export-result", "output",
    ):
        command.add_argument("--" + name, type=Path, required=True)
    command.add_argument(
        "--write", action="store_true", required=True,
        help="显式写入新的双版本来源清单；只读取源码、构建收据和已发布 source-export",
    )


def _add_comparison_verify_parser(commands) -> None:
    command = commands.add_parser("comparison-verify")
    command.add_argument("--backend-dir", type=Path, required=True)
    command.add_argument("--receipt", type=Path, required=True)


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
    return {"output": str(output), "status": "comparison_sources_captured", "restore_success": False}


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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("verify")
    command.add_argument("--backend-dir", type=Path, required=True)
    command.add_argument("--source-generation", type=Path, required=True)
    command.add_argument("--output", type=Path, required=True)
    command.add_argument(
        "--write",
        action="store_true",
        required=True,
        help="显式执行同代来源只读业务验收、登记会话副作用并写入严格收据",
    )
    _add_comparison_capture_parser(commands)
    _add_comparison_verify_parser(commands)
    args = parser.parse_args()
    backend = args.backend_dir.resolve()
    if args.command == "comparison-capture":
        print(json.dumps(_capture_comparison(args, backend)))
        return
    if args.command == "comparison-verify":
        print(json.dumps(_verify_comparison(args, backend)))
        return
    print(json.dumps(execute_source_verification(
        backend, args.source_generation, args.output
    )))


if __name__ == "__main__":
    main()
