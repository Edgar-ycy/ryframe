"""在新的隔离工作树生成 Device，用于带业务数据的真实全栈验收。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

from full_stack_process import write_receipt
from release_stage import release_stage
from source_inventory import git, snapshot


COMMIT_PATTERN = re.compile(r"[a-f0-9]{40}")


def validate_paths(backend: Path, frontend: Path, output: Path) -> None:
    if backend == frontend:
        raise ValueError("前后端必须是两个独立仓库")
    for root, marker in ((backend, "Cargo.toml"), (frontend, "package.json")):
        if not (root / marker).is_file():
            raise ValueError(f"工作区缺少 {marker}")
        actual = Path(
            git(root, "rev-parse", "--show-toplevel").decode().strip()
        ).resolve()
        if actual != root:
            raise ValueError("必须传入实际 Git 仓库根目录")
    local = (backend / ".local-tests").resolve()
    if not output.is_relative_to(local) or output == local or output.exists():
        raise ValueError("输出必须是后端 .local-tests 下尚不存在的独立目录")
    if frontend.is_relative_to(output) or backend.is_relative_to(output):
        raise ValueError("输出不能包含源仓库")
    relative = output.relative_to(backend).as_posix()
    git(backend, "check-ignore", relative)


def run(
    arguments: list[str], cwd: Path, log: Path, *, data: bytes | None = None
) -> None:
    with log.open("ab") as stream:
        stream.write(("\n> " + " ".join(arguments) + "\n").encode())
        stream.flush()
        subprocess.run(
            arguments,
            cwd=cwd,
            input=data,
            stdout=stream,
            stderr=subprocess.STDOUT,
            check=True,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )


def copy_snapshot(
    source: Path, destination: Path, receipt: dict, patch: bytes, log: Path
) -> None:
    run(
        ["git", "worktree", "add", "--detach", str(destination), receipt["head"]],
        source,
        log,
    )
    if patch:
        run(["git", "apply", "--check", "--binary", "-"], destination, log, data=patch)
        run(["git", "apply", "--binary", "-"], destination, log, data=patch)
    for entry in receipt["files"]:
        target = destination / entry["path"]
        if not target.resolve().is_relative_to(destination):
            raise ValueError("目标工作树的未跟踪文件路径越界")
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / entry["path"], target)
    if snapshot(destination)[0] != receipt or snapshot(source)[0] != receipt:
        raise ValueError("工作树复制期间源码发生变化，拒绝使用混合快照")


def exact_clean_snapshot(root: Path, expected_sha: str) -> tuple[dict, bytes]:
    """在创建正式夹具目录前绑定精确、干净的候选提交。"""
    if COMMIT_PATTERN.fullmatch(expected_sha) is None:
        raise ValueError("正式夹具的预期提交 SHA 无效")
    receipt, patch = snapshot(root)
    status = git(root, "status", "--porcelain=v1", "--untracked-files=all")
    if (
        receipt["head"] != expected_sha
        or patch
        or receipt["files"]
        or status.strip()
    ):
        raise ValueError("正式夹具必须使用精确、干净的候选提交")
    return receipt, patch


def source_inputs(
    backend: Path,
    frontend: Path,
    expected_backend_sha: str | None,
    expected_frontend_sha: str | None,
) -> tuple[tuple[dict, bytes], tuple[dict, bytes]]:
    if (expected_backend_sha is None) != (expected_frontend_sha is None):
        raise ValueError("正式夹具必须同时指定前后端预期提交")
    if expected_backend_sha is None or expected_frontend_sha is None:
        return snapshot(backend), snapshot(frontend)
    return (
        exact_clean_snapshot(backend, expected_backend_sha),
        exact_clean_snapshot(frontend, expected_frontend_sha),
    )


def register_fixture_migration(root: Path, log: Path) -> None:
    options = (
        ["--freeze"]
        if release_stage(root)["stable"]
        else ["--refresh-baseline", "--write"]
    )
    run(
        [sys.executable, "-X", "utf8", "scripts/check_migration_history.py", *options],
        root,
        log,
    )


def reference_fixture_root(backend: Path) -> Path:
    """为后续审阅、秘密 bootstrap 和运行计划预置唯一的忽略证据父目录。"""
    root = backend / ".local-tests/reference-fixture"
    if root.exists() or root.is_symlink():
        raise ValueError("新的 Device 工作树已存在参考夹具目录")
    root.mkdir(parents=True)
    return root


def prepare(
    backend: Path,
    frontend: Path,
    output: Path,
    *,
    expected_backend_sha: str | None = None,
    expected_frontend_sha: str | None = None,
) -> dict:
    validate_paths(backend, frontend, output)
    backend_source, frontend_source = source_inputs(
        backend, frontend, expected_backend_sha, expected_frontend_sha
    )
    backend_receipt, backend_patch = backend_source
    frontend_receipt, frontend_patch = frontend_source
    fixture = backend / "crates/ryframe-generator/tests/fixtures/device.toml"
    fixture_bytes = fixture.read_bytes()
    output.mkdir(parents=True, exist_ok=False)
    log = output / "prepare.log"
    roots = {name: output / name for name in ("backend", "frontend")}
    receipt = {
        "format_version": 1,
        "fixture": "device",
        "status": "preparing",
        "fixture_sha256": hashlib.sha256(fixture_bytes).hexdigest(),
        "sources": {"backend": backend_receipt, "frontend": frontend_receipt},
        "paths": {name: str(path) for name, path in roots.items()},
    }

    write_receipt(output / "fixture.json", receipt)
    copy_snapshot(backend, roots["backend"], backend_receipt, backend_patch, log)
    copy_snapshot(frontend, roots["frontend"], frontend_receipt, frontend_patch, log)
    reference_fixture_root(roots["backend"])
    (roots["backend"] / "catalog/resources/device.toml").write_bytes(fixture_bytes)
    # Corepack 读取快照内 packageManager；使用已安装的离线 store，不复制本机环境文件。
    package = ["corepack", "pnpm", "install", "--offline", "--frozen-lockfile"]
    if os.name == "nt":
        package = [
            os.environ.get("ComSpec", "cmd.exe"),
            "/d",
            "/s",
            "/c",
            " ".join(package),
        ]
    run(package, roots["frontend"], log)
    cargo = [
        "cargo",
        "run",
        "--locked",
        "--target-dir",
        str(backend / "target/xtask-resource"),
        "-p",
        "xtask",
        "--features",
        "resource",
        "--",
    ]
    run(
        [
            *cargo,
            "generate",
            "resource",
            "device",
            "--write",
            "--frontend-dir",
            str(roots["frontend"]),
        ],
        roots["backend"],
        log,
    )
    register_fixture_migration(roots["backend"], log)
    before = {name: snapshot(path)[0] for name, path in roots.items()}
    run(
        [
            *cargo,
            "generate",
            "resource",
            "--all",
            "--check",
            "--frontend-dir",
            str(roots["frontend"]),
        ],
        roots["backend"],
        log,
    )
    if before != {name: snapshot(path)[0] for name, path in roots.items()}:
        raise ValueError("资源只读检查改写了隔离工作树")
    receipt.update(status="ready", generated=before)
    write_receipt(output / "fixture.json", receipt)
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--frontend-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--expected-backend-sha")
    parser.add_argument("--expected-frontend-sha")
    parser.add_argument("--write", action="store_true", required=True)
    args = parser.parse_args()
    if (args.expected_backend_sha is None) != (args.expected_frontend_sha is None):
        parser.error("正式夹具必须同时指定 --expected-backend-sha 与 --expected-frontend-sha")
    receipt = prepare(
        args.backend_dir.resolve(),
        args.frontend_dir.resolve(),
        args.output_dir.resolve(),
        expected_backend_sha=args.expected_backend_sha,
        expected_frontend_sha=args.expected_frontend_sha,
    )
    print(
        json.dumps(
            {"status": receipt["status"], "paths": receipt["paths"]}, ensure_ascii=False
        )
    )


if __name__ == "__main__":
    main()
