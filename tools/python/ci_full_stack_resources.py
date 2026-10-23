"""绑定全栈构建来源与二进制产物；对象存储由 Rust 文件配置上下文处理。"""

from __future__ import annotations

import json
from pathlib import Path

from full_stack_provenance import BUILD_EVIDENCE, build_evidence, full_stack_source_evidence
from full_stack_process import write_receipt
from full_stack_build import build_artifacts

BINARIES = (("bin-reset", "ryframe-reset"), ("bin-migrate", "ryframe-migrate"),
            ("bin-api", "ryframe"), ("bin-worker", "ryframe-worker"))
ROLE_BY_NAME = {
    "ryframe": "api",
    "ryframe-worker": "worker",
    "ryframe-reset": "reset",
    "ryframe-migrate": "migrate",
}


def build_binaries(run, backend_root: Path, output_dir: Path) -> dict[str, str]:
    source = full_stack_source_evidence(backend_root, output_dir)
    built = build_artifacts(
        backend_root,
        tuple(ROLE_BY_NAME[name] for _, name in BINARIES),
        lambda command: run(command, cwd=backend_root, capture_output=True).stdout,
        require_workspace_manifest=False,
        require_binary_kind=False,
    )
    binaries = {item["name"]: str(item["executable"]) for item in built.values()}
    evidence = build_evidence(backend_root, output_dir, binaries, source)
    write_receipt(output_dir / "binaries.json", binaries)
    write_receipt(output_dir / BUILD_EVIDENCE, evidence)
    return binaries


def read_binaries(output_dir: Path) -> dict[str, str]:
    binaries = json.loads((output_dir / "binaries.json").read_text(encoding="utf-8"))
    if set(binaries) != {name for _, name in BINARIES}:
        raise ValueError("全栈二进制清单不完整")
    for value in binaries.values():
        if not Path(value).is_absolute() or not Path(value).is_file():
            raise ValueError("全栈二进制路径无效")
    return binaries
