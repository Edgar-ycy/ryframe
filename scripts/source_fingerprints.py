"""分别绑定产品构建输入、验收工具和完整来源；显式审计旧产物的复用。"""
from __future__ import annotations

from contextlib import contextmanager
from contextvars import ContextVar
import copy
import json
from pathlib import Path
import re

from source_inventory import (
    capture_inventory,
    canonical_digest,
    file_inventory,
    fingerprints,
)

_BRIDGES = ContextVar("artifact_source_bridges", default=None)
_CHECK = ContextVar("artifact_source_check", default=None)


def execution_source(inventory: dict) -> dict:
    return {**inventory["source"], "fingerprints": fingerprints(inventory)}


def current_execution_source(root: Path) -> dict:
    return execution_source(capture_inventory(root))


def build_source(receipt: dict) -> dict:
    if receipt.get("kind") == "devex-clone-tool-build":
        return receipt["source"]
    return {"snapshot": receipt["source"]}


def verify_inventory_source(inventory: dict, receipt: dict) -> None:
    source = inventory["source"]
    expected = build_source(receipt)
    if (source["snapshot"] != expected["snapshot"]
            or "worktree_fingerprint" in expected and source["worktree_fingerprint"] != expected["worktree_fingerprint"]
            or not re.fullmatch(r"sha256:[a-f0-9]{64}", source["worktree_fingerprint"])):
        raise ValueError("原始文件清单没有绑定构建时的完整源码")
    fingerprints(inventory)


def file_binding(path: Path) -> dict:
    from restore_build import file_digest
    if not path.is_absolute():
        raise ValueError("来源审计只接受明确绝对路径")
    return {"path": str(path), "sha256": file_digest(path)["sha256"]}


def read_binding(root: Path, binding: dict) -> dict:
    from devex_clone_model import local_path
    if set(binding) != {"path", "sha256"}:
        raise ValueError("来源审计文件缺少明确路径和摘要")
    path = local_path(root, binding["path"])
    if file_binding(path) != binding or path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError("来源审计文件摘要变化或超过限制")
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError("来源审计文件必须是对象")
    return value


def verify_execution_source(value: dict, label: str) -> None:
    if (not isinstance(value, dict)
            or set(value) != {"snapshot", "worktree_fingerprint", "fingerprints"}):
        raise ValueError(f"{label}完整来源格式无效")
    snapshot = value["snapshot"]
    if (not isinstance(snapshot, dict)
            or set(snapshot) != {"head", "patch_sha256", "files", "clean"}
            or not isinstance(snapshot["head"], str)
            or re.fullmatch(r"[a-f0-9]{40}", snapshot["head"]) is None
            or not isinstance(snapshot["files"], list) or type(snapshot["clean"]) is not bool
            or not isinstance(value["worktree_fingerprint"], str)
            or re.fullmatch(r"sha256:[a-f0-9]{64}", value["worktree_fingerprint"]) is None):
        raise ValueError(f"{label}完整来源格式无效")
    if not isinstance(snapshot["patch_sha256"], str):
        raise ValueError(f"{label}完整来源格式无效")
    digest = snapshot["patch_sha256"]
    if re.fullmatch(r"[a-f0-9]{64}", digest) is None:
        raise ValueError(f"{label}完整来源格式无效")
    groups = value["fingerprints"]
    if not isinstance(groups, dict) or set(groups) != {"product", "test_tools", "support"}:
        raise ValueError(f"{label}分域指纹格式无效")
    for group in groups.values():
        if (not isinstance(group, dict) or set(group) != {"sha256", "files"}
                or not isinstance(group["sha256"], str)
                or re.fullmatch(r"[a-f0-9]{64}", group["sha256"]) is None
                or type(group["files"]) is not int or group["files"] < 0):
            raise ValueError(f"{label}分域指纹格式无效")


def write_bridge(root: Path, build_path: Path, inventory_path: Path, output_path: Path) -> dict:
    """仅登记明确核验过的历史构建清单；不改旧收据、不构建、不取得恢复资格。"""
    from devex_clone_model import local_path

    build_binding, inventory_binding = file_binding(build_path), file_binding(inventory_path)
    receipt, inventory = read_binding(root, build_binding), read_binding(root, inventory_binding)
    if receipt.get("kind") not in {"restore-backend-build", "devex-clone-tool-build"}:
        raise ValueError("来源桥接只接受 API/Worker 或维护 CLI 构建收据")
    _verify_bridge_build(root, build_path, receipt)
    verify_inventory_source(inventory, receipt)
    current_inventory = capture_inventory(root)
    current = execution_source(current_inventory)
    original = fingerprints(inventory)
    if original["product"] != current["fingerprints"]["product"]:
        raise ValueError("产品构建输入已经变化，必须重新编译")
    value = {"format_version": 1, "kind": "devex-artifact-source-bridge", "backend_root": str(root),
             "build": build_binding, "inventory": inventory_binding, "original_fingerprints": original,
             "audited_source": current, "restore_qualified": False, "compiled": False}
    output = local_path(root, str(output_path), new=True)
    if current_execution_source(root) != current:
        raise ValueError("桥接核验期间源码发生变化")
    with output.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    return value


def _bridge_build_identity(receipt: dict) -> str:
    artifacts = receipt.get("artifacts")
    if not isinstance(artifacts, dict):
        raise ValueError("来源桥接构建缺少完整产物")
    roles = {}
    for role, artifact in sorted(artifacts.items()):
        if not isinstance(role, str) or not isinstance(artifact, dict):
            raise ValueError("来源桥接构建产物格式无效")
        roles[role] = {key: artifact.get(key) for key in ("command", "bytes", "sha256")}
    return canonical_digest({"kind": receipt.get("kind"), "source": build_source(receipt),
                             "artifacts": roles})


def _maintenance_build_root(root: Path, path: Path, receipt: dict) -> Path:
    """解析维护构建所属工作树，隔离工作树只能位于当前证据根内。"""
    from devex_clone_model import linked

    current = root.resolve(strict=True)
    declared = Path(receipt.get("backend_root", ""))
    if not declared.is_absolute() or not declared.is_dir():
        raise ValueError("维护构建缺少可用的所属工作树")
    declared = declared.resolve(strict=True)
    if declared == current:
        return current
    evidence_root = current / ".local-tests"
    if declared == evidence_root or not declared.is_relative_to(evidence_root):
        raise ValueError("隔离维护构建工作树必须位于当前 .local-tests")
    cursor = evidence_root
    for part in declared.relative_to(evidence_root).parts:
        cursor /= part
        if linked(cursor):
            raise ValueError("隔离维护构建工作树不能经过链接")
    if not path.is_relative_to(declared / ".local-tests"):
        raise ValueError("维护构建收据必须位于所属工作树的 .local-tests")
    return declared


def _verify_bridge_build(root: Path, path: Path, receipt: dict) -> None:
    kind = receipt.get("kind")
    if kind == "restore-backend-build":
        from restore_build import verify_build_artifacts

        fields = {"format_version", "kind", "source", "artifacts"}
        if "source_inventory" in receipt:
            fields.add("source_inventory")
        if set(receipt) != fields or receipt.get("format_version") != 1:
            raise ValueError("API/Worker 构建收据字段无效")
        core = {"executable", "command", "bytes", "sha256"}
        for artifact in receipt.get("artifacts", {}).values():
            artifact_fields = frozenset(artifact) if isinstance(artifact, dict) else frozenset()
            if artifact_fields not in {frozenset(core), frozenset(core | {"cargo_executable"})}:
                raise ValueError("API/Worker 构建产物字段无效")
        verify_build_artifacts(receipt)
        return
    if kind == "devex-clone-tool-build":
        from devex_clone_tools import verify_evidence

        verify_evidence(_maintenance_build_root(root, path, receipt), path, receipt)
        return
    raise ValueError("来源桥接只接受 API/Worker 或维护 CLI 构建收据")


def _bridge_registration(root: Path, binding: dict, current: dict, *, inherited: bool) -> tuple[str, str, dict]:
    bridge = read_binding(root, binding)
    if (set(bridge) != {"format_version", "kind", "backend_root", "build", "inventory",
                       "original_fingerprints", "audited_source", "restore_qualified", "compiled"}
            or bridge["format_version"] != 1 or bridge["kind"] != "devex-artifact-source-bridge"
            or bridge["backend_root"] != str(root) or bridge["restore_qualified"] is not False
            or bridge["compiled"] is not False):
        raise ValueError("产物来源桥接格式或用途不符")
    receipt = read_binding(root, bridge["build"])
    inventory = read_binding(root, bridge["inventory"])
    _verify_bridge_build(root, Path(bridge["build"]["path"]), receipt)
    verify_inventory_source(inventory, receipt)
    if receipt.get("source_inventory") is not None and receipt["source_inventory"] != inventory:
        raise ValueError("构建收据与来源桥接绑定了不同的构建时 inventory")
    original = fingerprints(inventory)
    verify_execution_source(bridge["audited_source"], "来源桥接 audited_source")
    if (original != bridge["original_fingerprints"]
            or bridge["audited_source"]["fingerprints"]["product"] != original["product"]
            or not inherited and original["product"] != current["fingerprints"]["product"]):
        raise ValueError("产物来源桥接的产品指纹不匹配")
    return (canonical_digest(receipt), _bridge_build_identity(receipt),
            {"inventory": inventory, "inherited": inherited})


@contextmanager
def artifact_sources(root: Path, bridge_bindings: list[dict],
                     inherited_bridge_bindings: list[dict] | None = None):
    """统一验收入口必须显式声明桥接文件；不通过环境变量或隐式旁路授权复用。"""
    if _BRIDGES.get() is not None:
        raise ValueError("产物来源上下文不能嵌套")
    registered, identities = {}, set()
    current_inventory = capture_inventory(root)
    current = execution_source(current_inventory)
    entries = [(copy.deepcopy(binding), False) for binding in bridge_bindings]
    entries.extend((copy.deepcopy(binding), True)
                   for binding in (inherited_bridge_bindings or []))
    checks = []
    for binding, inherited in entries:
        key, identity, registration = _bridge_registration(root, binding, current,
                                                            inherited=inherited)
        if key in registered or identity in identities:
            raise ValueError("同一构建不能重复登记来源桥接")
        registered[key] = registration
        identities.add(identity)
        checks.append((key, identity, registration))
    token = _BRIDGES.set((root.resolve(), registered, current, current_inventory))
    try:
        yield copy.deepcopy(current)
    finally:
        try:
            for (binding, inherited), expected in zip(entries, checks, strict=True):
                if _bridge_registration(root, binding, current, inherited=inherited) != expected:
                    raise ValueError("阶段结束时构建桥接或冻结产物发生变化")
            if capture_inventory(root) != current_inventory:
                raise ValueError("阶段结束时完整源码变化，不能发布成功结果")
        finally:
            _BRIDGES.reset(token)


def checked_source(root: Path) -> dict | None:
    active = _CHECK.get()
    if active is None:
        return None
    if root.resolve() != active[0]:
        raise ValueError("源码检查窗口不能跨仓库复用")
    return copy.deepcopy(active[1])


@contextmanager
def source_check(root: Path):
    """一个明确 generation 窗口只核验一次源码；不缓存业务或运行身份观察。"""
    active, stage = _CHECK.get(), _BRIDGES.get()
    if active is not None:
        yield checked_source(root)
        return
    if stage is None:
        yield None  # 原有独立入口继续执行自身完整检查。
        return
    if root.resolve() != stage[0]:
        raise ValueError("源码检查窗口必须属于当前验收仓库")
    observed = file_inventory(root)
    expected = {"files": stage[3]["files"], **stage[3]["guard"]}
    if observed != expected:
        raise ValueError("执行期间源码集合、内容或 Git 模式变化，需重新核验受影响阶段")
    token = _CHECK.set((stage[0], stage[2]))
    try:
        yield copy.deepcopy(stage[2])
    finally:
        _CHECK.reset(token)


def reusable_artifact_source(root: Path, receipt: dict) -> dict | None:
    """返回构建时的原始来源；执行时的工具来源由调用方单独记录。"""
    inventory, inherited = receipt.get("source_inventory"), False
    scope = _BRIDGES.get()
    if scope is not None:
        if root.resolve() != scope[0]:
            return None
        registration = scope[1].get(canonical_digest(receipt))
        if registration is not None:
            inventory = registration["inventory"]
            inherited = registration["inherited"]
    if inventory is None:
        return None
    verify_inventory_source(inventory, receipt)
    current = checked_source(root) or current_execution_source(root)
    if scope is not None and current != scope[2]:
        raise ValueError("执行期间完整源码变化，需重新核验受影响阶段")
    if (not inherited
            and fingerprints(inventory)["product"] != current["fingerprints"]["product"]):
        raise ValueError("产品构建输入已经变化，必须重新编译")
    return copy.deepcopy(inventory["source"])
