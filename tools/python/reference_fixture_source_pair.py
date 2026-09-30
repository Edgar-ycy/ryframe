"""为已完成的 Device 隔离夹具签发本地来源组合收据。"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

from devex_clone_capture import write_json
from devex_clone_model import local_path
from full_stack_provenance import reference_fixture_source_pair
from reference_fixture_control_protocol import run_private


PROTOCOL_SCHEMAS = {
    "publish": (("output",), (), True),
}


def write_pair(backend: Path, output: Path) -> dict:
    """只写入一个新收据，后续构建和运行将复用其精确来源。"""
    backend = backend.resolve(strict=True)
    requested = output if output.is_absolute() else backend / output
    path = local_path(backend, str(requested), new=True)
    receipt = reference_fixture_source_pair(backend)
    execution = Path(receipt["fixture_receipt"]["path"]).parent / "backend"
    allowed = execution / ".local-tests/reference-fixture"
    if path.name != "source-pair.json" or not path.parent.is_relative_to(allowed) or not path.parent.is_dir():
        raise ValueError("参考夹具来源组合必须写入执行工作树的指定新 source-pair.json")
    write_json(path, receipt)
    return receipt


def main(arguments: list[str]) -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args(arguments)
    if not args.write:
        parser.error("签发参考夹具来源组合需要显式 --write")
    write_pair(args.backend_dir, args.output)
    print(json.dumps({"status": "reference_fixture_source_pair_created", "remote_writes": 0}, ensure_ascii=False))


if __name__ == "__main__":
    raise SystemExit(
        run_private("source-pair", PROTOCOL_SCHEMAS, main, positional_operation=False)
    )
