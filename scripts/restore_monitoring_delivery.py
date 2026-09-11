"""正式恢复的隔离 Prometheus、Alertmanager 与本地 webhook 验收入口。"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import argparse
import json
import subprocess
from pathlib import Path

from devex_clone_source_proof import require_closed_port
from restore_build import repository, validate_new_output, write_new
from restore_monitoring_authority import monitoring_preflight
from restore_monitoring_evidence import (
    build_binding,
    descriptor,
    read_binding,
    verify_binding_inputs,
)
from restore_runtime_evidence import reject_link_or_reparse


def bind(
    backend: Path,
    runtime_receipt: Path,
    target_plan: Path,
    output: Path,
    run_id: str,
    credential: Path,
    tool_paths: dict[str, Path],
    ports: dict[str, int],
    *,
    preflight=monitoring_preflight,
    run=subprocess.run,
    port_check=require_closed_port,
) -> dict:
    backend = repository(backend, "监控验收协调后端")
    for label, path in (
        ("恢复运行收据", runtime_receipt),
        ("恢复目标计划", target_plan),
        ("监控凭据", credential),
        ("监控绑定输出", output),
    ):
        if not path.is_absolute():
            raise ValueError(f"{label}必须使用绝对路径")
    reject_link_or_reparse(output.parent)
    output = validate_new_output(output, backend)
    if output.name != "binding.json":
        raise ValueError("监控绑定输出必须命名为 binding.json")
    run_directory = output.parent
    if any(run_directory.iterdir()):
        raise ValueError("监控绑定要求独立的空 run 目录")
    authority = preflight(backend, runtime_receipt, target_plan)
    binding = build_binding(
        backend,
        run_id,
        authority,
        credential,
        tool_paths,
        ports,
        run=run,
        port_check=port_check,
    )
    if any(run_directory.iterdir()):
        raise ValueError("监控绑定期间 run 目录出现未知写入")
    write_new(output, binding, backend)
    document, observed = read_binding(backend, output)
    if observed != binding or descriptor(output) != {
        "path": str(document.path),
        "bytes": len(document.raw),
        "sha256": document.sha256,
    }:
        raise ValueError("监控绑定发布后内容或摘要不同")
    verified, snapshots = verify_binding_inputs(backend, observed, preflight, run)
    if verified != binding:
        raise ValueError("监控绑定发布后无法重建相同权威")
    document.assert_unchanged()
    for snapshot in snapshots:
        snapshot.assert_unchanged()
    return binding


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    commands = parser.add_subparsers(dest="command", required=True)
    child = commands.add_parser("bind", help="只读复核正式恢复运行并发布监控输入绑定", allow_abbrev=False)
    child.add_argument("--backend-dir", type=Path, required=True)
    child.add_argument("--runtime-receipt", type=Path, required=True)
    child.add_argument("--target-plan", type=Path, required=True)
    child.add_argument("--output", type=Path, required=True)
    child.add_argument("--run-id", required=True)
    child.add_argument("--metrics-token-file", type=Path, required=True)
    for name in ("prometheus", "promtool", "alertmanager", "amtool"):
        child.add_argument("--" + name, type=Path, required=True)
    for name in ("prometheus", "alertmanager", "webhook"):
        child.add_argument("--" + name + "-port", type=int, required=True)
    child.add_argument("--write", action="store_true", required=True)
    return parser


def main(arguments: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if arguments is None else arguments
    options = [value.partition("=")[0] for value in arguments if value.startswith("--")]
    if len(options) != len(set(options)):
        _parser().error("监控投递选项不能重复")
    args = _parser().parse_args(arguments)
    try:
        result = bind(
            args.backend_dir,
            args.runtime_receipt,
            args.target_plan,
            args.output,
            args.run_id,
            args.metrics_token_file,
            {name: getattr(args, name) for name in ("prometheus", "promtool", "alertmanager", "amtool")},
            {name: getattr(args, name + "_port") for name in ("prometheus", "alertmanager", "webhook")},
        )
        print(
            json.dumps(
                {
                    "status": "bound",
                    "run_id": result["run_id"],
                    "scope_id": result["scope_id"],
                    "binding": descriptor(args.output),
                    "remote_writes": 0,
                    "external_contacts": 0,
                },
                ensure_ascii=False,
                sort_keys=True,
                separators=(",", ":"),
            )
        )
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(
            json.dumps(
                {"status": "failed", "error_type": type(error).__name__},
                sort_keys=True,
                separators=(",", ":"),
            )
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
