"""分别绑定产品构建输入、验收工具和完整来源；显式审计旧产物的复用。"""
from __future__ import annotations

from contextlib import contextmanager
from contextvars import ContextVar
import copy
import json
from pathlib import Path
import re

from source_inventory import (
    build_source_domains,
    capture_inventory,
    canonical_digest,
    file_inventory,
    fingerprints,
    validate_build_source_domains,
    validate_inventory,
)

_BRIDGES = ContextVar("artifact_source_bridges", default=None)
_CHECK = ContextVar("artifact_source_check", default=None)


def execution_source(inventory: dict) -> dict:
    return {**inventory["source"], "fingerprints": fingerprints(inventory)}


def current_execution_source(root: Path) -> dict:
    return execution_source(capture_inventory(root))


def require_current_execution_source(root: Path, expected: dict) -> dict:
    """精确绑定已登记干净来源及全部分域，包含间接和动态加载的验收工具。"""
    verify_execution_source(expected, "登记的执行工具")
    current = current_execution_source(root)
    if not expected["snapshot"]["clean"] or current != expected:
        raise ValueError("协调器必须保持 START 登记的干净 SHA 和完整 test_tools 来源")
    return current


def build_source(receipt: dict) -> dict:
    if receipt.get("kind") == "devex-clone-tool-build":
        return receipt["source"]
    if receipt.get("kind") != "restore-backend-build":
        raise ValueError("构建来源类型无效")
    expected = {"format_version", "kind", "sources", "build", "artifacts"}
    if receipt.get("format_version") != 2 or set(receipt) != expected:
        raise ValueError("当前构建来源必须使用后端严格 v2 收据")
    return validate_build_source_domains(receipt.get("sources"), "backend")["full"]["source"]


def verify_inventory_source(inventory: dict, receipt: dict) -> None:
    source = inventory["source"]
    expected = build_source(receipt)
    if (source["snapshot"] != expected["snapshot"]
            or "worktree_fingerprint" in expected and source["worktree_fingerprint"] != expected["worktree_fingerprint"]
            or not re.fullmatch(r"sha256:[a-f0-9]{64}", source["worktree_fingerprint"])):
        raise ValueError("原始文件清单没有绑定构建时的完整源码")
    fingerprints(inventory)


def bridge_product_inputs(receipt: dict, inventory: dict) -> dict:
    """按实际产物角色计算复用边界；维护构建继续使用自身的产品集合。"""
    validate_inventory(inventory)
    if receipt.get("kind") == "restore-backend-build":
        return build_source_domains(inventory, "backend")["product"]
    return fingerprints(inventory)["product"]


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
        from restore_build import validate_build_context, verify_build_artifacts

        fields = {"format_version", "kind", "sources", "build", "artifacts"}
        if set(receipt) != fields or receipt.get("format_version") != 2:
            raise ValueError("API/Worker 构建收据字段无效")
        validate_build_source_domains(receipt["sources"], "backend")
        validate_build_context(receipt["build"])
        core = {"executable", "command", "bytes", "sha256"}
        for artifact in receipt.get("artifacts", {}).values():
            artifact_fields = frozenset(artifact) if isinstance(artifact, dict) else frozenset()
            if artifact_fields != frozenset(core):
                raise ValueError("API/Worker 构建产物字段无效")
        verify_build_artifacts(receipt)
        return
    if kind == "devex-clone-tool-build":
        from devex_clone_tools import verify_evidence

        verify_evidence(_maintenance_build_root(root, path, receipt), path, receipt)
        return
    raise ValueError("来源桥接只接受 API/Worker 或维护 CLI 构建收据")


def _bridge_registration(root: Path, binding: dict, current: dict, current_inventory: dict,
                         *, inherited: bool) -> tuple[str, str, dict]:
    bridge = read_binding(root, binding)
    if (set(bridge) != {"format_version", "kind", "backend_root", "build", "inventory",
                       "original_fingerprints", "audited_source", "audited_inventory", "product_inputs",
                       "restore_qualified", "compiled"}
            or bridge["format_version"] != 1 or bridge["kind"] != "devex-artifact-source-bridge"
            or bridge["restore_qualified"] is not False
            or bridge["compiled"] is not False):
        raise ValueError("产物来源桥接格式或用途不符")
    nested = bridge["backend_root"] != str(root)
    if nested:
        # 夹具等嵌套产品树自带根；必须位于运行根内且不经过链接，并按该根完整重查。
        from devex_clone_model import linked

        bridge_root = Path(bridge["backend_root"])
        if (not bridge_root.is_absolute() or any(linked(item) for item in (bridge_root, *bridge_root.parents))
                or not bridge_root.resolve().is_relative_to(root.resolve())):
            raise ValueError("嵌套产物来源桥接的根不属于当前运行目录")
        current, current_inventory = current_execution_source(bridge_root), capture_inventory(bridge_root)
    receipt = read_binding(root, bridge["build"])
    inventory = read_binding(root, bridge["inventory"])
    _verify_bridge_build(root, Path(bridge["build"]["path"]), receipt)
    verify_inventory_source(inventory, receipt)
    if (receipt.get("kind") == "restore-backend-build"
            and receipt["sources"]["full"] != inventory):
        raise ValueError("构建收据与来源桥接绑定了不同的构建时 inventory")
    original = fingerprints(inventory)
    verify_execution_source(bridge["audited_source"], "来源桥接 audited_source")
    audited_inventory = validate_inventory(bridge["audited_inventory"])
    expected_products = {"build": bridge_product_inputs(receipt, inventory),
                         "audited": bridge_product_inputs(receipt, audited_inventory)}
    if (original != bridge["original_fingerprints"]
            or execution_source(audited_inventory) != bridge["audited_source"]
            or bridge["product_inputs"] != expected_products
            or expected_products["build"] != expected_products["audited"]
            or not inherited and (audited_inventory != current_inventory
                                  or bridge["audited_source"] != current)):
        raise ValueError("产物来源桥接的产品指纹不匹配")
    return (canonical_digest(receipt), _bridge_build_identity(receipt),
            {"inventory": inventory, "inherited": inherited,
             "root": str(bridge_root if nested else root)})


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
        key, identity, registration = _bridge_registration(root, binding, current, current_inventory,
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
                if _bridge_registration(root, binding, current, current_inventory,
                                        inherited=inherited) != expected:
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
    if receipt.get("kind") == "restore-backend-build":
        expected_fields = {"format_version", "kind", "sources", "build", "artifacts"}
        if receipt.get("format_version") != 2 or set(receipt) != expected_fields:
            raise ValueError("当前构建收据字段无效")
        sources = validate_build_source_domains(receipt.get("sources"), "backend")
        scope = _BRIDGES.get()
        if scope is not None and root.resolve() != scope[0]:
            return None
        registration = scope[1].get(canonical_digest(receipt)) if scope is not None else None
        # 嵌套桥接（夹具等自带产品树）在登记阶段已按桥接根核验当前来源；此处不得再用运行根
        # 复核产品输入或构建上下文，否则与桥接审计结论冲突。
        nested = (registration is not None
                  and Path(registration.get("root", "")).resolve() != root.resolve())
        current_inventory = (scope[3] if scope is not None and root.resolve() == scope[0]
                             else capture_inventory(root))
        if not nested and (not registration or not registration["inherited"]) and (
                sources["product"] != build_source_domains(current_inventory, "backend")["product"]):
            raise ValueError("产品构建输入已经变化，必须重新编译")
        from restore_build import build_context

        if not nested and receipt.get("build") != build_context(root):
            raise ValueError("构建命令、工具链或有效环境已经变化，必须重新编译")
        return copy.deepcopy(sources["full"]["source"])
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
