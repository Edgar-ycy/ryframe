"""绑定 B0/B1 构建与同一恢复导出使用的精确源码证据。"""

from __future__ import annotations

import copy
import hashlib
import os
from pathlib import Path
import re
import subprocess
import uuid

from devex_clone_model import exact
from devex_clone_seed_export import published_export
from devex_clone_source_proof import bound_file
from reference_fixture_successor import published_source
from restore_build import file_digest, verify_build
from restore_frontend_build import FRONTEND_RECEIPT, frontend_files
from restore_runtime_evidence import (
    read_json_document,
    reject_link_or_reparse,
    validate_frontend_receipt,
)
from source_inventory import (
    build_source_domains,
    canonical_digest,
    capture_inventory,
    frontend_environment_files,
    validate_inventory,
)


B0_BACKEND_COMMIT = "815c5eafb09d4b493319d255fb7ab88ddd02c8b6"
B0_FRONTEND_COMMIT = "0087ea2ecf62530d042b9e52f5c950fb34c66d78"
B0_ADAPTER_COMMIT = "c05114bcdf5c369cd74087db6317ce3c8f89bee8"
B0_ADAPTER_TREE = "2ff15e7f34da1749c7eb27a1a025b9d3811b87a7"
B0_ADAPTER_PATCH = Path("xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch")
B0_ADAPTER_PATCH_SHA256 = "28ad29f21ca9fd58356d059700bcc871172466e0a62286c5ccdbf68b1ed3fba3"
B0_ADAPTER_PATHS = ["xtask/src/cli.rs"]
MANIFEST_FIELDS = {"format_version", "kind", "source_export", "arms"}
ARM_FIELDS = {"roots", "sources", "execution_sources", "builds", "adapter",
              "source_export_result_sha256", "source_export_identity_sha256"}
EXPORT_FIELDS = {"result", "origin_attempt", "source_registration", "source_rebind",
                 "review_successor", "source_request", "export", "export_sha256",
                 "generation_sha256", "logical_inventory_sha256", "identity_sha256"}


def _git(root: Path, *arguments: str, index: Path | None = None) -> bytes:
    environment = os.environ if index is None else {**os.environ, "GIT_INDEX_FILE": str(index)}
    result = subprocess.run(
        ["git", *arguments], cwd=root, env=environment, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, check=False,
        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
    )
    if result.returncode != 0:
        raise ValueError(f"无法核验 B0 适配 Git 对象：git {' '.join(arguments)}")
    return result.stdout


def _text(root: Path, *arguments: str, index: Path | None = None) -> str:
    try:
        return _git(root, *arguments, index=index).decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError:
        raise ValueError("B0 适配 Git 输出不是 UTF-8") from None


def _patch_bytes(root: Path) -> bytes:
    path = root / B0_ADAPTER_PATCH
    if not path.is_file() or path.is_symlink():
        raise ValueError("B0 内嵌适配补丁不是仓库内普通文件")
    return path.read_bytes()


def _reconstructed_tree(root: Path, patch: Path) -> str:
    scratch = root / "target"
    scratch.mkdir(exist_ok=True)
    index = scratch / f"b0-adapter-{os.getpid()}-{uuid.uuid4().hex}.index"
    lock = Path(str(index) + ".lock")
    try:
        _git(root, "read-tree", B0_BACKEND_COMMIT, index=index)
        _git(root, "apply", "--cached", "--whitespace=nowarn", str(patch), index=index)
        return _text(root, "write-tree", index=index)
    finally:
        for path in (lock, index):
            path.unlink(missing_ok=True)


def b0_adapter_evidence(root: Path) -> dict:
    """证明登记提交恰由内嵌工具补丁从原 B0 重建，且未越过工具层。"""
    root = root.resolve(strict=True)
    actual_root = Path(_text(root, "rev-parse", "--show-toplevel")).resolve(strict=True)
    if actual_root != root:
        raise ValueError("B0 适配核验必须从实际后端 Git 根目录执行")
    patch_path = root / B0_ADAPTER_PATCH
    patch = _patch_bytes(root)
    digest = hashlib.sha256(patch).hexdigest()
    reference = _git(root, "diff", "--binary", "--full-index", B0_BACKEND_COMMIT,
                     B0_ADAPTER_COMMIT, "--", ".")
    parent = _text(root, "rev-parse", f"{B0_ADAPTER_COMMIT}^")
    tree = _text(root, "rev-parse", f"{B0_ADAPTER_COMMIT}^{{tree}}")
    paths = [item.decode("utf-8", errors="strict") for item in _git(
        root, "diff", "--name-only", "-z", B0_BACKEND_COMMIT, B0_ADAPTER_COMMIT,
        "--", ".").split(b"\0") if item]
    rebuilt = _reconstructed_tree(root, patch_path)
    if (parent != B0_BACKEND_COMMIT or tree != B0_ADAPTER_TREE or paths != B0_ADAPTER_PATHS
            or digest != B0_ADAPTER_PATCH_SHA256 or reference != patch or rebuilt != tree):
        raise ValueError("B0 工具适配提交、补丁摘要、路径或重建树不匹配")
    return {
        "contract": "legacy-stable-readiness-b0-v1",
        "base_backend_sha": B0_BACKEND_COMMIT,
        "base_frontend_sha": B0_FRONTEND_COMMIT,
        "reference_adapter_sha": B0_ADAPTER_COMMIT,
        "adapter_tree": B0_ADAPTER_TREE,
        "adapter_paths": B0_ADAPTER_PATHS,
        "patch": {"path": B0_ADAPTER_PATCH.as_posix(), **file_digest(patch_path)},
    }


def _repository(path: Path, label: str) -> Path:
    if not path.is_absolute():
        raise ValueError(f"{label}必须是规范绝对 Git 根目录")
    reject_link_or_reparse(path)
    root = path.resolve(strict=True)
    if root != path or Path(_text(root, "rev-parse", "--show-toplevel")).resolve() != root:
        raise ValueError(f"{label}必须是规范绝对 Git 根目录")
    return root


def _inventory(path: Path, label: str, expected_head: str | None = None) -> tuple[Path, dict]:
    root = _repository(path, label)
    inventory = capture_inventory(root)
    snapshot = inventory["source"]["snapshot"]
    if not snapshot["clean"] or expected_head is not None and snapshot["head"] != expected_head:
        raise ValueError(f"{label}必须是精确干净源码")
    return root, inventory


def _build_document(path: Path, root: Path, label: str):
    document = read_json_document(path)
    if label == "后端":
        expected_root = root / ".local-tests"
        if not document.path.is_relative_to(expected_root):
            raise ValueError("后端构建收据必须位于对应源码的 .local-tests")
    elif document.path != root / "dist" / FRONTEND_RECEIPT:
        raise ValueError("前端构建收据必须是对应 dist 内的标准真实构建收据")
    return document


def _backend_build(root: Path, path: Path, inventory: dict) -> tuple[dict, dict]:
    document = _build_document(path, root, "后端")
    receipt = document.value
    expected_head = inventory["source"]["snapshot"]["head"]
    verify_build(root, receipt, expected_head)
    if receipt["sources"]["full"] != inventory:
        raise ValueError("后端构建收据与声明的精确执行源码不同")
    document.assert_unchanged()
    return receipt, {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def _frontend_build(root: Path, path: Path, inventory: dict) -> tuple[dict, dict]:
    document = _build_document(path, root, "前端")
    receipt = validate_frontend_receipt(document.value)
    environment_files = frontend_environment_files(root)
    files = frontend_files(root)
    if (receipt["sources"]["full"] != inventory
            or receipt["build"]["environment_files"] != environment_files
            or receipt["files"] != files):
        raise ValueError("前端构建收据与精确源码、环境文件或完整 dist 不同")
    if (capture_inventory(root) != inventory
            or frontend_environment_files(root) != environment_files
            or frontend_files(root) != files):
        raise ValueError("核验前端构建收据期间源码、环境文件或完整 dist 发生变化")
    document.assert_unchanged()
    return receipt, {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def _source_export(backend: Path, descriptor: dict) -> dict:
    result_path = bound_file(backend, descriptor)
    result = read_json_document(result_path)
    successor = result.value.get("review_successor")
    if not isinstance(successor, dict):
        raise ValueError("共享 source-export 结果缺少 successor 绑定")
    source = published_source(backend, successor, live_storage=False)
    value = published_export(backend, descriptor, source)
    summary = value["summary"]
    identity = {
        "result": copy.deepcopy(descriptor),
        "origin_attempt": value["origin_attempt"],
        "source_registration": copy.deepcopy(value["source_registration"]),
        "source_rebind": copy.deepcopy(value["source_rebind"]),
        "review_successor": copy.deepcopy(value["review_successor"]),
        "source_request": copy.deepcopy(value["source_request"]),
        "export": copy.deepcopy(value["export"]),
        "export_sha256": summary["export_sha256"],
        "generation_sha256": summary["generation_sha256"],
        "logical_inventory_sha256": summary["logical_inventory_sha256"],
    }
    identity["identity_sha256"] = canonical_digest(identity)
    result.assert_unchanged()
    return identity


def _b0_arm(coordinator: Path, source_backend: Path, execution_backend: Path,
            frontend: Path, backend_build: Path, frontend_build: Path,
            export: dict) -> dict:
    source_root, source_inventory = _inventory(source_backend, "B0 后端产品来源", B0_BACKEND_COMMIT)
    execution_root, execution_inventory = _inventory(
        execution_backend, "B0 后端适配来源", B0_ADAPTER_COMMIT)
    frontend_root, frontend_inventory = _inventory(frontend, "B0 前端来源", B0_FRONTEND_COMMIT)
    backend_receipt, backend_binding = _backend_build(execution_root, backend_build, execution_inventory)
    _frontend_receipt, frontend_binding = _frontend_build(frontend_root, frontend_build, frontend_inventory)
    expected_products = build_source_domains(source_inventory, "backend")["product"]
    if backend_receipt["sources"]["product"] != expected_products:
        raise ValueError("B0 适配构建改变了 API 或 Worker 产品输入")
    return _arm(
        {"source_backend": source_root, "execution_backend": execution_root, "frontend": frontend_root},
        {"backend": source_inventory, "frontend": frontend_inventory},
        {"backend": execution_inventory, "frontend": frontend_inventory},
        {"backend": backend_binding, "frontend": frontend_binding},
        b0_adapter_evidence(coordinator), export,
    )


def _b1_arm(backend: Path, frontend: Path, backend_build: Path,
            frontend_build: Path, export: dict) -> dict:
    backend_root, backend_inventory = _inventory(backend, "B1 后端来源")
    frontend_root, frontend_inventory = _inventory(frontend, "B1 前端来源")
    _backend_receipt, backend_binding = _backend_build(backend_root, backend_build, backend_inventory)
    _frontend_receipt, frontend_binding = _frontend_build(frontend_root, frontend_build, frontend_inventory)
    sources = {"backend": backend_inventory, "frontend": frontend_inventory}
    return _arm(
        {"source_backend": backend_root, "execution_backend": backend_root, "frontend": frontend_root},
        sources, sources, {"backend": backend_binding, "frontend": frontend_binding}, None, export,
    )


def _arm(roots: dict, sources: dict, execution_sources: dict, builds: dict,
         adapter: dict | None, export: dict) -> dict:
    return {
        "roots": {name: str(path) for name, path in roots.items()},
        "sources": copy.deepcopy(sources),
        "execution_sources": copy.deepcopy(execution_sources),
        "builds": copy.deepcopy(builds),
        "adapter": copy.deepcopy(adapter),
        "source_export_result_sha256": export["result"]["sha256"],
        "source_export_identity_sha256": export["identity_sha256"],
    }


def capture_comparison_sources(
    backend: Path, *, b0_backend: Path, b0_adapter_backend: Path, b0_frontend: Path,
    b0_backend_build: Path, b0_frontend_build: Path, b1_backend: Path, b1_frontend: Path,
    b1_backend_build: Path, b1_frontend_build: Path, source_export_result: dict,
) -> dict:
    """只读生成双版本来源清单；不创建恢复 run、锁或业务数据。"""
    coordinator = _repository(backend, "来源协调后端")
    export = _source_export(coordinator, source_export_result)
    b0 = _b0_arm(coordinator, b0_backend, b0_adapter_backend, b0_frontend,
                 b0_backend_build, b0_frontend_build, export)
    b1 = _b1_arm(b1_backend, b1_frontend, b1_backend_build, b1_frontend_build, export)
    roots = [Path(path) for arm in (b0, b1) for path in arm["roots"].values()]
    if len(set(roots)) != 5:
        raise ValueError("B0/B1 产品、适配和前端必须使用五个独立工作树")
    if (b1["sources"]["backend"]["source"]["snapshot"]["head"] == B0_BACKEND_COMMIT
            or b1["sources"]["frontend"]["source"]["snapshot"]["head"] == B0_FRONTEND_COMMIT):
        raise ValueError("B1 必须分别绑定区别于 B0 的最终双端源码")
    result = {"format_version": 1, "kind": "restore-comparison-sources",
              "source_export": export, "arms": {"b0": b0, "b1": b1}}
    repeated = {
        "export": _source_export(coordinator, source_export_result),
        "b0": _b0_arm(coordinator, b0_backend, b0_adapter_backend, b0_frontend,
                       b0_backend_build, b0_frontend_build, export),
        "b1": _b1_arm(b1_backend, b1_frontend, b1_backend_build, b1_frontend_build, export),
    }
    if repeated != {"export": export, "b0": b0, "b1": b1}:
        raise ValueError("采集双版本来源期间源码、构建或共享 source-export 发生变化")
    return result


def _binding_shape(value: object, label: str) -> dict:
    exact(value, {"path", "bytes", "sha256"})
    if (not isinstance(value["path"], str) or not Path(value["path"]).is_absolute()
            or type(value["bytes"]) is not int or value["bytes"] <= 0
            or not isinstance(value["sha256"], str)
            or re.fullmatch(r"[a-f0-9]{64}", value["sha256"]) is None):
        raise ValueError(f"{label}文件绑定无效")
    return value


def _validate_export_shape(value: dict) -> None:
    exact(value, EXPORT_FIELDS)
    for field in ("result", "source_registration", "source_rebind", "review_successor",
                  "source_request", "export"):
        _binding_shape(value[field], f"source-export {field}")
    if (type(value["origin_attempt"]) is not int or value["origin_attempt"] <= 0
            or any(not isinstance(value[field], str)
                   or re.fullmatch(r"[a-f0-9]{64}", value[field]) is None
                   for field in ("export_sha256", "generation_sha256",
                                 "logical_inventory_sha256", "identity_sha256"))):
        raise ValueError("source-export 内部身份无效")
    expected = canonical_digest({key: item for key, item in value.items() if key != "identity_sha256"})
    if value["identity_sha256"] != expected:
        raise ValueError("source-export 内部身份摘要无效")


def _validate_arm_shape(name: str, arm: dict, export: dict) -> None:
    exact(arm, ARM_FIELDS)
    exact(arm["roots"], {"source_backend", "execution_backend", "frontend"})
    exact(arm["sources"], {"backend", "frontend"})
    exact(arm["execution_sources"], {"backend", "frontend"})
    exact(arm["builds"], {"backend", "frontend"})
    for field in arm["roots"].values():
        if not isinstance(field, str) or not Path(field).is_absolute():
            raise ValueError(f"{name} 源码根目录无效")
    for inventory in (*arm["sources"].values(), *arm["execution_sources"].values()):
        validate_inventory(inventory)
    for role, binding in arm["builds"].items():
        _binding_shape(binding, f"{name} {role} 构建")
    if (arm["source_export_result_sha256"] != export["result"]["sha256"]
            or arm["source_export_identity_sha256"] != export["identity_sha256"]):
        raise ValueError("B0/B1 没有共同绑定同一 source-export")
    if name == "b0":
        if not isinstance(arm["adapter"], dict):
            raise ValueError("B0 缺少工具适配来源")
        exact(arm["adapter"], {"contract", "base_backend_sha", "base_frontend_sha",
                               "reference_adapter_sha", "adapter_tree", "adapter_paths", "patch"})
        exact(arm["adapter"]["patch"], {"path", "bytes", "sha256"})
        expected = {
            "contract": "legacy-stable-readiness-b0-v1",
            "base_backend_sha": B0_BACKEND_COMMIT,
            "base_frontend_sha": B0_FRONTEND_COMMIT,
            "reference_adapter_sha": B0_ADAPTER_COMMIT,
            "adapter_tree": B0_ADAPTER_TREE,
            "adapter_paths": B0_ADAPTER_PATHS,
            "patch_path": B0_ADAPTER_PATCH.as_posix(),
            "patch_sha256": B0_ADAPTER_PATCH_SHA256,
        }
        observed = {key: arm["adapter"].get(key) for key in expected if not key.startswith("patch_")}
        observed.update(patch_path=arm["adapter"]["patch"].get("path"),
                        patch_sha256=arm["adapter"]["patch"].get("sha256"))
        if observed != expected:
            raise ValueError("B0 工具适配身份无效")
    elif arm["adapter"] is not None or arm["sources"] != arm["execution_sources"]:
        raise ValueError("B1 不得携带 B0 工具适配或不同执行来源")


def verify_comparison_sources(backend: Path, value: dict) -> dict:
    """按清单声明的路径重建同一结果；strict v1 不接受旧字段或双读。"""
    exact(value, MANIFEST_FIELDS)
    if value["format_version"] != 1 or value["kind"] != "restore-comparison-sources":
        raise ValueError("双版本来源清单版本或类型无效")
    exact(value["arms"], {"b0", "b1"})
    _validate_export_shape(value["source_export"])
    for name, arm in value["arms"].items():
        _validate_arm_shape(name, arm, value["source_export"])
    b0, b1 = value["arms"]["b0"], value["arms"]["b1"]
    expected = capture_comparison_sources(
        backend,
        b0_backend=Path(b0["roots"]["source_backend"]),
        b0_adapter_backend=Path(b0["roots"]["execution_backend"]),
        b0_frontend=Path(b0["roots"]["frontend"]),
        b0_backend_build=Path(b0["builds"]["backend"]["path"]),
        b0_frontend_build=Path(b0["builds"]["frontend"]["path"]),
        b1_backend=Path(b1["roots"]["source_backend"]),
        b1_frontend=Path(b1["roots"]["frontend"]),
        b1_backend_build=Path(b1["builds"]["backend"]["path"]),
        b1_frontend_build=Path(b1["builds"]["frontend"]["path"]),
        source_export_result=value["source_export"]["result"],
    )
    if expected != value:
        raise ValueError("双版本源码、构建或共享导出与来源清单不一致")
    return value
