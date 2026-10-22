"""恢复测试的真实 TOML、受管身份和干净 Git 来源准备；不伪造 clean 标志。"""
from __future__ import annotations

import json
from pathlib import Path
import subprocess

from restore_frontend_build import FRONTEND_BUILD_TOOL_FILES, FRONTEND_RECEIPT, frontend_files
from restore_frontend_configuration import _capture
from restore_frontend_context import IDENTITY
from source_inventory import build_source_domains, capture_inventory

APPLICATION = ('api_origin = ""\n[server]\nhost = "localhost"\nport = 5173\n'
               'proxy_target = "http://localhost:8080"\n')


def write_json(file: Path, value: object) -> None:
    file.parent.mkdir(parents=True, exist_ok=True)
    file.write_text(json.dumps(value, ensure_ascii=False) + "\n", encoding="utf-8")


def git(root: Path, *arguments: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(root), *arguments], stderr=subprocess.PIPE)


def prepare_frontend(root: Path, *, formal: bool = True) -> str:
    (root / "config").mkdir(parents=True, exist_ok=True)
    (root / ".local-tests/build").mkdir(parents=True, exist_ok=True)
    (root / "dist/.vite").mkdir(parents=True, exist_ok=True)
    (root / "config/app.toml").write_text(APPLICATION, encoding="utf-8")
    for environment in ("dev", "test", "prod"):
        (root / f"config/app.{environment}.toml").write_text("# 所选环境\n", encoding="utf-8")
    (root / "config/tools.json").write_text("{}\n", encoding="utf-8")
    (root / ".gitignore").write_text("/dist/\n/.local-tests/\n/node_modules/\n/.env*\n", encoding="utf-8")
    # 此处仅为受控命令调用测试提供输入文件集合，不包含或执行 JS 实现。
    for relative in (*FRONTEND_BUILD_TOOL_FILES, "vite.config.ts"):
        file = root / relative
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_bytes(b"")
    if not (root / "dist/index.html").exists():
        (root / "dist/index.html").write_bytes(b"<main>fixture</main>")
    (root / "dist/.vite/manifest.json").write_bytes(b"{}")
    git(root, "init", "--quiet")
    git(root, "add", ".")
    git(root, "-c", "user.name=RyFrame fixture", "-c", "user.email=fixture@example.invalid",
        "commit", "--quiet", "-m", "isolated frontend fixture")
    head = git(root, "rev-parse", "HEAD").decode("ascii").strip()
    commit = head if formal else "development"
    write_json(root / IDENTITY, {"format_version": 1, "frontend_commit": commit})
    write_json(root / "dist/build-identity.json", {"frontend_commit": commit})
    return head


def frontend_context(root: Path, environment: str = "dev") -> dict:
    inputs = _capture(root, environment)
    inputs.assert_unchanged()
    return {
        "command": ["vite", "build"], "mode": "production", "target": "vite-default",
        "toolchain": {"node": "v24.0.0", "pnpm": {"pinned": "11.20.0", "observed": "11.20.0"},
                      "vite": "7.1.7"},
        "configuration": inputs.configuration,
        "identity": inputs.identity,
    }


def frontend_receipt(root: Path, environment: str = "dev") -> dict:
    return {"format_version": 3, "kind": "restore-frontend-build",
            "sources": build_source_domains(capture_inventory(root), "frontend"),
            "build": frontend_context(root, environment), "files": frontend_files(root)}


def save_receipt(root: Path, environment: str = "dev") -> Path:
    file = root / "dist" / FRONTEND_RECEIPT
    write_json(file, frontend_receipt(root, environment))
    return file
