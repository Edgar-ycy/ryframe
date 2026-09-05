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

from release_stage import release_stage


def git(root: Path, *arguments: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(root), *arguments])


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


def snapshot(root: Path) -> tuple[dict, bytes]:
    head = git(root, "rev-parse", "HEAD").decode().strip()
    if not re.fullmatch(r"[a-f0-9]{40}", head):
        raise ValueError("源码 HEAD 不是有效 SHA")
    patch = git(root, "diff", "--binary", "HEAD")
    files = []
    for raw in sorted(
        git(root, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0")
    ):
        if not raw:
            continue
        relative = raw.decode("utf-8")
        path = root / relative
        if (
            path.is_symlink()
            or not path.resolve().is_relative_to(root)
            or not path.is_file()
        ):
            raise ValueError("未跟踪文件越界、不是普通文件或包含符号链接")
        files.append(
            {"path": relative, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
        )
    return {
        "head": head,
        "patch_sha256": hashlib.sha256(patch).hexdigest(),
        "files": files,
    }, patch


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


def prepare(backend: Path, frontend: Path, output: Path) -> dict:
    validate_paths(backend, frontend, output)
    backend_receipt, backend_patch = snapshot(backend)
    frontend_receipt, frontend_patch = snapshot(frontend)
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

    def write_receipt() -> None:
        (output / "fixture.json").write_text(
            json.dumps(receipt, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
        )

    write_receipt()
    copy_snapshot(backend, roots["backend"], backend_receipt, backend_patch, log)
    copy_snapshot(frontend, roots["frontend"], frontend_receipt, frontend_patch, log)
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
    write_receipt()
    return receipt


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--frontend-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--write", action="store_true", required=True)
    args = parser.parse_args()
    receipt = prepare(
        args.backend_dir.resolve(),
        args.frontend_dir.resolve(),
        args.output_dir.resolve(),
    )
    print(
        json.dumps(
            {"status": receipt["status"], "paths": receipt["paths"]}, ensure_ascii=False
        )
    )


if __name__ == "__main__":
    main()
