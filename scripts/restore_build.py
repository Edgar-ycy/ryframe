"""从本次 Cargo JSON 构建绑定源码快照与二进制摘要。"""

from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
from pathlib import Path

from artifact_digests import file_digest
from full_stack_build import build_artifacts, cargo_artifact
from source_inventory import (
    build_source_domains,
    canonical_digest,
    capture_inventory,
    git,
    source_snapshot,
    validate_build_source_domains,
)

ROLES = {"api": ("bin-api", "ryframe"), "worker": ("bin-worker", "ryframe-worker")}
# 正式恢复夹具沿用既有的无 --release 定向 Cargo 构建；统一 xtask build 的默认 release 另行管理。
RESTORE_BUILD_PROFILE = "dev"
BUILD_ENVIRONMENT_NAMES = {
    "AR", "BINDGEN_EXTRA_CLANG_ARGS", "CARGO_BUILD_JOBS", "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS", "CARGO_HOME", "CARGO_INCREMENTAL", "CARGO_NET_OFFLINE",
    "CARGO_TARGET_DIR", "CC", "CC_ENABLE_DEBUG_OUTPUT", "CC_FORCE_DISABLE", "CFLAGS", "CMAKE",
    "CMAKE_GENERATOR", "CMAKE_TOOLCHAIN_FILE", "CRATE_CC_NO_DEFAULTS", "CXX", "CXXFLAGS",
    "CXXSTDLIB", "HOST", "LDFLAGS", "RANLIB", "RUSTC", "RUSTC_BOOTSTRAP", "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER", "RUSTDOCFLAGS", "RUSTFLAGS", "RUSTUP_TOOLCHAIN", "SDKROOT",
    "SOURCE_DATE_EPOCH", "TARGET", "VCPKGRS_DYNAMIC", "VCPKGRS_TRIPLET",
}
BUILD_ENVIRONMENT_PREFIXES = (
    "AR_", "AWS_LC_", "AWS_LC_SYS_", "BINDGEN_", "CARGO_PROFILE_", "CARGO_TARGET_", "CC_",
    "CFLAGS_", "CMAKE_", "CXX_", "CXXFLAGS_", "LDFLAGS_", "OPENSSL_", "PKG_CONFIG_",
    "RANLIB_", "RUSTFLAGS_", "VCPKG_",
)


def build_environment(environment: dict[str, str] | None = None) -> dict:
    """只记录会影响 Cargo 产物的变量名及值摘要，不把环境值写入收据。"""
    environment = os.environ if environment is None else environment
    entries = [
        {"name": name, "sha256": hashlib.sha256(environment[name].encode()).hexdigest()}
        for name in sorted(environment)
        if name in BUILD_ENVIRONMENT_NAMES or name.startswith(BUILD_ENVIRONMENT_PREFIXES)
    ]
    return {"variables": [item["name"] for item in entries], "sha256": canonical_digest(entries)}


def validate_build_environment(value: object) -> dict:
    if (not isinstance(value, dict) or set(value) != {"variables", "sha256"}
            or not isinstance(value["variables"], list)
            or value["variables"] != sorted(set(value["variables"]))
            or any(not isinstance(name, str) or not name for name in value["variables"])
            or not isinstance(value["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", value["sha256"]) is None):
        raise ValueError("构建环境摘要无效")
    return value


def build_command(role: str) -> list[str]:
    feature, name = ROLES[role]
    return ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
            "--features", feature, "--bin", name, "--message-format=json"]


def _version(root: Path, run, command: list[str]) -> str:
    result = run(command, cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                 encoding="utf-8", check=True,
                 creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
    value = result.stdout.strip()
    if not value:
        raise ValueError("构建工具链版本输出为空")
    return value


def build_context(root: Path, run=subprocess.run, environment: dict[str, str] | None = None) -> dict:
    environment = os.environ if environment is None else environment
    rustc = _version(root, run, ["rustc", "-vV"])
    host = next((line.removeprefix("host: ") for line in rustc.splitlines() if line.startswith("host: ")), None)
    target = environment.get("CARGO_BUILD_TARGET", "").strip() or host
    if not target:
        raise ValueError("无法确定 Cargo 构建目标")
    jobs = environment.get("CARGO_BUILD_JOBS", "").strip() or "cargo-default"
    if jobs != "cargo-default" and (not jobs.isdigit() or int(jobs) < 1):
        raise ValueError("CARGO_BUILD_JOBS 必须是正整数")
    return {
        "commands": {role: build_command(role) for role in ROLES},
        "profile": RESTORE_BUILD_PROFILE,
        "target": target,
        "jobs": jobs,
        "toolchain": {"cargo": _version(root, run, ["cargo", "-V"]), "rustc": rustc},
        "environment": build_environment(environment),
    }


def validate_build_context(value: object) -> dict:
    if (not isinstance(value, dict)
            or set(value) != {"commands", "profile", "target", "jobs", "toolchain", "environment"}
            or value["commands"] != {role: build_command(role) for role in ROLES}
            or value["profile"] != RESTORE_BUILD_PROFILE
            or not isinstance(value["target"], str) or not value["target"]
            or not isinstance(value["jobs"], str) or not value["jobs"]
            or not isinstance(value["toolchain"], dict)
            or set(value["toolchain"]) != {"cargo", "rustc"}
            or any(not isinstance(item, str) or not item for item in value["toolchain"].values())):
        raise ValueError("构建命令或工具链上下文无效")
    validate_build_environment(value["environment"])
    return value


def validate_new_output(path: Path, root: Path) -> Path:
    path = path.resolve()
    local = (root / ".local-tests").resolve()
    if path == local or not path.is_relative_to(local):
        raise ValueError("运行收据只能写入当前仓库忽略的 .local-tests")
    if path.exists():
        raise FileExistsError(f"运行收据已经存在：{path}")
    if not path.parent.is_dir():
        raise ValueError("运行收据的父目录必须已经存在")
    git(root, "check-ignore", path.relative_to(root).as_posix())
    return path


def write_new(path: Path, value: dict, root: Path) -> None:
    path = validate_new_output(path, root)
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def build(root: Path, run=subprocess.run) -> dict:
    source = source_snapshot(root)
    inventory = capture_inventory(root, source)
    sources = build_source_domains(inventory, "backend")
    context = build_context(root, run)
    built = build_artifacts(
        root,
        tuple(ROLES),
        lambda command: run(
            command,
            cwd=root,
            stdout=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            check=True,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        ).stdout,
    )
    artifacts = {
        role: {
            "executable": str(item["executable"]),
            "command": item["command"],
            **file_digest(item["executable"]),
        }
        for role, item in built.items()
    }
    if capture_inventory(root) != inventory or build_context(root, run) != context:
        raise ValueError("构建期间源码发生变化，不能登记混合来源产物")
    return {"format_version": 2, "kind": "restore-backend-build", "sources": sources,
            "build": context, "artifacts": artifacts}


def verify_build(root: Path, receipt: dict, sha: str, run=subprocess.run) -> None:
    sources = validate_build_source_domains(receipt.get("sources"), "backend")
    snapshot = sources["full"]["source"]["snapshot"]
    if (set(receipt) != {"format_version", "kind", "sources", "build", "artifacts"}
            or receipt.get("format_version") != 2 or receipt.get("kind") != "restore-backend-build"
            or snapshot.get("head") != sha or not snapshot.get("clean")
            or sources != build_source_domains(capture_inventory(root), "backend")
            or receipt.get("build") != build_context(root, run)
            or set(receipt.get("artifacts", {})) != set(ROLES)):
        raise ValueError("正式恢复必须使用精确干净 SHA 的本次构建收据")
    validate_build_context(receipt.get("build"))
    verify_build_artifacts(receipt)


def verify_build_artifacts(receipt: dict) -> None:
    if (receipt.get("format_version") != 2 or receipt.get("kind") != "restore-backend-build"
            or set(receipt.get("artifacts", {})) != set(ROLES)):
        raise ValueError("构建收据缺少完整 API 与 Worker 产物")
    for role, artifact in receipt["artifacts"].items():
        if artifact.get("command") != build_command(role):
            raise ValueError("恢复二进制构建命令不匹配")
        actual = file_digest(Path(artifact["executable"]))
        if any(artifact.get(key) != value for key, value in actual.items()):
            raise ValueError("恢复二进制文件与构建收据不一致")
