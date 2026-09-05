"""只读绑定开发性能样本的源码、构建、进程和实际站点；不授予正式恢复资格。"""

from __future__ import annotations

import hashlib
import json
import re
import sys
from pathlib import Path

from full_stack_process import process_identity, read_process
from full_stack_runtime import verify_runtime
from prepare_full_stack_fixture import git
from process_sockets import verify_listener
from restore_build import source_snapshot, verify_build_artifacts
from restore_runtime import FRONTEND_RECEIPT, verify_frontend_artifacts
from source_fingerprints import current_execution_source, reusable_artifact_source


class ProvenanceError(ValueError):
    def __init__(self, stage: str):
        super().__init__(f"运行来源核验失败：{stage}")
        self.stage = stage


def checked(stage: str, operation):
    try:
        return operation()
    except Exception:
        # 原始异常可能带配置路径、连接信息或响应内容，报告只保留确定阶段。
        raise ProvenanceError(stage) from None


def absolute_path(value: str) -> Path:
    path = Path(value)
    if not path.is_absolute() or path.is_symlink():
        raise ValueError("来源绑定必须为显式绝对普通路径")
    return path.resolve(strict=True)


def request_roots(request: dict) -> tuple[Path, Path, Path]:
    expected_fields = {
        "backend", "frontend", "driver", "driver_fingerprint", "scope_id",
        "source_fingerprints", "api_url", "frontend_url", "metrics_urls",
        "environment_sha256", "provenance",
    }
    source_fingerprints = request.get("source_fingerprints") if isinstance(request, dict) else None
    if (
        not isinstance(request, dict)
        or set(request) != expected_fields
        or not isinstance(request.get("driver_fingerprint"), str)
        or re.fullmatch(r"sha256:[a-f0-9]{64}", request["driver_fingerprint"]) is None
        or not isinstance(source_fingerprints, dict)
        or set(source_fingerprints) != {"backend", "frontend"}
        or any(
            not isinstance(value, str)
            or re.fullmatch(r"sha256:[a-f0-9]{64}", value) is None
            for value in source_fingerprints.values()
        )
    ):
        raise ValueError("运行来源请求字段不完整")
    return tuple(absolute_path(request[name]) for name in ("backend", "frontend", "driver"))


def bound_file(binding: dict, *, document: bool = False):
    if set(binding) != {"path", "sha256"} or not re.fullmatch(r"[a-f0-9]{64}", binding["sha256"]):
        raise ValueError("文件绑定缺少精确 SHA")
    path = absolute_path(binding["path"])
    if not path.is_file() or not 0 < path.stat().st_size <= 16 * 1024 * 1024:
        raise ValueError("来源文件必须是 16 MiB 内的非空普通文件")
    content = path.read_bytes()
    if hashlib.sha256(content).hexdigest() != binding["sha256"]:
        raise ValueError("来源文件摘要已变化")
    if document:
        if not content.decode("utf-8").strip():
            raise ValueError("环境说明不能为空白文件")
        return binding["sha256"]
    result = json.loads(content)
    if not isinstance(result, dict):
        raise ValueError("来源收据必须是对象")
    return result


def worktree_fingerprint(root: Path, commit: str) -> str:
    # 与 xtask metadata::worktree_fingerprint 使用完全相同的 Git 参数及长度前缀。
    digest = hashlib.sha256()

    def update(value: bytes) -> None:
        digest.update(len(value).to_bytes(8, "little"))
        digest.update(value)

    update(commit.encode())
    update(git(root, "diff", "--binary", "--no-ext-diff", "HEAD", "--", "."))
    for raw in git(root, "ls-files", "--others", "--exclude-standard", "-z").split(b"\0"):
        if not raw:
            continue
        relative = Path(raw.decode("utf-8"))
        path = root / relative
        if (relative.is_absolute() or ".." in relative.parts or path.is_symlink()
                or not path.resolve(strict=True).is_relative_to(root.resolve())):
            raise ValueError("源码包含越界的未跟踪路径")
        update(raw)
        update(path.read_bytes())
    return "sha256:" + digest.hexdigest()


def verify_source(root: Path, receipt: dict, fingerprint: str) -> dict:
    original = reusable_artifact_source(root, receipt)
    if original is not None:
        if original["worktree_fingerprint"] != fingerprint:
            raise ValueError("请求没有绑定构建时的完整工作区指纹")
        return original["snapshot"]
    source = source_snapshot(root)
    actual = worktree_fingerprint(root, source["head"])
    if (receipt.get("source") != source or type(source.get("clean")) is not bool
            or not re.fullmatch(r"[a-f0-9]{40}", source["head"])
            or actual != fingerprint):
        raise ValueError("实际源码与本侧精确构建快照或 DevEx 指纹不一致")
    return source


def file_bindings(request: dict, frontend: Path) -> dict:
    binding = request["provenance"]
    if set(binding) != {"backend_build", "runtime", "processes", "frontend_build_sha256", "environment_document"}:
        raise ValueError("运行来源绑定字段不完整")
    runtime = absolute_path(binding["runtime"]["path"])
    if runtime.name != "runtime.json" or set(binding["processes"]) != {"api", "worker"}:
        raise ValueError("必须绑定现有全栈运行和成对进程收据")
    return {
        "backend": binding["backend_build"], "runtime": binding["runtime"],
        "frontend": {"path": str(frontend / "dist" / FRONTEND_RECEIPT), "sha256": binding["frontend_build_sha256"]},
        **{role: {"path": str(runtime.parent / f"{role}.json"), "sha256": binding["processes"][role]}
           for role in ("api", "worker")},
    }


def verify_processes(request: dict, receipts: dict, directory: Path) -> dict:
    identities = {}
    for role in ("api", "worker"):
        identity = read_process(directory, role, request["scope_id"])
        artifact = receipts["backend"]["artifacts"][role]
        if (identity != receipts[role].get("identity") or process_identity(identity["pid"]) != identity
                or Path(identity["executable"]).resolve() != Path(artifact["executable"]).resolve()):
            raise ValueError("运行进程已退出、重启或使用其他二进制")
        urls = [request["metrics_urls"][role], request["api_url"] if role == "api"
                else receipts["runtime"]["worker_ready_url"]]
        for url in urls:
            verify_listener(identity["pid"], url)
        identities[role] = identity
    return identities


def verify_runtime_binding(request: dict, receipts: dict, backend: Path, directory: Path) -> None:
    runtime = receipts["runtime"]
    if (verify_runtime(backend, directory) != runtime or runtime["scope_id"] != request["scope_id"]
            or Path(runtime["backend_root"]).resolve() != backend or set(runtime["artifacts"]) != {"api", "worker"}):
        raise ValueError("运行配置、隔离范围或源码目录与收据不一致")
    for role, artifact in receipts["backend"]["artifacts"].items():
        recorded = runtime["artifacts"][role]
        if (Path(recorded["path"]).resolve() != Path(artifact["executable"]).resolve()
                or recorded["sha256"] != artifact["sha256"]):
            raise ValueError("运行收据没有使用本次构建产物")


def verify(request: dict) -> dict:
    backend, frontend, driver = checked("bindings", lambda: request_roots(request))
    files = checked("bindings", lambda: file_bindings(request, frontend))
    environment = {"path": request["provenance"]["environment_document"], "sha256": request["environment_sha256"]}
    checked("environment_document", lambda: bound_file(environment, document=True))
    receipts = {name: checked(f"{name}_receipt", lambda binding=binding: bound_file(binding))
                for name, binding in files.items()}
    directory = Path(files["runtime"]["path"]).resolve().parent
    sources = {}
    for role, root in (("backend", backend), ("frontend", frontend)):
        sources[role] = checked(f"{role}_source", lambda: verify_source(
            root, receipts[role], request["source_fingerprints"][role]))
    driver_source = checked("driver_source", lambda: current_execution_source(driver))
    if driver_source["worktree_fingerprint"] != request["driver_fingerprint"]:
        raise ProvenanceError("driver_source")
    sources["driver"] = driver_source["snapshot"]
    checked("backend_artifacts", lambda: verify_build_artifacts(receipts["backend"]))
    checked("runtime_configuration", lambda: verify_runtime_binding(request, receipts, backend, directory))
    processes = checked("processes", lambda: verify_processes(request, receipts, directory))
    checked("frontend_artifacts", lambda: verify_frontend_artifacts(frontend, receipts["frontend"], request["frontend_url"]))
    # 每次核验也检查自身窗口，避免读取源码、产物和站点期间拼接不同代次。
    for name, binding in files.items():
        checked(f"{name}_receipt_stable", lambda binding=binding: bound_file(binding))
    checked("environment_document_stable", lambda: bound_file(environment, document=True))
    for role, root in (("backend", backend), ("frontend", frontend)):
        checked(f"{role}_source_stable", lambda: verify_source(root, receipts[role], request["source_fingerprints"][role]))
    if checked("driver_source_stable", lambda: current_execution_source(driver)) != driver_source:
        raise ProvenanceError("driver_source_stable")
    checked("backend_artifacts_stable", lambda: verify_build_artifacts(receipts["backend"]))
    checked("runtime_configuration_stable", lambda: verify_runtime_binding(request, receipts, backend, directory))
    checked("processes_stable", lambda: verify_processes(request, receipts, directory))
    return {"format_version": 1, "kind": "devex-runtime-provenance", "scope_id": request["scope_id"],
            "backend_root": str(backend), "frontend_root": str(frontend), "sources": sources,
            "source_fingerprints": request["source_fingerprints"], "receipts": files, "processes": processes,
            "execution_source": driver_source,
            "environment_document": environment, "environment_evidence": "operator_declared_document",
            "api_url": request["api_url"], "frontend_url": request["frontend_url"], "metrics_urls": request["metrics_urls"]}


def main() -> None:
    try:
        content = sys.stdin.buffer.read(1024 * 1024 + 1)
        if len(content) > 1024 * 1024:
            raise ProvenanceError("bindings")
        request = checked("bindings", lambda: json.loads(content))
        result = {"ok": True, "receipt": verify(request)}
    except ProvenanceError as error:
        result = {"ok": False, "stage": error.stage}
    except Exception:
        result = {"ok": False, "stage": "bindings"}
    print(json.dumps(result, ensure_ascii=False))
    if not result["ok"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
