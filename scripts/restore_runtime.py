"""显式构建、绑定与只读核验恢复验收的真实运行产物；不执行备份或恢复。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import quote

from full_stack_process import process_identity
from process_sockets import verify_listener
from restore_build import (
    build_registered,
    validate_new_output,
    verify_build,
    write_new,
)
from restore_identifiers import valid_identifier, valid_scope_identifier
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    ArtifactSnapshot,
    JsonDocument,
    artifact_snapshot,
    canonical_endpoint as _canonical_endpoint,
    digest_matches as _digest_matches,
    directory as _directory,
    exact_fields as _exact_fields,
    is_reparse as _is_reparse,
    process_document as _process_document,
    read_authority,
    read_json as read_json,
    read_json_document,
    reject_json_constant as _reject_json_constant,
    same_path as _same_path,
    strict_object_pairs as _strict_object_pairs,
    timestamp as _timestamp,
    validate_authority,
    validate_frontend_receipt as _validate_frontend_receipt,
    validate_runtime_receipt as _validate_runtime_receipt,
)
from source_inventory import build_source_domains, capture_inventory, frontend_environment_files

FRONTEND_RECEIPT = ".vite/restore-build.json"


def require_bindings(bindings: dict) -> tuple[dict, dict]:
    if not isinstance(bindings, dict) or set(bindings) != {"record", "manifest"}:
        raise ValueError("恢复 bindings 字段必须精确匹配当前格式")
    record, manifest = bindings["record"], bindings["manifest"]
    if not isinstance(record, dict) or not isinstance(manifest, dict) or not isinstance(record.get("plan"), dict):
        raise ValueError("恢复 bindings 缺少 record、manifest 或 plan")
    plan = record["plan"]
    checks = (
        record.get("status") == "data_verified",
        plan.get("backup_id") == manifest.get("id"),
        isinstance(record.get("plan_hash"), str) and bool(HEX_64.fullmatch(record["plan_hash"])),
        isinstance(manifest.get("source_sha"), str) and bool(HEX_40.fullmatch(manifest["source_sha"])),
        isinstance(plan.get("frontend_sha"), str) and bool(HEX_40.fullmatch(plan["frontend_sha"])),
    )
    if not all(checks):
        raise ValueError("运行产物必须绑定已完成数据校验的恢复演练")
    for field in ("id", "backup_id"):
        if not valid_identifier(plan.get(field)):
            raise ValueError(f"恢复 bindings 的 {field} 无效")
    if not valid_scope_identifier(plan.get("scope_id")):
        raise ValueError("恢复 bindings 的 scope_id 无效")
    _timestamp(record.get("data_verified_at"), "恢复数据校验时间")
    return record, manifest


def _frontend_snapshots(root: Path) -> tuple[list[dict], list[ArtifactSnapshot]]:
    dist = _directory(root / "dist", "前端 dist")
    snapshots = []
    for directory, names, filenames in os.walk(dist, followlinks=False):
        base = Path(directory)
        for name in names:
            child = base / name
            metadata = child.lstat()
            if stat.S_ISLNK(metadata.st_mode) or _is_reparse(metadata) or not stat.S_ISDIR(metadata.st_mode):
                raise ValueError("前端产物包含链接、重解析点或非普通目录")
        for name in filenames:
            path = base / name
            relative = path.relative_to(dist).as_posix()
            if relative != FRONTEND_RECEIPT:
                snapshots.append(artifact_snapshot(path))
    snapshots.sort(key=lambda item: item.path.relative_to(dist).as_posix())
    files = [
        {"path": item.path.relative_to(dist).as_posix(), "bytes": item.bytes, "sha256": item.sha256}
        for item in snapshots
    ]
    return files, snapshots


def frontend_files(root: Path) -> list[dict]:
    return _frontend_snapshots(root)[0]


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def _opener():
    return urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


def _read_response(url: str, limit: int, headers: dict | None = None) -> tuple[object, bytes]:
    request = urllib.request.Request(url, headers=headers or {})
    try:
        with _opener().open(request, timeout=10) as response:
            body = response.read(limit + 1)
            if response.status != 200 or len(body) > limit:
                raise ValueError("恢复探针拒绝非 200、重定向或超限响应")
            return getattr(response, "headers", None), body
    except (urllib.error.HTTPError, urllib.error.URLError) as error:
        raise ValueError("恢复探针连接失败或收到重定向") from error


def _verify_frontend_artifacts(root: Path, receipt: dict, base_url: str) -> tuple[list[dict], list[ArtifactSnapshot]]:
    _canonical_endpoint(base_url, "", "前端端点")
    receipt = _validate_frontend_receipt(receipt)
    files, snapshots = _frontend_snapshots(root)
    if files != receipt["files"]:
        raise ValueError("前端生产构建与文件收据不一致")
    if not {"index.html", ".vite/manifest.json"}.issubset({item["path"] for item in files}):
        raise ValueError("恢复前端缺少生产首页或 Vite manifest")
    for item in files:
        if item["path"].startswith(".vite/"):
            continue
        url = base_url + "/" + quote(item["path"], safe="/")
        _headers, body = _read_response(url, item["bytes"], {"Accept-Encoding": "identity"})
        if len(body) != item["bytes"] or hashlib.sha256(body).hexdigest() != item["sha256"]:
            raise ValueError("恢复站点实际返回的资源与生产构建不一致")
    return files, snapshots


def verify_frontend(root: Path, receipt: dict, sha: str, base_url: str) -> None:
    receipt = _validate_frontend_receipt(receipt)
    before = capture_inventory(root)
    snapshot = before["source"]["snapshot"]
    if (
        snapshot["head"] != sha
        or not snapshot["clean"]
        or receipt["sources"] != build_source_domains(before, "frontend")
        or receipt["build"]["environment_files"] != frontend_environment_files(root)
    ):
        raise ValueError("恢复前端必须使用精确干净 SHA 的生产构建")
    _files, snapshots = _verify_frontend_artifacts(root, receipt, base_url)
    if capture_inventory(root) != before or receipt["build"]["environment_files"] != frontend_environment_files(root):
        raise ValueError("核验期间前端源码发生变化")
    for snapshot in snapshots:
        snapshot.assert_unchanged()


def verify_frontend_artifacts(root: Path, receipt: dict, base_url: str) -> None:
    _files, snapshots = _verify_frontend_artifacts(root, receipt, base_url)
    for snapshot in snapshots:
        snapshot.assert_unchanged()


def _context(record: dict, manifest: dict, frontend_endpoint: str) -> dict:
    plan = record["plan"]
    return validate_authority(
        {
            "format_version": 1,
            "kind": "restore-runtime-authority",
            "restore_id": plan["id"],
            "backup_id": plan["backup_id"],
            "plan_hash": record["plan_hash"],
            "scope_id": plan["scope_id"],
            "data_verified_at": record["data_verified_at"],
            "backend_sha": manifest["source_sha"],
            "frontend_sha": plan["frontend_sha"],
            "api_endpoint": plan["api_ready_url"],
            "worker_endpoint": plan["worker_ready_url"],
            "frontend_endpoint": frontend_endpoint,
        }
    )


def _probe_api(url: str) -> dict:
    headers, body = _read_response(url, 64 * 1024, {"Accept": "application/json"})
    if headers is None or headers.get_content_type() != "application/json":
        raise ValueError("API readyz 必须返回 application/json")
    try:
        value = json.loads(
            body.decode("utf-8", errors="strict"),
            object_pairs_hook=_strict_object_pairs,
            parse_constant=_reject_json_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("API readyz 必须返回 UTF-8 严格 JSON") from error
    value = _exact_fields(value, {"status", "mysql", "redis", "object_storage"}, "API readyz")
    if (
        value["status"] != "ready"
        or value["mysql"] != "up"
        or value["redis"] not in {"up", "optional_degraded"}
        or value["object_storage"] not in {"up", "not_required"}
    ):
        raise ValueError("API readyz 没有证明必要依赖已就绪")
    return value


def _probe_worker(url: str) -> None:
    _headers, body = _read_response(url, 0)
    if body:
        raise ValueError("Worker readyz 必须返回有界空响应体")


def _observe_processes(receipt: dict, endpoints: dict) -> tuple[dict, list[JsonDocument], list[ArtifactSnapshot]]:
    observations, documents, executables = {}, [], []
    scope = receipt["restore"]["scope_id"]
    for role in ("api", "worker"):
        bound = receipt["processes"][role]
        path = Path(bound["receipt_path"])
        document, identity = _process_document(path, role, scope)
        _same_path(
            bound["receipt_path"],
            Path(receipt["paths"]["runtime_dir"]) / f"{role}.json",
            f"{role} 进程收据",
        )
        _digest_matches(bound["receipt_sha256"], document, f"{role} 进程收据")
        if identity != bound["identity"] or process_identity(identity["pid"]) != identity:
            raise ValueError("恢复进程已退出、重启或被替换")
        expected = Path(receipt["backend"]["artifacts"][role]["executable"]).absolute()
        _same_path(identity["executable"], expected, f"{role} 可执行文件")
        executable = artifact_snapshot(expected)
        artifact = receipt["backend"]["artifacts"][role]
        if executable.bytes != artifact["bytes"] or executable.sha256 != artifact["sha256"]:
            raise ValueError("恢复进程没有运行已登记的构建产物")
        verify_listener(identity["pid"], endpoints[role])
        documents.append(document)
        executables.append(executable)
        observations[role] = {
            "identity": identity,
            "process_receipt": {"path": str(document.path), "sha256": document.sha256},
            "executable": executable.descriptor(),
        }
    return observations, documents, executables


def _assert_processes(observations: dict, documents: list[JsonDocument], executables: list[ArtifactSnapshot], endpoints: dict) -> None:
    for document in documents:
        document.assert_unchanged()
    for role, executable in zip(("api", "worker"), executables, strict=True):
        identity = observations[role]["identity"]
        if process_identity(identity["pid"]) != identity:
            raise ValueError("恢复探针结束后进程已退出、重启或被替换")
        executable.assert_unchanged()
        verify_listener(identity["pid"], endpoints[role])


def _receipt_documents(receipt: dict, bindings_path: Path) -> tuple[JsonDocument, JsonDocument, JsonDocument]:
    paths, digests = receipt["paths"], receipt["digests"]
    _same_path(paths["bindings"], bindings_path, "bindings")
    bindings = read_json_document(bindings_path)
    backend_build = read_json_document(Path(paths["backend_build"]))
    frontend_build = read_json_document(Path(paths["frontend_build"]))
    for label, document in (
        ("bindings", bindings),
        ("backend_build", backend_build),
        ("frontend_build", frontend_build),
    ):
        _digest_matches(digests[label], document, label)
    if backend_build.value != receipt["backend"] or frontend_build.value != receipt["frontend"]:
        raise ValueError("恢复运行收据内嵌构建信息与原始收据不一致")
    return bindings, backend_build, frontend_build


def bind(
    backend: Path,
    frontend: Path,
    build_path: Path,
    runtime: Path,
    bindings_path: Path,
    frontend_url: str,
) -> dict:
    backend = _directory(backend, "后端源码")
    frontend = _directory(frontend, "前端源码")
    runtime = _directory(runtime, "恢复运行目录")
    bindings = read_json_document(bindings_path)
    backend_build = read_json_document(build_path)
    frontend_path = frontend / "dist" / FRONTEND_RECEIPT
    frontend_build = read_json_document(frontend_path)
    record, manifest = require_bindings(bindings.value)
    authority = _context(record, manifest, frontend_url)
    processes = {}
    for role in ("api", "worker"):
        process_document, identity = _process_document(runtime / f"{role}.json", role, authority["scope_id"])
        processes[role] = {
            "receipt_path": str(process_document.path),
            "receipt_sha256": process_document.sha256,
            "identity": identity,
        }
    receipt = {
        "format_version": 2,
        "kind": "restore-runtime",
        "restore": {
            "id": authority["restore_id"],
            "backup_id": authority["backup_id"],
            "plan_hash": authority["plan_hash"],
            "scope_id": authority["scope_id"],
            "data_verified_at": authority["data_verified_at"],
        },
        "paths": {
            "backend_root": str(backend),
            "frontend_root": str(frontend),
            "runtime_dir": str(runtime),
            "bindings": str(bindings.path),
            "backend_build": str(backend_build.path),
            "frontend_build": str(frontend_build.path),
        },
        "digests": {
            "bindings": bindings.sha256,
            "backend_build": backend_build.sha256,
            "frontend_build": frontend_build.sha256,
        },
        "source": {"backend_sha": authority["backend_sha"], "frontend_sha": authority["frontend_sha"]},
        "endpoints": {
            "api": authority["api_endpoint"],
            "worker": authority["worker_endpoint"],
            "frontend": authority["frontend_endpoint"],
        },
        "backend": backend_build.value,
        "frontend": frontend_build.value,
        "processes": processes,
    }
    verify(receipt, backend, frontend, bindings.path, authority, None)
    return receipt


def verify(
    receipt: dict,
    backend: Path,
    frontend: Path,
    bindings_path: Path,
    authority: dict,
    runtime_receipt_sha256: str | None,
) -> dict:
    receipt = _validate_runtime_receipt(receipt)
    authority = validate_authority(authority)
    backend = _directory(backend, "后端源码")
    frontend = _directory(frontend, "前端源码")
    paths = receipt["paths"]
    _same_path(paths["backend_root"], backend, "后端源码")
    _same_path(paths["frontend_root"], frontend, "前端源码")
    _directory(Path(paths["runtime_dir"]), "恢复运行目录")
    bindings, backend_build, frontend_build = _receipt_documents(receipt, bindings_path)
    record, manifest = require_bindings(bindings.value)
    if authority != _context(record, manifest, authority["frontend_endpoint"]):
        raise ValueError("stdin 权威恢复上下文与数据校验 bindings 不匹配")
    expected = {
        "restore": {
            "id": authority["restore_id"],
            "backup_id": authority["backup_id"],
            "plan_hash": authority["plan_hash"],
            "scope_id": authority["scope_id"],
            "data_verified_at": authority["data_verified_at"],
        },
        "source": {"backend_sha": authority["backend_sha"], "frontend_sha": authority["frontend_sha"]},
        "endpoints": {
            "api": authority["api_endpoint"],
            "worker": authority["worker_endpoint"],
            "frontend": authority["frontend_endpoint"],
        },
    }
    if any(receipt[key] != value for key, value in expected.items()):
        raise ValueError("恢复运行收据与权威上下文不匹配")
    backend_source = capture_inventory(backend)
    frontend_source = capture_inventory(frontend)
    verify_build(backend, backend_build.value, authority["backend_sha"])
    if backend_build.value["sources"]["full"] != backend_source:
        raise ValueError("后端构建收据的完整源码清单不匹配")
    frontend_snapshot = frontend_source["source"]["snapshot"]
    if (
        frontend_build.value["sources"] != build_source_domains(frontend_source, "frontend")
        or frontend_build.value["build"]["environment_files"] != frontend_environment_files(frontend)
        or frontend_snapshot["head"] != authority["frontend_sha"]
        or not frontend_snapshot["clean"]
    ):
        raise ValueError("前端构建收据没有绑定权威干净源码")
    observations, process_documents, executables = _observe_processes(receipt, receipt["endpoints"])
    readiness = _probe_api(receipt["endpoints"]["api"])
    _probe_worker(receipt["endpoints"]["worker"])
    files, frontend_files_snapshot = _verify_frontend_artifacts(
        frontend,
        frontend_build.value,
        receipt["endpoints"]["frontend"],
    )
    _assert_processes(observations, process_documents, executables, receipt["endpoints"])
    for document in (bindings, backend_build, frontend_build):
        document.assert_unchanged()
    for snapshot in frontend_files_snapshot:
        snapshot.assert_unchanged()
    if (capture_inventory(backend) != backend_source or capture_inventory(frontend) != frontend_source
            or frontend_build.value["build"]["environment_files"] != frontend_environment_files(frontend)):
        raise ValueError("恢复运行核验期间源码发生变化")
    return {
        "format_version": 1,
        "kind": "restore-runtime-verification",
        "status": "verified",
        "runtime_receipt_sha256": runtime_receipt_sha256,
        "bindings": {"path": str(bindings.path), "sha256": bindings.sha256},
        "build_receipts": {
            "backend": {"path": str(backend_build.path), "sha256": backend_build.sha256},
            "frontend": {"path": str(frontend_build.path), "sha256": frontend_build.sha256},
        },
        "restore": receipt["restore"],
        "source": receipt["source"],
        "endpoints": receipt["endpoints"],
        "processes": observations,
        "api_readiness": readiness,
        "frontend": {"build_receipt_sha256": frontend_build.sha256, "files": files},
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for operation in ("build", "bind", "verify"):
        command = subparsers.add_parser(operation)
        command.add_argument("--backend-dir", type=Path, required=True)
        if operation in ("build", "bind"):
            command.add_argument("--output", type=Path, required=True)
            command.add_argument("--write", action="store_true", required=True)
        if operation == "build":
            command.add_argument("--source-backend", type=Path, required=True)
            command.add_argument("--expected-head", required=True)
            command.add_argument("--adapter-contract")
            command.add_argument("--product-backend", type=Path)
        if operation in ("bind", "verify"):
            command.add_argument("--frontend-dir", type=Path, required=True)
            command.add_argument("--bindings", type=Path, required=True)
        if operation == "bind":
            command.add_argument("--frontend-url", required=True)
            command.add_argument("--build-receipt", type=Path, required=True)
            command.add_argument("--runtime-dir", type=Path, required=True)
        if operation == "verify":
            command.add_argument("--receipt", type=Path, required=True)
            command.add_argument("--frontend-url")
    args = parser.parse_args()
    backend = args.backend_dir.resolve()
    if args.command == "build":
        source, receipt = build_registered(
            backend,
            args.source_backend,
            args.expected_head,
            adapter_contract=args.adapter_contract,
            product_backend=args.product_backend,
        )
        output = validate_new_output(args.output, source)
        write_root = source
    elif args.command == "bind":
        output = validate_new_output(args.output, backend)
        write_root = backend
        receipt = bind(backend, args.frontend_dir.resolve(), args.build_receipt.resolve(),
                        args.runtime_dir.resolve(), args.bindings.resolve(), args.frontend_url)
    else:
        runtime = read_json_document(args.receipt)
        authority = read_authority()
        if args.frontend_url is not None and args.frontend_url != authority["frontend_endpoint"]:
            raise ValueError("命令行前端地址与 stdin 权威恢复上下文不匹配")
        result = verify(
            runtime.value,
            backend,
            args.frontend_dir.resolve(),
            args.bindings.resolve(),
            authority,
            runtime.sha256,
        )
        runtime.assert_unchanged()
        print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        return
    write_new(output, receipt, write_root)
    print(json.dumps({"output": str(output)}))


if __name__ == "__main__":
    main()
