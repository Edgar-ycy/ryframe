from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

try:
    import yaml
    from yaml.constructor import ConstructorError
    from yaml.nodes import MappingNode
except ModuleNotFoundError:
    yaml = None
    ConstructorError = None
    MappingNode = None


ROOT = Path(__file__).resolve().parents[2]
DEFAULT_POLICY = ROOT / "scripts" / "supply_chain_policy.json"
DEFAULT_WORKFLOWS = ROOT / ".github" / "workflows"
TOOL_NAMES = ("cargo-audit", "cargo-deny", "cargo-cyclonedx", "sccache", "trivy")
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
ACTION_REF = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}")
REUSABLE_WORKFLOW_REF = re.compile(
    r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/\.github/workflows/"
    r"[A-Za-z0-9_.-]+\.ya?ml@[0-9a-f]{40}"
)
LOCAL_ACTION_REF = re.compile(r"\./[A-Za-z0-9_./-]+")
LOCAL_REUSABLE_WORKFLOW_REF = re.compile(
    r"\./\.github/workflows/[A-Za-z0-9_.-]+\.ya?ml"
)
CONTAINER_ACTION_REF = re.compile(r"docker://[A-Za-z0-9_.:/-]+@sha256:[0-9a-f]{64}")
INSTALL_TOOL_REF = re.compile(r"([A-Za-z0-9_.-]+)@([0-9]+\.[0-9]+\.[0-9]+)")
SERVICE_IMAGE_REF = re.compile(r"[A-Za-z0-9][A-Za-z0-9._:/-]*@sha256:[0-9a-f]{64}")
PACKAGE_NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9_-]*")
CARGO_FEATURE_NAME = re.compile(r"[A-Za-z0-9_][A-Za-z0-9_.+-]*")
SHA256 = re.compile(r"[0-9a-f]{64}")
SPDX_IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+-]*")
PORTABLE_PATH = re.compile(r"[A-Za-z0-9_.-]+(?:/[A-Za-z0-9_.-]+)*")
REQUIRED_FEATURE_TREE_PROFILES = {"API", "Worker", "Migrate", "GeneratorDefault"}


class PolicyError(ValueError):
    pass


def _object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PolicyError(f"{label} 必须是对象")
    return value


def _list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise PolicyError(f"{label} 必须是数组")
    return value


def _exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise PolicyError(f"{label} 字段不匹配，缺少={missing}，多余={extra}")


def _text(value: Any, label: str, *, minimum: int = 1) -> str:
    if not isinstance(value, str) or len(value.strip()) < minimum:
        raise PolicyError(f"{label} 必须是至少 {minimum} 个字符的非空字符串")
    return value.strip()


def _expiry(value: Any, label: str, today: dt.date) -> dt.date:
    raw = _text(value, label)
    try:
        expires = dt.date.fromisoformat(raw)
    except ValueError as exc:
        raise PolicyError(f"{label} 必须是 YYYY-MM-DD 日期") from exc
    if expires <= today:
        raise PolicyError(f"{label} 已到期或不是未来日期：{raw}")
    return expires


def _portable_path(value: Any, label: str) -> str:
    path = _text(value, label)
    parts = path.split("/")
    if PORTABLE_PATH.fullmatch(path) is None or any(
        part in {".", ".."} for part in parts
    ):
        raise PolicyError(f"{label} 必须是安全的 POSIX 相对路径")
    return path


def _license_identifiers(expression: Any, label: str) -> list[str]:
    text = _text(expression, label)
    identifiers = text.split(" OR ")
    if not identifiers or any(
        SPDX_IDENTIFIER.fullmatch(identifier) is None for identifier in identifiers
    ):
        raise PolicyError(f"{label} 只允许由 SPDX 标识符和 OR 组成")
    if len(identifiers) != len(set(identifiers)):
        raise PolicyError(f"{label} 包含重复 SPDX 标识符")
    return identifiers


def _validate_local_patch_policy(policy: dict[str, Any]) -> None:
    seen_patches: set[str] = set()
    seen_paths: set[str] = set()
    entries = _list(policy["local_patch_licenses"], "local_patch_licenses")
    for index, item in enumerate(entries):
        label = f"本地 patch 许可证[{index}]"
        patch = _object(item, label)
        _exact_keys(
            patch,
            {
                "patch",
                "package",
                "version",
                "path",
                "license_expression",
                "license_files",
            },
            label,
        )
        patch_name = _text(patch["patch"], f"{label}.patch")
        package = _text(patch["package"], f"{label}.package")
        version = _text(patch["version"], f"{label}.version")
        if PACKAGE_NAME.fullmatch(patch_name) is None:
            raise PolicyError(f"{label}.patch 不是合法 Cargo 包名")
        if PACKAGE_NAME.fullmatch(package) is None:
            raise PolicyError(f"{label}.package 不是合法 Cargo 包名")
        if SEMVER.fullmatch(version) is None:
            raise PolicyError(f"{label}.version 必须固定到完整三段版本")
        path = _portable_path(patch["path"], f"{label}.path")
        patch_identity = patch_name.casefold()
        path_identity = path.casefold()
        if patch_identity in seen_patches or path_identity in seen_paths:
            raise PolicyError(f"{label} 重复声明 patch 或路径")
        seen_patches.add(patch_identity)
        seen_paths.add(path_identity)

        declared = set(
            _license_identifiers(
                patch["license_expression"], f"{label}.license_expression"
            )
        )
        files = _list(patch["license_files"], f"{label}.license_files")
        if not files:
            raise PolicyError(f"{label}.license_files 不得为空")
        seen_spdx: set[str] = set()
        seen_files: set[str] = set()
        for file_index, item in enumerate(files):
            file_label = f"{label}.license_files[{file_index}]"
            license_file = _object(item, file_label)
            _exact_keys(
                license_file,
                {"spdx", "file", "sha256", "registry_source"},
                file_label,
            )
            spdx = _text(license_file["spdx"], f"{file_label}.spdx")
            if SPDX_IDENTIFIER.fullmatch(spdx) is None:
                raise PolicyError(f"{file_label}.spdx 不是合法 SPDX 标识符")
            file_path = _portable_path(license_file["file"], f"{file_label}.file")
            digest = _text(license_file["sha256"], f"{file_label}.sha256")
            if SHA256.fullmatch(digest) is None:
                raise PolicyError(f"{file_label}.sha256 必须是小写 SHA-256")
            if spdx in seen_spdx or file_path.casefold() in seen_files:
                raise PolicyError(f"{file_label} 重复声明 SPDX 或文件")
            seen_spdx.add(spdx)
            seen_files.add(file_path.casefold())

            registry = _object(
                license_file["registry_source"], f"{file_label}.registry_source"
            )
            _exact_keys(
                registry,
                {"package", "version", "file"},
                f"{file_label}.registry_source",
            )
            registry_package = _text(
                registry["package"], f"{file_label}.registry_source.package"
            )
            registry_version = _text(
                registry["version"], f"{file_label}.registry_source.version"
            )
            if PACKAGE_NAME.fullmatch(registry_package) is None:
                raise PolicyError(
                    f"{file_label}.registry_source.package 不是合法 Cargo 包名"
                )
            if SEMVER.fullmatch(registry_version) is None:
                raise PolicyError(
                    f"{file_label}.registry_source.version 必须固定到完整三段版本"
                )
            _portable_path(registry["file"], f"{file_label}.registry_source.file")
        if seen_spdx != declared:
            raise PolicyError(
                f"{label}.license_files 必须逐项覆盖 license_expression，"
                f"声明={sorted(declared)}，文件={sorted(seen_spdx)}"
            )


def _unique_names(
    value: Any,
    label: str,
    *,
    pattern: re.Pattern[str] = PACKAGE_NAME,
    allow_empty: bool = False,
) -> list[str]:
    names = _list(value, label)
    normalized: list[str] = []
    for index, item in enumerate(names):
        name = _text(item, f"{label}[{index}]")
        if pattern.fullmatch(name) is None:
            raise PolicyError(f"{label}[{index}] 不是合法的 Cargo 名称")
        normalized.append(name)
    if (
        (not allow_empty and not normalized)
        or len(normalized) != len(set(normalized))
    ):
        qualifier = "" if allow_empty else "为空或"
        raise PolicyError(f"{label} 不得{qualifier}包含重复项")
    return normalized


def _feature_contract(
    value: Any, label: str, *, allow_empty: bool = False
) -> dict[str, list[str]]:
    contract = _object(value, label)
    if not allow_empty and not contract:
        raise PolicyError(f"{label} 不得为空")
    normalized: dict[str, list[str]] = {}
    for package, raw_features in contract.items():
        if not isinstance(package, str) or PACKAGE_NAME.fullmatch(package) is None:
            raise PolicyError(f"{label} 包含非法的 Cargo 包名")
        normalized[package] = _unique_names(
            raw_features,
            f"{label}.{package}",
            pattern=CARGO_FEATURE_NAME,
        )
    return normalized


def _validate_runtime_feature_tree_policy(policy: dict[str, Any]) -> None:
    gate = _object(policy["runtime_feature_tree_gate"], "runtime_feature_tree_gate")
    _exact_keys(gate, {"profiles"}, "runtime_feature_tree_gate")
    profiles = _list(gate["profiles"], "runtime_feature_tree_gate.profiles")
    if not profiles:
        raise PolicyError("runtime_feature_tree_gate.profiles 不得为空")
    seen_profiles: set[str] = set()
    seen_commands: set[tuple[str, bool, tuple[str, ...], tuple[str, ...]]] = set()
    for index, item in enumerate(profiles):
        label = f"runtime_feature_tree_gate.profiles[{index}]"
        profile = _object(item, label)
        _exact_keys(profile, {"name", "command", "constraints"}, label)
        name = _text(profile["name"], f"{label}.name")
        if PACKAGE_NAME.fullmatch(name) is None:
            raise PolicyError(f"{label}.name 不是合法的 profile 名称")
        if name in seen_profiles:
            raise PolicyError(f"{label}.name 重复")
        seen_profiles.add(name)

        command = _object(profile["command"], f"{label}.command")
        _exact_keys(
            command,
            {"package", "no_default_features", "features", "edges"},
            f"{label}.command",
        )
        package = _text(command["package"], f"{label}.command.package")
        if PACKAGE_NAME.fullmatch(package) is None:
            raise PolicyError(f"{label}.command.package 不是合法的 Cargo 包名")
        no_default_features = command["no_default_features"]
        if not isinstance(no_default_features, bool):
            raise PolicyError(f"{label}.command.no_default_features 必须是布尔值")
        features = _unique_names(
            command["features"],
            f"{label}.command.features",
            pattern=CARGO_FEATURE_NAME,
            allow_empty=True,
        )
        edges = _unique_names(
            command["edges"],
            f"{label}.command.edges",
        )
        if set(edges) != {"normal", "build"}:
            raise PolicyError(f"{label}.command.edges 必须且只能包含 normal、build")
        command_identity = (
            package,
            no_default_features,
            tuple(features),
            tuple(edges),
        )
        if command_identity in seen_commands:
            raise PolicyError(f"{label}.command 与其他 profile 重复")
        seen_commands.add(command_identity)

        constraints = _object(profile["constraints"], f"{label}.constraints")
        _exact_keys(
            constraints,
            {
                "required_packages",
                "required_features",
                "forbidden_packages",
                "forbidden_features",
                "max_unique_packages",
            },
            f"{label}.constraints",
        )
        required_packages = set(
            _unique_names(
                constraints["required_packages"],
                f"{label}.constraints.required_packages",
                allow_empty=True,
            )
        )
        forbidden_packages = set(
            _unique_names(
                constraints["forbidden_packages"],
                f"{label}.constraints.forbidden_packages",
                allow_empty=True,
            )
        )
        if required_packages & forbidden_packages:
            raise PolicyError(f"{label} 的 required/forbidden package 不得重叠")
        required_features = _feature_contract(
            constraints["required_features"],
            f"{label}.constraints.required_features",
            allow_empty=True,
        )
        forbidden_features = _feature_contract(
            constraints["forbidden_features"],
            f"{label}.constraints.forbidden_features",
            allow_empty=True,
        )
        for feature_package in required_features.keys() & forbidden_features.keys():
            if set(required_features[feature_package]) & set(
                forbidden_features[feature_package]
            ):
                raise PolicyError(
                    f"{label} 的 required/forbidden feature 不得重叠："
                    f"{feature_package}"
                )
        maximum = constraints["max_unique_packages"]
        if maximum is not None and (
            not isinstance(maximum, int)
            or isinstance(maximum, bool)
            or maximum < 1
        ):
            raise PolicyError(
                f"{label}.constraints.max_unique_packages 必须为正整数或 null"
            )
    missing_profiles = REQUIRED_FEATURE_TREE_PROFILES - seen_profiles
    if missing_profiles:
        raise PolicyError(
            "runtime_feature_tree_gate 缺少强制 profile："
            + "、".join(sorted(missing_profiles))
        )


def load_policy(
    path: Path = DEFAULT_POLICY, *, today: dt.date | None = None
) -> dict[str, Any]:
    check_date = today or dt.date.today()
    try:
        policy = _object(json.loads(path.read_text(encoding="utf-8")), "供应链策略")
    except (OSError, json.JSONDecodeError) as exc:
        raise PolicyError(f"无法读取供应链策略 {path}: {exc}") from exc

    _exact_keys(
        policy,
        {
            "schema_version",
            "tools",
            "vulnerability_gate",
            "dependency_graph_exceptions",
            "service_images",
            "local_patch_licenses",
            "runtime_feature_tree_gate",
        },
        "供应链策略",
    )
    if policy["schema_version"] != 4:
        raise PolicyError("schema_version 只允许为 4")

    tools = _object(policy["tools"], "tools")
    _exact_keys(tools, set(TOOL_NAMES), "tools")
    for name, version in tools.items():
        if not isinstance(version, str) or SEMVER.fullmatch(version) is None:
            raise PolicyError(f"工具 {name} 必须固定到完整三段版本")

    _validate_local_patch_policy(policy)
    _validate_runtime_feature_tree_policy(policy)

    seen_services: set[tuple[str, str, str]] = set()
    for index, item in enumerate(_list(policy["service_images"], "service_images")):
        service_image = _object(item, f"服务镜像[{index}]")
        _exact_keys(
            service_image,
            {"workflow", "job", "service", "image"},
            f"服务镜像[{index}]",
        )
        workflow = _text(service_image["workflow"], f"服务镜像[{index}].workflow")
        if (
            Path(workflow).name != workflow
            or re.fullmatch(r"[A-Za-z0-9_.-]+\.ya?ml", workflow) is None
        ):
            raise PolicyError(f"服务镜像[{index}].workflow 必须是安全的工作流文件名")
        job = _text(service_image["job"], f"服务镜像[{index}].job")
        service = _text(service_image["service"], f"服务镜像[{index}].service")
        image = _text(service_image["image"], f"服务镜像[{index}].image")
        image_name = image.partition("@sha256:")[0]
        if (
            SERVICE_IMAGE_REF.fullmatch(image) is None
            or ":" not in image_name.rsplit("/", 1)[-1]
        ):
            raise PolicyError(
                f"服务镜像[{index}].image 必须固定到完整 tag 和 sha256 digest"
            )
        identity = (workflow, job, service)
        if identity in seen_services:
            raise PolicyError(f"服务镜像[{index}] 重复声明 {workflow}/{job}/{service}")
        seen_services.add(identity)

    gate = _object(policy["vulnerability_gate"], "vulnerability_gate")
    _exact_keys(gate, {"severities", "exceptions"}, "vulnerability_gate")
    severities = _list(gate["severities"], "vulnerability_gate.severities")
    if len(severities) != len(set(severities)) or set(severities) != {
        "HIGH",
        "CRITICAL",
    }:
        raise PolicyError("漏洞门禁必须且只能覆盖 HIGH、CRITICAL")

    seen_vulnerabilities: set[tuple[str, str, str, str | None]] = set()
    for index, item in enumerate(
        _list(gate["exceptions"], "vulnerability_gate.exceptions")
    ):
        exception = _object(item, f"漏洞例外[{index}]")
        allowed = {
            "id",
            "package",
            "installed_version",
            "owner",
            "expires",
            "reason",
            "target",
        }
        required = allowed - {"target"}
        actual = set(exception)
        if not required.issubset(actual) or not actual.issubset(allowed):
            raise PolicyError(f"漏洞例外[{index}] 字段不完整或包含未知字段")
        advisory_id = _text(exception["id"], f"漏洞例外[{index}].id")
        if re.fullmatch(r"[A-Z0-9][A-Z0-9._:-]+", advisory_id) is None:
            raise PolicyError(f"漏洞例外[{index}].id 格式无效")
        package = _text(exception["package"], f"漏洞例外[{index}].package")
        installed = _text(
            exception["installed_version"],
            f"漏洞例外[{index}].installed_version",
        )
        target = exception.get("target")
        if target is not None:
            target = _text(target, f"漏洞例外[{index}].target")
        _text(exception["owner"], f"漏洞例外[{index}].owner", minimum=3)
        _expiry(exception["expires"], f"漏洞例外[{index}].expires", check_date)
        _text(exception["reason"], f"漏洞例外[{index}].reason", minimum=12)
        identity = (advisory_id, package, installed, target)
        if identity in seen_vulnerabilities:
            raise PolicyError(f"漏洞例外[{index}] 重复")
        seen_vulnerabilities.add(identity)

    seen_packages: set[tuple[str, str]] = set()
    graph_exceptions = _list(
        policy["dependency_graph_exceptions"],
        "dependency_graph_exceptions",
    )
    for index, item in enumerate(graph_exceptions):
        exception = _object(item, f"依赖图例外[{index}]")
        _exact_keys(
            exception,
            {"package", "version", "enforcement", "owner", "expires", "reason"},
            f"依赖图例外[{index}]",
        )
        package = _text(exception["package"], f"依赖图例外[{index}].package")
        version = _text(exception["version"], f"依赖图例外[{index}].version")
        if exception["enforcement"] != "must_be_absent_from_resolved_graph":
            raise PolicyError(f"依赖图例外[{index}] 必须使用失败关闭的执行方式")
        _text(exception["owner"], f"依赖图例外[{index}].owner", minimum=3)
        _expiry(exception["expires"], f"依赖图例外[{index}].expires", check_date)
        _text(exception["reason"], f"依赖图例外[{index}].reason", minimum=12)
        identity = (package, version)
        if identity in seen_packages:
            raise PolicyError(f"依赖图例外[{index}] 重复")
        seen_packages.add(identity)

    return policy


def _is_link_like(path: Path) -> bool:
    if path.is_symlink():
        return True
    is_junction = getattr(path, "is_junction", None)
    return bool(is_junction is not None and is_junction())


def _resolve_regular_path(
    base: Path,
    relative: str,
    label: str,
    *,
    directory: bool,
) -> tuple[Path | None, str | None]:
    current = base
    for part in relative.split("/"):
        current /= part
        if current.exists() and _is_link_like(current):
            return None, f"{label} 不得经过符号链接或目录联接：{current}"
    try:
        base_resolved = base.resolve(strict=True)
        resolved = current.resolve(strict=True)
    except OSError as exc:
        return None, f"{label} 不存在或无法读取：{current} ({exc})"
    if not resolved.is_relative_to(base_resolved):
        return None, f"{label} 逃逸出允许目录：{resolved}"
    if directory and not resolved.is_dir():
        return None, f"{label} 必须是目录：{resolved}"
    if not directory and not resolved.is_file():
        return None, f"{label} 必须是普通文件：{resolved}"
    return resolved, None


def _read_toml(path: Path, label: str) -> tuple[dict[str, Any] | None, str | None]:
    try:
        document = tomllib.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, tomllib.TOMLDecodeError) as exc:
        return None, f"无法读取 {label} {path}：{exc}"
    return document, None


def _local_path_patches(
    root: Path,
) -> tuple[dict[str, tuple[str, str]], list[str]]:
    manifest, error = _read_toml(root / "Cargo.toml", "Workspace Cargo.toml")
    if error is not None or manifest is None:
        return {}, [error or "Workspace Cargo.toml 无法解析"]
    patch_table = manifest.get("patch", {})
    if not isinstance(patch_table, dict):
        return {}, ["Workspace Cargo.toml 的 [patch] 必须是表"]
    crates_io = patch_table.get("crates-io", {})
    if not isinstance(crates_io, dict):
        return {}, ["Workspace Cargo.toml 的 [patch.crates-io] 必须是表"]

    patches: dict[str, tuple[str, str]] = {}
    errors: list[str] = []
    for patch_name, specification in crates_io.items():
        if not isinstance(specification, dict) or "path" not in specification:
            continue
        label = f"[patch.crates-io].{patch_name}.path"
        try:
            path = _portable_path(specification["path"], label)
        except PolicyError as exc:
            errors.append(str(exc))
            continue
        identity = str(patch_name).casefold()
        if identity in patches:
            errors.append(f"Cargo 本地 patch 名称大小写冲突：{patch_name}")
            continue
        patches[identity] = (str(patch_name), path)
    return patches, errors


def _registry_source_paths(
    cargo_home: Path,
    source: dict[str, Any],
) -> list[Path]:
    registry_root = cargo_home / "registry" / "src"
    if not registry_root.is_dir():
        return []
    package_dir = f"{source['package']}-{source['version']}"
    return sorted(
        candidate / package_dir
        for candidate in registry_root.iterdir()
        if candidate.is_dir() and (candidate / package_dir).is_dir()
    )


def _canonical_license_bytes(content: bytes) -> bytes:
    """许可证是文本事实源；统一换行，避免 Git checkout 平台改变哈希。"""

    return content.replace(b"\r\n", b"\n").replace(b"\r", b"\n")


def _validate_registry_copy(
    cargo_home: Path,
    source: dict[str, Any],
    vendor_bytes: bytes,
    expected_hash: str,
    label: str,
) -> list[str]:
    errors: list[str] = []
    package_roots = _registry_source_paths(cargo_home, source)
    for package_root in package_roots:
        source_path, error = _resolve_regular_path(
            package_root,
            source["file"],
            f"{label} registry 原件",
            directory=False,
        )
        if error is not None or source_path is None:
            errors.append(error or f"{label} registry 原件无法读取")
            continue
        source_bytes = _canonical_license_bytes(source_path.read_bytes())
        source_hash = hashlib.sha256(source_bytes).hexdigest()
        if source_hash != expected_hash or source_bytes != vendor_bytes:
            errors.append(
                f"{label} 与精确 registry 原件不一致：{source_path} "
                f"(期望 {expected_hash}，实际 {source_hash})"
            )
    return errors


def validate_local_patch_licenses(
    root: Path,
    policy: dict[str, Any],
    *,
    cargo_home: Path | None = None,
) -> list[str]:
    actual, errors = _local_path_patches(root)
    configured = {
        patch["patch"].casefold(): patch for patch in policy["local_patch_licenses"]
    }
    for identity, (patch_name, path) in actual.items():
        entry = configured.get(identity)
        if entry is None:
            errors.append(f"本地 patch 未登记许可证原件：{patch_name} ({path})")
        elif entry["path"] != path:
            errors.append(
                f"本地 patch 路径与策略不一致：{patch_name} "
                f"Cargo={path}，策略={entry['path']}"
            )
    for identity, entry in configured.items():
        if identity not in actual:
            errors.append(
                f"许可证策略包含不存在的本地 patch：{entry['patch']} ({entry['path']})"
            )

    selected_cargo_home = cargo_home
    if selected_cargo_home is None:
        selected_cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    for identity, entry in configured.items():
        actual_patch = actual.get(identity)
        if actual_patch is None or actual_patch[1] != entry["path"]:
            continue
        crate_root, error = _resolve_regular_path(
            root,
            entry["path"],
            f"本地 patch {entry['patch']}",
            directory=True,
        )
        if error is not None or crate_root is None:
            errors.append(error or f"本地 patch {entry['patch']} 无法读取")
            continue
        manifest, error = _read_toml(
            crate_root / "Cargo.toml", f"本地 patch {entry['patch']} Cargo.toml"
        )
        if error is not None or manifest is None:
            errors.append(error or f"本地 patch {entry['patch']} Cargo.toml 无法解析")
            continue
        package = manifest.get("package")
        if not isinstance(package, dict):
            errors.append(f"本地 patch {entry['patch']} 缺少 [package]")
            continue
        expected_manifest = {
            "name": entry["package"],
            "version": entry["version"],
            "license": entry["license_expression"],
        }
        for field, expected_field in expected_manifest.items():
            if package.get(field) != expected_field:
                errors.append(
                    f"本地 patch {entry['patch']} 的 package.{field} 不一致："
                    f"期望 {expected_field}，实际 {package.get(field)}"
                )
        if "license-file" in package:
            errors.append(
                f"本地 patch {entry['patch']} 使用未建模的 package.license-file"
            )

        for license_file in entry["license_files"]:
            label = f"本地 patch {entry['patch']} {license_file['spdx']}"
            path, error = _resolve_regular_path(
                crate_root,
                license_file["file"],
                f"{label} 文件",
                directory=False,
            )
            if error is not None or path is None:
                errors.append(error or f"{label} 文件无法读取")
                continue
            vendor_bytes = _canonical_license_bytes(path.read_bytes())
            actual_hash = hashlib.sha256(vendor_bytes).hexdigest()
            expected_hash = license_file["sha256"]
            if actual_hash != expected_hash:
                errors.append(
                    f"{label} 登记哈希不一致：期望 {expected_hash}，实际 {actual_hash}"
                )
            errors.extend(
                _validate_registry_copy(
                    selected_cargo_home,
                    license_file["registry_source"],
                    vendor_bytes,
                    expected_hash,
                    label,
                )
            )
    return errors


if yaml is not None:

    class StrictWorkflowLoader(yaml.SafeLoader):
        """保留 YAML alias 语义，并拒绝会遮蔽安全配置的重复键。"""

        yaml_implicit_resolvers = {
            key: [
                (tag, pattern)
                for tag, pattern in resolvers
                if tag != "tag:yaml.org,2002:bool"
            ]
            for key, resolvers in yaml.SafeLoader.yaml_implicit_resolvers.items()
        }

        def construct_mapping(self, node: Any, deep: bool = False) -> dict[Any, Any]:
            if not isinstance(node, MappingNode):
                raise ConstructorError(
                    None,
                    None,
                    "期望 YAML 对象",
                    node.start_mark,
                )
            self.flatten_mapping(node)
            mapping: dict[Any, Any] = {}
            for key_node, value_node in node.value:
                key = self.construct_object(key_node, deep=deep)
                try:
                    duplicate = key in mapping
                except TypeError as exc:
                    raise ConstructorError(
                        "构造 YAML 对象时",
                        node.start_mark,
                        "对象键必须可哈希",
                        key_node.start_mark,
                    ) from exc
                if duplicate:
                    raise ConstructorError(
                        "构造 YAML 对象时",
                        node.start_mark,
                        f"发现重复键 {key!r}",
                        key_node.start_mark,
                    )
                mapping[key] = self.construct_object(value_node, deep=deep)
            return mapping

    StrictWorkflowLoader.add_implicit_resolver(
        "tag:yaml.org,2002:bool",
        re.compile(r"^(?:true|false)$", re.IGNORECASE),
        list("tTfF"),
    )


def _load_workflow(path: Path, text: str) -> tuple[dict[str, Any] | None, list[str]]:
    """完整解析工作流；缺少解析器或遇到未知结构时失败关闭。"""

    if yaml is None:
        return None, [f"{path}: 缺少 PyYAML，无法安全校验工作流供应链配置"]
    try:
        document = yaml.load(text, Loader=StrictWorkflowLoader)
    except yaml.YAMLError as exc:
        return None, [f"{path}: 工作流 YAML 无法安全解析：{exc}"]
    if not isinstance(document, dict):
        return None, [f"{path}: 工作流根节点必须是对象"]
    return document, []


def _workflow_jobs(
    path: Path,
    document: dict[str, Any],
) -> tuple[list[tuple[str, dict[str, Any]]], list[str]]:
    jobs = document.get("jobs")
    if not isinstance(jobs, dict):
        return [], [f"{path}: jobs 必须是对象"]

    valid: list[tuple[str, dict[str, Any]]] = []
    errors: list[str] = []
    for job_name, job in jobs.items():
        if not isinstance(job_name, str) or not isinstance(job, dict):
            errors.append(f"{path}: jobs 中的任务名称和配置必须分别是字符串、对象")
            continue
        valid.append((job_name, job))
    return valid, errors


def _contains_expression_reference(value: object, name: str) -> bool:
    if not isinstance(value, str) or "${{" not in value:
        return False
    pattern = rf"(?<![A-Za-z0-9_]){re.escape(name)}\s*(?:\.|\[)"
    return re.search(pattern, value) is not None


def _validate_environment_contexts(
    path: Path,
    document: dict[str, Any],
    jobs: list[tuple[str, dict[str, Any]]],
) -> list[str]:
    """阻止在 GitHub 尚未创建 runner/step 的 env 层级引用运行期上下文。"""

    errors: list[str] = []
    scopes = [
        (
            "工作流 env",
            document.get("env"),
            {"runner", "job", "steps", "env", "needs", "strategy", "matrix"},
        )
    ]
    scopes.extend(
        (f"job {job_name} env", job.get("env"), {"runner", "job", "steps", "env"})
        for job_name, job in jobs
    )
    for location, environment, forbidden in scopes:
        if environment is None:
            continue
        if not isinstance(environment, dict):
            errors.append(f"{path}: {location} 必须是对象")
            continue
        for variable, value in environment.items():
            for name in sorted(forbidden):
                if _contains_expression_reference(value, name):
                    errors.append(
                        f"{path}: {location}.{variable} 不能引用 {name} 上下文；"
                        "请下沉到具体 step 的 env、with 或 run"
                    )
            if (
                isinstance(value, str)
                and "${{" in value
                and re.search(r"\bhashFiles\s*\(", value)
            ):
                errors.append(
                    f"{path}: {location}.{variable} 不能调用 hashFiles；"
                    "请下沉到具体 step 的 env 或 with"
                )
    return errors


def _collect_service_images(
    path: Path,
    jobs: list[tuple[str, dict[str, Any]]],
) -> tuple[dict[tuple[str, str, str], str], list[str]]:
    images: dict[tuple[str, str, str], str] = {}
    errors: list[str] = []
    for job_name, job in jobs:
        if "services" not in job:
            continue
        services = job["services"]
        if not isinstance(services, dict):
            errors.append(f"{path}: job {job_name} 的 services 必须是对象")
            continue
        for service_name, service in services.items():
            if not isinstance(service_name, str) or not isinstance(service, dict):
                errors.append(
                    f"{path}: job {job_name} 的 service 名称和配置必须分别是字符串、对象"
                )
                continue
            image = service.get("image")
            if image is None:
                errors.append(
                    f"{path}: job {job_name} 的 service {service_name} 缺少固定 image"
                )
                continue
            if not isinstance(image, str):
                errors.append(
                    f"{path}: job {job_name} 的 service {service_name} "
                    f"镜像未固定到 sha256 digest：{image!r}"
                )
                continue
            images[(path.name, job_name, service_name)] = image
            if SERVICE_IMAGE_REF.fullmatch(image) is None:
                errors.append(
                    f"{path}: job {job_name} 的 service {service_name} "
                    f"镜像未固定到 sha256 digest：{image!r}"
                )
    return images, errors


def _validate_service_images(
    path: Path,
    jobs: list[tuple[str, dict[str, Any]]],
) -> list[str]:
    _, errors = _collect_service_images(path, jobs)
    return errors


def _expected_service_images(
    policy: dict[str, Any],
) -> dict[tuple[str, str, str], str]:
    return {
        (item["workflow"], item["job"], item["service"]): item["image"]
        for item in policy["service_images"]
    }


def _compare_service_images(
    expected: dict[tuple[str, str, str], str],
    actual: dict[tuple[str, str, str], str],
) -> list[str]:
    errors: list[str] = []
    for workflow, job, service in sorted(expected.keys() - actual.keys()):
        errors.append(f"策略声明的服务镜像不存在于工作流：{workflow}/{job}/{service}")
    for workflow, job, service in sorted(actual.keys() - expected.keys()):
        errors.append(f"工作流服务镜像未在策略声明：{workflow}/{job}/{service}")
    for identity in sorted(expected.keys() & actual.keys()):
        expected_image = expected[identity]
        actual_image = actual[identity]
        if expected_image == actual_image:
            continue
        workflow, job, service = identity
        errors.append(
            f"服务镜像引用漂移 {workflow}/{job}/{service}："
            f"期望 {expected_image}，实际 {actual_image}"
        )
    return errors


def _valid_local_reference(reference: str, pattern: re.Pattern[str]) -> bool:
    return pattern.fullmatch(reference) is not None and ".." not in reference.split("/")


def _validate_action_reference(
    path: Path,
    location: str,
    reference: Any,
    *,
    reusable_workflow: bool,
) -> list[str]:
    if not isinstance(reference, str):
        return [f"{path}: {location}.uses 必须是静态字符串"]

    if reusable_workflow:
        valid = (
            _valid_local_reference(reference, LOCAL_REUSABLE_WORKFLOW_REF)
            if reference.startswith("./")
            else REUSABLE_WORKFLOW_REF.fullmatch(reference) is not None
        )
    elif reference.startswith("./"):
        valid = _valid_local_reference(reference, LOCAL_ACTION_REF)
    elif reference.startswith("docker://"):
        valid = CONTAINER_ACTION_REF.fullmatch(reference) is not None
    else:
        valid = ACTION_REF.fullmatch(reference) is not None

    if valid:
        return []
    kind = "可复用工作流" if reusable_workflow else "action"
    return [
        f"{path}: {location} 的 {kind} 未固定到提交 SHA 或镜像 digest：{reference!r}"
    ]


def _validate_install_action(
    path: Path,
    location: str,
    step: dict[str, Any],
    policy: dict[str, Any],
) -> tuple[list[str], list[tuple[str, str]]]:
    errors: list[str] = []
    installed: list[tuple[str, str]] = []
    configuration = step.get("with")
    if not isinstance(configuration, dict):
        return [f"{path}: {location} 的 install-action.with 必须是对象"], installed
    if configuration.get("fallback") != "none":
        errors.append(f"{path}: {location} 的 install-action 必须禁用 fallback")

    raw_tools = configuration.get("tool")
    if not isinstance(raw_tools, str) or not raw_tools.strip():
        errors.append(f"{path}: {location} 的 install-action.tool 必须是非空字符串")
        return errors, installed

    seen: set[str] = set()
    for raw_tool in raw_tools.split(","):
        specification = raw_tool.strip()
        matched = INSTALL_TOOL_REF.fullmatch(specification)
        if matched is None:
            errors.append(
                f"{path}: {location} 的 install-action 工具未固定到完整版本："
                f"{specification!r}"
            )
            continue
        name, version = matched.groups()
        if name in seen:
            errors.append(f"{path}: {location} 的 install-action 重复声明工具 {name}")
            continue
        seen.add(name)
        expected = policy["tools"].get(name)
        if expected is None:
            errors.append(f"{path}: {location} 的工具 {name} 未在供应链策略声明")
            continue
        if version != expected:
            errors.append(f"工具 {name} 版本漂移：期望 {expected}，实际 {version}")
            continue
        installed.append((name, version))
    return errors, installed


def _validate_action_uses(
    path: Path,
    jobs: list[tuple[str, dict[str, Any]]],
    policy: dict[str, Any],
) -> tuple[
    list[str],
    list[tuple[str, str]],
    list[str],
    list[tuple[Path, str, str]],
]:
    errors: list[str] = []
    installed: list[tuple[str, str]] = []
    references: list[str] = []
    run_blocks: list[tuple[Path, str, str]] = []
    for job_name, job in jobs:
        job_location = f"jobs.{job_name}"
        if "uses" in job:
            reference = job["uses"]
            errors.extend(
                _validate_action_reference(
                    path,
                    job_location,
                    reference,
                    reusable_workflow=True,
                )
            )
            if isinstance(reference, str):
                references.append(reference)

        if "steps" not in job:
            continue
        steps = job["steps"]
        if not isinstance(steps, list):
            errors.append(f"{path}: {job_location}.steps 必须是数组")
            continue
        for index, step in enumerate(steps):
            location = f"{job_location}.steps[{index}]"
            if not isinstance(step, dict):
                errors.append(f"{path}: {location} 必须是对象")
                continue
            if "run" in step:
                run = step["run"]
                if not isinstance(run, str):
                    errors.append(f"{path}: {location}.run 必须是静态字符串")
                else:
                    run_blocks.append((path, location, run))
            if "uses" not in step:
                continue
            reference = step["uses"]
            errors.extend(
                _validate_action_reference(
                    path,
                    location,
                    reference,
                    reusable_workflow=False,
                )
            )
            if not isinstance(reference, str):
                continue
            references.append(reference)
            if reference.lower().startswith("taiki-e/install-action@"):
                install_errors, install_tools = _validate_install_action(
                    path,
                    location,
                    step,
                    policy,
                )
                errors.extend(install_errors)
                installed.extend(install_tools)
    return errors, installed, references, run_blocks


def _logical_command_lines(script: str) -> list[str]:
    """合并 Bash、PowerShell 和 cmd 的显式续行，保留真正的命令边界。"""

    normalized = re.sub(r"(?:\\|`|\^)\r?\n[ \t]*", " ", script)
    return [line.strip() for line in normalized.splitlines() if line.strip()]


def _shell_command_segments(line: str) -> list[list[str]]:
    lexer = shlex.shlex(line, posix=True, punctuation_chars="();&|")
    lexer.whitespace_split = True
    lexer.commenters = "#"
    segments: list[list[str]] = []
    current: list[str] = []
    for token in lexer:
        if token and not token.strip("();&|"):
            if current:
                segments.append(current)
                current = []
        else:
            current.append(token)
    if current:
        segments.append(current)
    return segments


def _command_tokens(tokens: list[str]) -> list[str]:
    """移除不会改变可执行文件身份的静态 shell 前缀。"""

    command = list(tokens)
    while command:
        first = command[0]
        if first in {
            "!",
            "command",
            "do",
            "elif",
            "if",
            "sudo",
            "then",
            "until",
            "while",
        }:
            command.pop(0)
            continue
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", first):
            command.pop(0)
            continue
        break
    return command


def _gate_command_tokens(tokens: list[str]) -> list[str]:
    """供应链必备命令只能位于非注释命令行的静态起点。"""

    command = list(tokens)
    while command and re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*=.*", command[0]):
        command.pop(0)
    if command and command[0] == "command":
        command.pop(0)
    return command


def _has_option(tokens: list[str], name: str, value: str) -> bool:
    for index, token in enumerate(tokens):
        if token == f"{name}={value}":
            return True
        if token == name and index + 1 < len(tokens) and tokens[index + 1] == value:
            return True
    return False


def _has_value_option(tokens: list[str], name: str) -> bool:
    for index, token in enumerate(tokens):
        if token.startswith(f"{name}=") and token != f"{name}=":
            return True
        if (
            token == name
            and index + 1 < len(tokens)
            and not tokens[index + 1].startswith("-")
        ):
            return True
    return False


def _typed_security_report(tokens: list[str]) -> tuple[str | None, bool, list[str]]:
    """解析 workflow 中唯一允许的供应链报告核验入口。"""

    prefix = ["cargo", "xtask", "check", "ci", "security", "report"]
    if tokens[: len(prefix)] != prefix:
        return None, False, []
    errors: list[str] = []
    if len(tokens) == len(prefix):
        return None, False, ["security report 缺少报告类型"]
    kind = tokens[len(prefix)]
    if kind not in {"cyclonedx", "trivy"}:
        return None, False, [f"security report 类型无效：{kind}"]
    input_seen = False
    reproducible = False
    index = len(prefix) + 1
    while index < len(tokens):
        option = tokens[index]
        if option == "--input":
            if input_seen:
                errors.append("security report --input 不能重复")
            if index + 1 >= len(tokens) or tokens[index + 1].startswith("-"):
                errors.append("security report --input 缺少取值")
                index += 1
                continue
            input_seen = True
            index += 2
            continue
        if option == "--require-reproducible":
            if kind != "cyclonedx":
                errors.append("security report trivy 不支持 --require-reproducible")
            elif reproducible:
                errors.append("security report --require-reproducible 不能重复")
            reproducible = True
            index += 1
            continue
        errors.append(f"security report 包含未知参数：{option}")
        index += 1
    if not input_seen:
        errors.append("security report 缺少 --input <报告>")
    return kind, reproducible, errors


DOCKER_RUN_FLAG_OPTIONS = {
    "--detach",
    "--init",
    "--interactive",
    "--privileged",
    "--read-only",
    "--rm",
    "--sig-proxy",
    "--tty",
    "-d",
    "-i",
    "-t",
}
DOCKER_RUN_VALUE_OPTIONS = {
    "--add-host",
    "--cap-add",
    "--cap-drop",
    "--cpus",
    "--entrypoint",
    "--env",
    "--env-file",
    "--hostname",
    "--label",
    "--memory",
    "--mount",
    "--name",
    "--network",
    "--platform",
    "--publish",
    "--pull",
    "--restart",
    "--security-opt",
    "--user",
    "--volume",
    "--workdir",
    "-e",
    "-h",
    "-l",
    "-m",
    "-p",
    "-u",
    "-v",
    "-w",
}
DOCKER_PULL_FLAG_OPTIONS = {"--all-tags", "--quiet", "-a", "-q"}
DOCKER_PULL_VALUE_OPTIONS = {"--platform"}


def _docker_image(tokens: list[str]) -> tuple[str | None, str | None]:
    operation = tokens[1]
    if operation == "run":
        flags = DOCKER_RUN_FLAG_OPTIONS
        values = DOCKER_RUN_VALUE_OPTIONS
    else:
        flags = DOCKER_PULL_FLAG_OPTIONS
        values = DOCKER_PULL_VALUE_OPTIONS

    index = 2
    while index < len(tokens):
        token = tokens[index]
        if token == "--":
            index += 1
            break
        if token in flags:
            index += 1
            continue
        if token in values:
            if index + 1 >= len(tokens):
                return None, f"选项 {token} 缺少值"
            index += 2
            continue
        if token.startswith("--") and "=" in token:
            index += 1
            continue
        if token.startswith("-"):
            return None, f"无法识别选项 {token}"
        return token, None
    if index < len(tokens):
        return tokens[index], None
    return None, "缺少镜像参数"


def _validate_run_commands(
    run_blocks: list[tuple[Path, str, str]],
) -> list[str]:
    errors: list[str] = []
    cargo_cyclonedx = False
    trivy_cyclonedx = False
    cyclonedx_reports = 0
    reproducible_cyclonedx_reports = 0
    trivy_reports = 0

    for path, location, script in run_blocks:
        for line in _logical_command_lines(script):
            if line.startswith("#"):
                continue
            try:
                segments = _shell_command_segments(line)
            except ValueError as exc:
                if re.search(
                    r"\b(?:cargo\s+cyclonedx|trivy\s+image|"
                    r"check_supply_chain\.py|docker\s+(?:run|pull))\b",
                    line,
                ):
                    errors.append(f"{path}: {location}.run 无法安全解析命令：{exc}")
                continue
            for segment_index, raw_tokens in enumerate(segments):
                tokens = _command_tokens(raw_tokens)
                gate_tokens = (
                    _gate_command_tokens(raw_tokens) if segment_index == 0 else []
                )
                if len(gate_tokens) >= 2 and gate_tokens[:2] == ["cargo", "cyclonedx"]:
                    if _has_option(gate_tokens, "--format", "json"):
                        cargo_cyclonedx = True
                    continue
                if len(gate_tokens) >= 2 and gate_tokens[:2] == ["trivy", "image"]:
                    if _has_option(gate_tokens, "--format", "cyclonedx"):
                        trivy_cyclonedx = True
                    continue
                report_kind, reproducible, report_errors = _typed_security_report(
                    gate_tokens
                )
                if report_kind is not None or report_errors:
                    errors.extend(
                        f"{path}: {location}.run 的 {error}"
                        for error in report_errors
                    )
                    if not report_errors and report_kind == "cyclonedx":
                        cyclonedx_reports += 1
                        reproducible_cyclonedx_reports += int(reproducible)
                    if not report_errors and report_kind == "trivy":
                        trivy_reports += 1
                    continue
                if (
                    len(gate_tokens) >= 2
                    and gate_tokens[0] in {"python", "python.exe", "python3", "py"}
                    and gate_tokens[1]
                    in {
                        "tools/python/check_supply_chain.py",
                        "./tools/python/check_supply_chain.py",
                    }
                ):
                    if _has_value_option(gate_tokens, "--trivy-report") or _has_value_option(
                        gate_tokens, "--cyclonedx"
                    ):
                        errors.append(
                            f"{path}: {location}.run 禁止直接调用 "
                            "check_supply_chain.py 核验报告；必须使用 "
                            "cargo xtask check ci security report"
                        )
                    continue
                if (
                    len(tokens) >= 2
                    and tokens[0] == "docker"
                    and tokens[1]
                    in {
                        "pull",
                        "run",
                    }
                ):
                    image, image_error = _docker_image(tokens)
                    if image_error is not None:
                        errors.append(
                            f"{path}: {location}.run 的 docker {tokens[1]} "
                            f"无法安全识别镜像：{image_error}"
                        )
                    elif image is None or SERVICE_IMAGE_REF.fullmatch(image) is None:
                        errors.append(
                            f"{path}: {location}.run 的 docker {tokens[1]} "
                            "镜像未固定到 sha256 digest"
                        )

    if not cargo_cyclonedx:
        errors.append("工作流缺少供应链门禁：cargo cyclonedx --format json")
    if not trivy_cyclonedx:
        errors.append("工作流缺少供应链门禁：trivy image --format cyclonedx")
    if cyclonedx_reports != 2:
        errors.append(
            "工作流必须恰好两次使用 cargo xtask check ci security report "
            "cyclonedx --input <报告>，分别核验 Cargo 与镜像清单"
        )
    if reproducible_cyclonedx_reports != 1:
        errors.append(
            "工作流必须恰好一次为 Cargo CycloneDX 核验使用 "
            "--require-reproducible"
        )
    if trivy_reports != 1:
        errors.append(
            "工作流必须恰好一次使用 cargo xtask check ci security report "
            "trivy --input <报告>"
        )
    return errors


def validate_service_images(path: Path, text: str) -> list[str]:
    """校验 jobs.*.services.*.image 使用不可变 OCI digest。"""

    document, errors = _load_workflow(path, text)
    if document is None:
        return errors
    jobs, job_errors = _workflow_jobs(path, document)
    return [*errors, *job_errors, *_validate_service_images(path, jobs)]


def validate_action_uses(
    path: Path,
    text: str,
    policy: dict[str, Any],
) -> list[str]:
    """从 YAML AST 校验 job-level workflow 与 step action 引用。"""

    document, errors = _load_workflow(path, text)
    if document is None:
        return errors
    jobs, job_errors = _workflow_jobs(path, document)
    action_errors, _, _, _ = _validate_action_uses(path, jobs, policy)
    return [*errors, *job_errors, *action_errors]


def validate_workflows(workflow_dir: Path, policy: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    files = sorted((*workflow_dir.glob("*.yml"), *workflow_dir.glob("*.yaml")))
    if not files:
        return [f"没有找到工作流：{workflow_dir}"]

    installed_tools: list[tuple[str, str]] = []
    action_references: list[str] = []
    run_blocks: list[tuple[Path, str, str]] = []
    service_images: dict[tuple[str, str, str], str] = {}
    for path in files:
        text = path.read_text(encoding="utf-8")
        document, parse_errors = _load_workflow(path, text)
        errors.extend(parse_errors)
        if document is not None:
            jobs, job_errors = _workflow_jobs(path, document)
            errors.extend(job_errors)
            errors.extend(_validate_environment_contexts(path, document, jobs))
            workflow_images, image_errors = _collect_service_images(path, jobs)
            errors.extend(image_errors)
            service_images.update(workflow_images)
            (
                action_errors,
                workflow_tools,
                workflow_references,
                workflow_runs,
            ) = _validate_action_uses(path, jobs, policy)
            errors.extend(action_errors)
            installed_tools.extend(workflow_tools)
            action_references.extend(workflow_references)
            run_blocks.extend(workflow_runs)

    errors.extend(
        _compare_service_images(_expected_service_images(policy), service_images)
    )

    for name, version in policy["tools"].items():
        occurrences = [actual for tool, actual in installed_tools if tool == name]
        if not occurrences:
            errors.append(f"工作流没有安装策略声明的工具 {name}@{version}")

    errors.extend(_validate_run_commands(run_blocks))
    if not any(
        reference.startswith("actions/upload-artifact@")
        for reference in action_references
    ):
        errors.append("工作流缺少供应链门禁：actions/upload-artifact")
    return errors


def validate_cyclonedx(path: Path, *, require_reproducible: bool = False) -> list[str]:
    try:
        document = _object(json.loads(path.read_text(encoding="utf-8")), "CycloneDX")
    except (OSError, json.JSONDecodeError, PolicyError) as exc:
        return [f"无法读取 CycloneDX 清单 {path}: {exc}"]
    errors: list[str] = []
    if document.get("bomFormat") != "CycloneDX":
        errors.append("SBOM 的 bomFormat 必须是 CycloneDX")
    if document.get("specVersion") not in {"1.5", "1.6", "1.7"}:
        errors.append("SBOM 规范版本不得低于 1.5")
    components = document.get("components")
    if not isinstance(components, list) or not components:
        errors.append("SBOM 必须包含非空 components")
    metadata = document.get("metadata")
    if not isinstance(metadata, dict) or not isinstance(
        metadata.get("component"), dict
    ):
        errors.append("SBOM 必须声明顶层 metadata.component")
    if require_reproducible and document.get("serialNumber") is not None:
        errors.append("可复现 SBOM 不得包含随机 serialNumber")
    return errors


def evaluate_trivy_report(report_path: Path, policy: dict[str, Any]) -> list[str]:
    try:
        report = _object(
            json.loads(report_path.read_text(encoding="utf-8")), "Trivy 报告"
        )
    except (OSError, json.JSONDecodeError, PolicyError) as exc:
        return [f"无法读取 Trivy 报告 {report_path}: {exc}"]
    results = report.get("Results")
    if not isinstance(results, list):
        return ["Trivy 报告缺少 Results 数组"]

    gate = policy["vulnerability_gate"]
    severities = set(gate["severities"])
    exceptions = gate["exceptions"]
    used: set[int] = set()
    errors: list[str] = []
    for result in results:
        if not isinstance(result, dict):
            errors.append("Trivy Results 包含非对象条目")
            continue
        target = result.get("Target")
        vulnerabilities = result.get("Vulnerabilities") or []
        if not isinstance(vulnerabilities, list):
            errors.append(f"Trivy 目标 {target!r} 的 Vulnerabilities 不是数组")
            continue
        for vulnerability in vulnerabilities:
            if not isinstance(vulnerability, dict):
                errors.append(f"Trivy 目标 {target!r} 包含无效漏洞条目")
                continue
            severity = vulnerability.get("Severity")
            if severity not in severities:
                continue
            advisory_id = vulnerability.get("VulnerabilityID")
            package = vulnerability.get("PkgName")
            installed = vulnerability.get("InstalledVersion")
            matched = False
            for index, exception in enumerate(exceptions):
                if (
                    exception["id"] == advisory_id
                    and exception["package"] == package
                    and exception["installed_version"] == installed
                    and (
                        exception.get("target") is None or exception["target"] == target
                    )
                ):
                    used.add(index)
                    matched = True
                    break
            if not matched:
                errors.append(
                    f"未放行的 {severity} 漏洞：{advisory_id} "
                    f"{package}@{installed} target={target}"
                )
    for index, exception in enumerate(exceptions):
        if index not in used:
            errors.append(
                "漏洞例外未被报告使用，必须删除或校正："
                f"{exception['id']} {exception['package']}@{exception['installed_version']}"
            )
    return errors


def resolved_graph_violations(tree_output: str, policy: dict[str, Any]) -> list[str]:
    resolved = {line.strip() for line in tree_output.splitlines() if line.strip()}
    errors: list[str] = []
    for exception in policy["dependency_graph_exceptions"]:
        package_id = f"{exception['package']} v{exception['version']}"
        if package_id in resolved:
            errors.append(f"条件依赖例外已进入实际构建图，必须升级：{package_id}")
    return errors


def feature_tree_violations(
    tree_output: str,
    profile: dict[str, Any],
) -> list[str]:
    variant = profile["name"]
    constraints = profile["constraints"]
    features_by_package: dict[str, set[str]] = {}
    package_identities: set[str] = set()
    errors: list[str] = []
    for line_number, raw_line in enumerate(tree_output.splitlines(), start=1):
        line = raw_line.strip()
        if not line:
            continue
        package, separator, raw_features = line.partition("|")
        name, version_separator, _ = package.partition(" v")
        if not separator or not version_separator or not PACKAGE_NAME.fullmatch(name):
            errors.append(f"{variant} Cargo feature tree 第 {line_number} 行格式无效")
            continue
        package_identities.add(package.removesuffix(" (*)"))
        features = raw_features.removesuffix(" (*)")
        features_by_package.setdefault(name, set()).update(
            feature for feature in features.split(",") if feature
        )

    if not package_identities:
        errors.append(f"{variant} Cargo feature tree 没有有效 package")
    maximum = constraints["max_unique_packages"]
    if maximum is not None and len(package_identities) > maximum:
        errors.append(
            f"{variant} 定向运行图 unique package closure 超限："
            f"{len(package_identities)} > {maximum}"
        )
    for package in constraints["forbidden_packages"]:
        if package in features_by_package:
            errors.append(f"{variant} 定向运行图包含禁止的 {package} package/provider")
    for package, forbidden in constraints["forbidden_features"].items():
        enabled = set(forbidden) & features_by_package.get(package, set())
        if enabled:
            errors.append(
                f"{variant} 定向运行图启用了禁止的 {package} provider feature："
                f"{', '.join(sorted(enabled))}"
            )
    for package in constraints["required_packages"]:
        if package not in features_by_package:
            errors.append(f"{variant} 定向运行图缺少 {package}")
    for package, required_features in constraints["required_features"].items():
        required = set(required_features)
        actual = features_by_package.get(package)
        if actual is None:
            errors.append(f"{variant} 定向运行图缺少 {package}")
            continue
        missing = required - actual
        if missing:
            errors.append(
                f"{variant} 定向运行图的 {package} 缺少 AWS-LC feature："
                f"{', '.join(sorted(missing))}"
            )
    return errors


def feature_tree_args(profile: dict[str, Any]) -> list[str]:
    command = profile["command"]
    args = [
        "tree",
        "--locked",
        "-p",
        command["package"],
    ]
    if command["no_default_features"]:
        args.append("--no-default-features")
    if command["features"]:
        args.extend(["--features", ",".join(command["features"])])
    args.extend(
        [
            "--edges",
            ",".join(command["edges"]),
            "--prefix",
            "none",
            "--format",
            "{p}|{f}",
        ]
    )
    return args


def verify_cargo_graph(policy: dict[str, Any]) -> list[str]:
    completed = subprocess.run(
        [
            "cargo",
            "tree",
            "--locked",
            "--workspace",
            "--all-features",
            "--target",
            "all",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        return [f"cargo tree 执行失败：{completed.stderr.strip()}"]
    errors = resolved_graph_violations(completed.stdout, policy)
    gate = policy["runtime_feature_tree_gate"]
    for profile in gate["profiles"]:
        variant = profile["name"]
        completed = subprocess.run(
            ["cargo", *feature_tree_args(profile)],
            cwd=ROOT,
            check=False,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        if completed.returncode != 0:
            errors.append(
                f"{variant} 定向 cargo tree 执行失败：{completed.stderr.strip()}"
            )
            continue
        errors.extend(feature_tree_violations(completed.stdout, profile))
    return errors


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="校验供应链策略、固定版本和安全报告")
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    parser.add_argument("--workflow-dir", type=Path, default=DEFAULT_WORKFLOWS)
    parser.add_argument("--trivy-report", type=Path)
    parser.add_argument("--cyclonedx", type=Path)
    parser.add_argument("--require-reproducible-cyclonedx", action="store_true")
    parser.add_argument("--verify-cargo-graph", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    report_count = int(args.cyclonedx is not None) + int(args.trivy_report is not None)
    if report_count > 1:
        print("参数错误：CycloneDX 与 Trivy 报告必须分别核验", file=sys.stderr)
        return 2
    if args.require_reproducible_cyclonedx and args.cyclonedx is None:
        print(
            "参数错误：--require-reproducible-cyclonedx 只适用于 --cyclonedx",
            file=sys.stderr,
        )
        return 2
    if report_count and args.verify_cargo_graph:
        print("参数错误：报告核验不能同时执行 Cargo 来源图核验", file=sys.stderr)
        return 2

    if args.cyclonedx is not None:
        errors = validate_cyclonedx(
            args.cyclonedx,
            require_reproducible=args.require_reproducible_cyclonedx,
        )
    else:
        try:
            policy = load_policy(args.policy)
        except PolicyError as exc:
            print(f"供应链策略无效：{exc}", file=sys.stderr)
            return 1
        if args.trivy_report is not None:
            errors = evaluate_trivy_report(args.trivy_report, policy)
        else:
            errors = validate_local_patch_licenses(ROOT, policy)
            errors.extend(validate_workflows(args.workflow_dir, policy))
            if args.verify_cargo_graph:
                errors.extend(verify_cargo_graph(policy))
    if errors:
        for error in errors:
            print(f"供应链门禁失败：{error}", file=sys.stderr)
        return 1
    print("供应链策略、固定版本和安全报告校验通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
