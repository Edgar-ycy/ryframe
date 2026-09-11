"""显式构建、绑定与只读核验恢复验收的真实运行产物；不执行备份或恢复。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import quote

from full_stack_process import process_identity
from process_sockets import verify_listener
from restore_build import (
    build_registered,
    repository,
    validate_new_output,
    write_new,
)
from restore_frontend_build import (
    FRONTEND_RECEIPT,
    build_registered_frontend,
    frontend_files,
    frontend_snapshots,
)
from restore_runtime_source import require_bindings, resolve_runtime_sources, runtime_authority
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
    process_document as _process_document,
    read_authority,
    read_json as read_json,
    read_json_document,
    reject_json_constant as _reject_json_constant,
    same_path as _same_path,
    strict_object_pairs as _strict_object_pairs,
    validate_authority,
    validate_frontend_receipt as _validate_frontend_receipt,
    validate_runtime_receipt as _validate_runtime_receipt,
)
from source_inventory import build_source_domains, capture_inventory, frontend_environment_files

ROLES = ("api", "worker", "frontend")


def _descriptor(document: JsonDocument) -> dict:
    return {"path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256}


def validate_launch(value: object, generation: Path) -> dict:
    """延迟加载私有生命周期模型，避免恢复来源工具的导入环。"""
    from restore_runtime_generation import validate_launch as validate

    return validate(value, generation)


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
    files, snapshots = frontend_snapshots(root)
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


def _observe_processes(
    receipt: dict, launch: dict, endpoints: dict
) -> tuple[dict, list[JsonDocument], list[ArtifactSnapshot]]:
    observations, documents, executables = {}, [], []
    scope = receipt["restore"]["scope_id"]
    for role in ROLES:
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
        expected = (
            Path(receipt["backend"]["artifacts"][role]["executable"]).absolute()
            if role != "frontend"
            else Path(launch["processes"][role]["command"][0]).absolute()
        )
        _same_path(identity["executable"], expected, f"{role} 可执行文件")
        executable = artifact_snapshot(expected)
        artifact = receipt["backend"]["artifacts"].get(role)
        if artifact is not None and (
            executable.bytes != artifact["bytes"] or executable.sha256 != artifact["sha256"]
        ):
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
    for role, executable in zip(ROLES, executables, strict=True):
        identity = observations[role]["identity"]
        if process_identity(identity["pid"]) != identity:
            raise ValueError("恢复探针结束后进程已退出、重启或被替换")
        executable.assert_unchanged()
        verify_listener(identity["pid"], endpoints[role])


def _receipt_documents(
    receipt: dict, bindings_path: Path
) -> tuple[JsonDocument, JsonDocument, JsonDocument, JsonDocument]:
    paths, digests = receipt["paths"], receipt["digests"]
    _same_path(paths["bindings"], bindings_path, "bindings")
    bindings = read_json_document(bindings_path)
    backend_build = read_json_document(Path(paths["backend_build"]))
    frontend_build = read_json_document(Path(paths["frontend_build"]))
    launch = read_json_document(Path(paths["launch"]))
    for label, document in (
        ("bindings", bindings),
        ("backend_build", backend_build),
        ("frontend_build", frontend_build),
        ("launch", launch),
    ):
        _digest_matches(digests[label], document, label)
    if backend_build.value != receipt["backend"] or frontend_build.value != receipt["frontend"]:
        raise ValueError("恢复运行收据内嵌构建信息与原始收据不一致")
    validate_launch(launch.value, Path(paths["runtime_dir"]))
    return bindings, backend_build, frontend_build, launch


def bind(
    coordinator: Path,
    execution_backend: Path,
    frontend: Path,
    build_path: Path,
    launch_path: Path,
    bindings_path: Path,
    *,
    adapter_contract: str | None = None,
    product_backend: Path | None = None,
) -> dict:
    launch_document = read_json_document(launch_path)
    launch = validate_launch(launch_document.value, launch_document.path.parent)
    runtime = _directory(Path(launch["runtime_directory"]), "恢复运行目录")
    _same_path(str(launch_document.path), runtime / "runtime-launch.json", "恢复运行启动收据")
    bindings = read_json_document(bindings_path)
    backend_build = read_json_document(build_path)
    frontend_build = read_json_document(frontend / "dist" / FRONTEND_RECEIPT)
    record, manifest = require_bindings(bindings.value)
    resolved = resolve_runtime_sources(
        coordinator,
        execution_backend,
        frontend,
        backend_build,
        frontend_build,
        record["plan"]["frontend_sha"],
        adapter_contract=adapter_contract,
        product_backend=product_backend,
    )
    authority = runtime_authority(
        record, manifest, resolved["source"], launch["request"]["authority"]["frontend_endpoint"]
    )
    if (
        launch["request"]["authority"] != authority
        or launch["request"]["roots"] != resolved["roots"]
        or launch["request"]["paths"]
        != {
            "bindings": str(bindings.path),
            "backend_build": str(backend_build.path),
            "frontend_build": str(frontend_build.path),
        }
        or launch["request"]["digests"]
        != {
            "bindings": bindings.sha256,
            "backend_build": backend_build.sha256,
            "frontend_build": frontend_build.sha256,
        }
    ):
        raise ValueError("恢复运行启动收据与当前来源、构建或数据绑定不一致")
    source = {"backup_source_sha": authority["backup_source_sha"], **resolved["source"]}
    processes = {}
    for role in ROLES:
        process_document, identity = _process_document(runtime / f"{role}.json", role, authority["scope_id"])
        processes[role] = {
            "receipt_path": str(process_document.path),
            "receipt_sha256": process_document.sha256,
            "identity": identity,
        }
    receipt = {
        "format_version": 3,
        "kind": "restore-runtime",
        "restore": {
            "id": authority["restore_id"],
            "backup_id": authority["backup_id"],
            "plan_hash": authority["plan_hash"],
            "scope_id": authority["scope_id"],
            "data_verified_at": authority["data_verified_at"],
        },
        "paths": {
            "backend_product_root": resolved["roots"]["backend_product"],
            "backend_execution_root": resolved["roots"]["backend_execution"],
            "frontend_root": resolved["roots"]["frontend"],
            "runtime_dir": str(runtime),
            "bindings": str(bindings.path),
            "backend_build": str(backend_build.path),
            "frontend_build": str(frontend_build.path),
            "launch": str(launch_document.path),
        },
        "digests": {
            "bindings": bindings.sha256,
            "backend_build": backend_build.sha256,
            "frontend_build": frontend_build.sha256,
            "launch": launch_document.sha256,
        },
        "source": source,
        "endpoints": {
            "api": authority["api_endpoint"],
            "worker": authority["worker_endpoint"],
            "frontend": authority["frontend_endpoint"],
        },
        "backend": backend_build.value,
        "frontend": frontend_build.value,
        "processes": processes,
    }
    verify(
        receipt,
        coordinator,
        execution_backend,
        frontend,
        bindings.path,
        authority,
        None,
        product_backend=product_backend,
    )
    return receipt


def _bind_control_inputs(coordinator: Path, launch_path: Path):
    """把 bind 精确绑定到同一 registration、target plan 与运行中代次。"""
    from restore_runtime_generation import state_document, validate_state
    from restore_runtime_registration import registration_binding

    launch_document = read_json_document(launch_path)
    launch = validate_launch(launch_document.value, launch_document.path.parent)
    binding = launch["request"]["registration"]
    registration_path = Path(binding["registration"]["path"])
    target_path = Path(binding["target_plan"]["path"])
    target = read_json_document(target_path)
    if _descriptor(target) != binding["target_plan"]:
        raise ValueError("恢复运行 bind 的 target plan 描述已经变化")
    _registration, facts, registration_documents = registration_binding(
        coordinator, registration_path, binding["target_plan"]
    )
    if (
        _descriptor(registration_documents[0]) != binding["registration"]
        or registration_documents[2].path != target.path
        or registration_documents[2].raw != target.raw
    ):
        raise ValueError("恢复运行 bind 没有绑定同一 registration 或 target plan")
    root = Path(facts["runtime_directory"])
    lifecycle = state_document(root)
    if lifecycle is None:
        raise ValueError("恢复运行 bind 缺少 lifecycle 状态")
    state = validate_state(lifecycle.value, root, binding)
    generation_number = launch["generation"]
    if generation_number > len(state["generations"]):
        raise ValueError("恢复运行 bind 的 generation 不存在")
    generation = state["generations"][generation_number - 1]
    if (
        generation["number"] != generation_number
        or generation["status"] != "running"
        or generation["request"] != launch["request"]
        or generation["launch"] != _descriptor(launch_document)
    ):
        raise ValueError("恢复运行 bind 只接受同一 lifecycle 的运行中 generation")
    documents = (*registration_documents, target, lifecycle, launch_document)
    for document in documents:
        document.assert_unchanged()
    return root, binding, launch, documents


def bind_and_write(
    coordinator: Path,
    execution_backend: Path,
    frontend: Path,
    build_path: Path,
    launch_path: Path,
    bindings_path: Path,
    output: Path,
    *,
    adapter_contract: str | None = None,
    product_backend: Path | None = None,
) -> dict:
    """在全局 ownership 锁内复核运行代次并原子签发运行收据。"""
    from restore_runtime_registration import REGISTRATION_LOCK, runtime_control_directory
    from runtime_control_lock import controller_lock

    output = validate_new_output(output, coordinator)
    root, binding, launch, documents = _bind_control_inputs(coordinator, launch_path)
    if output == root or output.is_relative_to(root):
        raise ValueError("恢复运行收据不能写入 lifecycle 管理的运行目录")
    operation = (
        f"runtime-bind:{binding['registration']['sha256']}:"
        f"{binding['target_plan']['sha256']}:{launch['generation']}"
    )
    with controller_lock(runtime_control_directory(coordinator), operation, REGISTRATION_LOCK):
        repeated_root, repeated_binding, repeated_launch, repeated_documents = (
            _bind_control_inputs(coordinator, launch_path)
        )
        if (
            repeated_root != root
            or repeated_binding != binding
            or repeated_launch != launch
            or [_descriptor(item) for item in repeated_documents]
            != [_descriptor(item) for item in documents]
        ):
            raise ValueError("取得 ownership 控制锁前 bind 输入或运行代次发生变化")
        receipt = bind(
            coordinator,
            execution_backend,
            frontend,
            build_path,
            launch_path,
            bindings_path,
            adapter_contract=adapter_contract,
            product_backend=product_backend,
        )
        write_new(output, receipt, coordinator)
        written = read_json_document(output)
        if written.value != receipt:
            raise ValueError("恢复运行收据写入后内容不一致")
        _validate_runtime_receipt(written.value)
        for document in (*documents, *repeated_documents, written):
            document.assert_unchanged()
    return receipt


def verify(
    receipt: dict,
    coordinator: Path,
    execution_backend: Path,
    frontend: Path,
    bindings_path: Path,
    authority: dict,
    runtime_receipt_sha256: str | None,
    *,
    product_backend: Path | None = None,
) -> dict:
    receipt = _validate_runtime_receipt(receipt)
    authority = validate_authority(authority)
    paths = receipt["paths"]
    _same_path(paths["backend_execution_root"], execution_backend, "后端执行源码")
    expected_product = execution_backend if product_backend is None else product_backend
    _same_path(paths["backend_product_root"], expected_product, "后端产品源码")
    _same_path(paths["frontend_root"], frontend, "前端源码")
    _directory(Path(paths["runtime_dir"]), "恢复运行目录")
    bindings, backend_build, frontend_build, launch_document = _receipt_documents(
        receipt, bindings_path
    )
    launch = launch_document.value
    record, manifest = require_bindings(bindings.value)
    resolved = resolve_runtime_sources(
        coordinator,
        execution_backend,
        frontend,
        backend_build,
        frontend_build,
        record["plan"]["frontend_sha"],
        adapter_contract=authority["backend_adapter_contract"],
        product_backend=product_backend,
    )
    source = {"backup_source_sha": manifest["source_sha"], **resolved["source"]}
    if authority != runtime_authority(
        record, manifest, resolved["source"], authority["frontend_endpoint"]
    ):
        raise ValueError("stdin 权威恢复上下文与数据校验 bindings 不匹配")
    expected = {
        "restore": {
            "id": authority["restore_id"],
            "backup_id": authority["backup_id"],
            "plan_hash": authority["plan_hash"],
            "scope_id": authority["scope_id"],
            "data_verified_at": authority["data_verified_at"],
        },
        "source": source,
        "endpoints": {
            "api": authority["api_endpoint"],
            "worker": authority["worker_endpoint"],
            "frontend": authority["frontend_endpoint"],
        },
    }
    if any(receipt[key] != value for key, value in expected.items()):
        raise ValueError("恢复运行收据与权威上下文不匹配")
    if (
        launch["request"]["authority"] != authority
        or launch["request"]["roots"]
        != {
            "backend_product": paths["backend_product_root"],
            "backend_execution": paths["backend_execution_root"],
            "frontend": paths["frontend_root"],
        }
        or launch["request"]["paths"]
        != {key: paths[key] for key in ("bindings", "backend_build", "frontend_build")}
        or launch["request"]["digests"]
        != {key: receipt["digests"][key] for key in ("bindings", "backend_build", "frontend_build")}
    ):
        raise ValueError("恢复运行启动代次与运行收据的来源或权威上下文不一致")
    observations, process_documents, executables = _observe_processes(
        receipt, launch, receipt["endpoints"]
    )
    readiness = _probe_api(receipt["endpoints"]["api"])
    _probe_worker(receipt["endpoints"]["worker"])
    files, frontend_files_snapshot = _verify_frontend_artifacts(
        frontend,
        frontend_build.value,
        receipt["endpoints"]["frontend"],
    )
    _assert_processes(observations, process_documents, executables, receipt["endpoints"])
    for document in (bindings, backend_build, frontend_build, launch_document):
        document.assert_unchanged()
    for snapshot in frontend_files_snapshot:
        snapshot.assert_unchanged()
    repeated = resolve_runtime_sources(
        coordinator,
        execution_backend,
        frontend,
        backend_build,
        frontend_build,
        authority["frontend_sha"],
        adapter_contract=authority["backend_adapter_contract"],
        product_backend=product_backend,
    )
    if repeated != resolved:
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
            "launch": {"path": str(launch_document.path), "sha256": launch_document.sha256},
        },
        "restore": receipt["restore"],
        "source": receipt["source"],
        "endpoints": receipt["endpoints"],
        "processes": observations,
        "api_readiness": readiness,
        "frontend": {"build_receipt_sha256": frontend_build.sha256, "files": files},
    }


def main() -> None:
    from restore_runtime_registration import (
        add_arguments as add_registration_arguments,
        execute as execute_registration,
    )
    from restore_runtime_lifecycle import (
        add_arguments as add_lifecycle_arguments,
        dispatch as dispatch_lifecycle,
    )

    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for operation in ("build", "register", "start", "status", "stop", "recover", "bind", "verify"):
        command = subparsers.add_parser(operation)
        command.add_argument("--backend-dir", type=Path, required=True)
        if operation == "register":
            add_registration_arguments(command)
            continue
        if operation in {"start", "status", "stop", "recover"}:
            add_lifecycle_arguments(command, operation)
            continue
        if operation in ("build", "bind"):
            command.add_argument("--output", type=Path, required=True)
            command.add_argument("--write", action="store_true", required=True)
        if operation == "build":
            command.add_argument("--frontend-dir", type=Path, required=True)
            command.add_argument("--source-backend", type=Path, required=True)
            command.add_argument("--expected-head", required=True)
            command.add_argument("--source-frontend", type=Path, required=True)
            command.add_argument("--expected-frontend-head", required=True)
            command.add_argument("--adapter-contract")
            command.add_argument("--product-backend", type=Path)
        if operation in ("bind", "verify"):
            command.add_argument("--source-backend", type=Path, required=True)
            command.add_argument("--source-frontend", type=Path, required=True)
            command.add_argument("--adapter-contract")
            command.add_argument("--product-backend", type=Path)
            command.add_argument("--bindings", type=Path, required=True)
        if operation == "bind":
            command.add_argument("--build-receipt", type=Path, required=True)
            command.add_argument("--launch-receipt", type=Path, required=True)
        if operation == "verify":
            command.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    backend = args.backend_dir.resolve()
    if args.command == "register":
        print(json.dumps(execute_registration(args, backend), ensure_ascii=False,
                         sort_keys=True, separators=(",", ":")))
        return
    if args.command in {"start", "status", "stop", "recover"}:
        result = dispatch_lifecycle(
            args, backend, _probe_api, _probe_worker, verify_frontend_artifacts
        )
        print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        return
    if args.command == "build":
        expected_source = repository(args.source_backend, "后端构建来源")
        output = validate_new_output(args.output, expected_source)
        frontend_source, frontend_receipt, frontend_action = build_registered_frontend(
            args.frontend_dir.resolve(),
            args.source_frontend,
            args.expected_frontend_head,
        )
        source, receipt = build_registered(
            backend,
            expected_source,
            args.expected_head,
            adapter_contract=args.adapter_contract,
            product_backend=args.product_backend,
        )
        if source != expected_source:
            raise ValueError("后端构建来源在校验与构建之间发生变化")
        write_new(output, receipt, source)
        print(json.dumps({
            "backend_receipt": str(output),
            "frontend_receipt": str(frontend_receipt.path),
            "frontend_source": str(frontend_source),
            "frontend_action": frontend_action,
        }, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        return
    elif args.command == "bind":
        bind_and_write(
            backend,
            args.source_backend,
            args.source_frontend,
            args.build_receipt.resolve(),
            args.launch_receipt.resolve(),
            args.bindings.resolve(),
            args.output,
            adapter_contract=args.adapter_contract,
            product_backend=args.product_backend,
        )
        print(json.dumps({"output": str(args.output.resolve())}))
        return
    else:
        runtime = read_json_document(args.receipt)
        authority = read_authority()
        if args.adapter_contract != authority["backend_adapter_contract"]:
            raise ValueError("命令行适配合同与 stdin 权威恢复上下文不匹配")
        result = verify(
            runtime.value,
            backend,
            args.source_backend,
            args.source_frontend,
            args.bindings.resolve(),
            authority,
            runtime.sha256,
            product_backend=args.product_backend,
        )
        runtime.assert_unchanged()
        print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        return


if __name__ == "__main__":
    main()
