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
    "AR", "AWS_LC_SYS", "BINDGEN_EXTRA_CLANG_ARGS", "CARGO_BUILD_JOBS", "CARGO_BUILD_TARGET",
    "CARGO_ENCODED_RUSTFLAGS", "CARGO_HOME", "CARGO_INCREMENTAL", "CARGO_NET_OFFLINE",
    "CARGO_MAKEFLAGS", "CARGO_TARGET_DIR", "CC", "CC_ENABLE_DEBUG_OUTPUT", "CC_FORCE_DISABLE",
    "CFLAGS", "CLANG", "CLANG_PATH", "CMAKE",
    "CMAKE_GENERATOR", "CMAKE_TOOLCHAIN_FILE", "CRATE_CC_NO_DEFAULTS", "CXX", "CXXFLAGS",
    "CXXSTDLIB", "HOST", "INCLUDE", "LDFLAGS", "LIB", "LIBCLANG_PATH", "LIBPATH", "MAKE",
    "NASM", "NINJA", "OPENSSL", "PATH", "PERL", "PKG_CONFIG", "RANLIB", "RUSTC",
    "RUSTC_BOOTSTRAP", "RUSTC_LINKER", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER",
    "RUSTDOCFLAGS", "RUSTFLAGS", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN", "RYFRAME_BUILD_COMMIT",
    "SDKROOT", "SOURCE_DATE_EPOCH", "TARGET", "UCRTVERSION", "UNIVERSALCRTSDKDIR",
    "VCINSTALLDIR", "VCPKGRS_DYNAMIC", "VCPKGRS_TRIPLET", "VCTOOLSINSTALLDIR",
    "WINDOWSSDKDIR", "WINDOWSSDKVERSION",
}
BUILD_ENVIRONMENT_PREFIXES = (
    "AR_", "AWS_LC_", "AWS_LC_SYS_", "BINDGEN_", "CARGO_PROFILE_", "CARGO_TARGET_", "CC_",
    "CFLAGS_", "CMAKE_", "CXX_", "CXXFLAGS_", "LDFLAGS_", "OPENSSL_", "PKG_CONFIG_",
    "RANLIB_", "RUSTFLAGS_", "VCPKG_",
)


def repository(path: Path, label: str) -> Path:
    """只接受调用方明确给出的、无链接的规范 Git 工作树根目录。"""
    if not path.is_absolute():
        raise ValueError(f"{label}必须是绝对路径")
    for candidate in (path, *path.parents):
        metadata = candidate.lstat()
        if candidate.is_symlink() or (getattr(metadata, "st_file_attributes", 0) or 0) & 0x400:
            raise ValueError(f"{label}路径不能包含符号链接或 junction")
    root = path.resolve(strict=True)
    actual = Path(git(root, "rev-parse", "--show-toplevel").decode("utf-8", errors="strict").strip())
    if root != path or actual.resolve(strict=True) != root:
        raise ValueError(f"{label}必须是规范 Git 工作树根目录")
    return root


def registered_source(
    coordinator: Path,
    source_backend: Path,
    expected_head: str,
    *,
    adapter_contract: str | None = None,
    product_backend: Path | None = None,
    reconstruct: bool = True,
) -> tuple[Path, dict, tuple[Path, dict] | None]:
    """核验声明的 clean 源；发布重建 B0 补丁，预览只读复核已绑定提交及产品域。"""
    if re.fullmatch(r"[a-f0-9]{40}", expected_head) is None:
        raise ValueError("构建来源提交必须是完整小写 SHA")
    source = repository(source_backend, "后端构建来源")
    inventory = capture_inventory(source)
    snapshot = inventory["source"]["snapshot"]
    if snapshot["head"] != expected_head or not snapshot["clean"]:
        raise ValueError("恢复构建必须使用声明的精确干净后端源码")
    if adapter_contract is None:
        if product_backend is not None:
            raise ValueError("普通构建来源不接受独立产品来源")
        return source, inventory, None
    if adapter_contract != "legacy-stable-readiness-b0-v1" or product_backend is None:
        raise ValueError("未知或不完整的 B0 构建适配声明")

    # 延迟导入避免 comparison 模块与本模块的构建收据核验形成导入环。
    from restore_comparison_source import (
        B0_ADAPTER_COMMIT,
        B0_BACKEND_COMMIT,
        b0_adapter_evidence,
    )

    coordinator = repository(coordinator, "构建协调后端")
    product = repository(product_backend, "B0 后端产品来源")
    product_inventory = capture_inventory(product)
    product_snapshot = product_inventory["source"]["snapshot"]
    if (expected_head != B0_ADAPTER_COMMIT or snapshot["head"] != B0_ADAPTER_COMMIT
            or product_snapshot["head"] != B0_BACKEND_COMMIT or not product_snapshot["clean"]):
        raise ValueError("B0 构建来源与登记的产品或适配提交不匹配")
    evidence = b0_adapter_evidence(coordinator, reconstruct=reconstruct)
    if evidence["contract"] != adapter_contract:
        raise ValueError("B0 内嵌适配证据与构建声明不匹配")
    product_domains = build_source_domains(product_inventory, "backend")["product"]
    if build_source_domains(inventory, "backend")["product"] != product_domains:
        raise ValueError("B0 工具适配改变了 API 或 Worker 产品输入")
    return source, inventory, (product, product_inventory)


def build_environment(environment: dict[str, str] | None = None) -> dict:
    """只记录会影响 Cargo 产物的变量名及值摘要，不把环境值写入收据。"""
    environment = os.environ if environment is None else environment

    def selected(name: str) -> bool:
        # Windows 环境变量名不区分大小写；收据仍保留操作系统提供的实际名称。
        candidate = name.upper() if os.name == "nt" else name
        return candidate in BUILD_ENVIRONMENT_NAMES or candidate.startswith(BUILD_ENVIRONMENT_PREFIXES)

    entries = [
        {"name": name, "sha256": hashlib.sha256(environment[name].encode()).hexdigest()}
        for name in sorted(environment)
        if selected(name)
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


def build(root: Path, run=subprocess.run, expected_inventory: dict | None = None) -> dict:
    source = source_snapshot(root)
    inventory = capture_inventory(root, source)
    if expected_inventory is not None and inventory != expected_inventory:
        raise ValueError("构建开始前源码与已核验来源不一致")
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


def build_registered(
    coordinator: Path,
    source_backend: Path,
    expected_head: str,
    *,
    adapter_contract: str | None = None,
    product_backend: Path | None = None,
    run=subprocess.run,
) -> tuple[Path, dict]:
    source, inventory, product = registered_source(
        coordinator,
        source_backend,
        expected_head,
        adapter_contract=adapter_contract,
        product_backend=product_backend,
    )
    receipt = build(source, run, inventory)
    verify_build(source, receipt, expected_head, run)
    if capture_inventory(source) != inventory:
        raise ValueError("构建结束后已登记后端来源发生变化")
    if product is not None and capture_inventory(product[0]) != product[1]:
        raise ValueError("构建期间 B0 产品来源发生变化")
    return source, receipt


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
    fields = {"format_version", "kind", "sources", "build", "artifacts"}
    if (not isinstance(receipt, dict) or set(receipt) != fields
            or receipt.get("format_version") != 2 or receipt.get("kind") != "restore-backend-build"
            or set(receipt.get("artifacts", {})) != set(ROLES)):
        raise ValueError("构建收据缺少完整 API 与 Worker 产物")
    for role, artifact in receipt["artifacts"].items():
        if (not isinstance(artifact, dict)
                or set(artifact) != {"executable", "command", "bytes", "sha256"}
                or artifact.get("command") != build_command(role)):
            raise ValueError("恢复二进制构建命令不匹配")
        actual = file_digest(Path(artifact["executable"]))
        if any(artifact.get(key) != value for key, value in actual.items()):
            raise ValueError("恢复二进制文件与构建收据不一致")
