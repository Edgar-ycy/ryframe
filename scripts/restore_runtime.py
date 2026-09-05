"""显式构建、绑定与只读核验恢复验收的真实运行产物；不执行备份或恢复。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import stat
import sys
import urllib.request
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from urllib.parse import quote, urlsplit

from full_stack_process import process_identity, read_process
from restore_build import (
    build,
    file_digest,
    source_snapshot,
    validate_new_output,
    verify_build,
    write_new,
)
from process_sockets import endpoint, verify_listener

FRONTEND_RECEIPT = ".vite/restore-build.json"
MAX_JSON_BYTES = 16 * 1024 * 1024
HEX_40 = re.compile(r"[a-f0-9]{40}")
HEX_64 = re.compile(r"[a-f0-9]{64}")
IDENTIFIER = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
AUTHORITY_FIELDS = {
    "format_version",
    "kind",
    "restore_id",
    "backup_id",
    "plan_hash",
    "scope_id",
    "data_verified_at",
    "backend_sha",
    "frontend_sha",
    "api_endpoint",
    "worker_endpoint",
    "frontend_endpoint",
}


def _is_reparse(metadata: os.stat_result) -> bool:
    flag = getattr(stat, "FILE_ATTRIBUTE_REPARSE_POINT", 0x400)
    return bool(getattr(metadata, "st_file_attributes", 0) & flag)


def _reject_link_or_reparse(path: Path) -> None:
    current = path.absolute()
    while True:
        metadata = current.lstat()
        if stat.S_ISLNK(metadata.st_mode) or _is_reparse(metadata):
            raise ValueError(f"恢复核验输入不得经过符号链接或重解析点：{current}")
        if current.parent == current:
            return
        current = current.parent


def _file_state(metadata: os.stat_result) -> tuple[int, int, int, int]:
    return (
        metadata.st_dev,
        metadata.st_ino,
        metadata.st_size,
        metadata.st_mtime_ns,
    )


def _strict_object_pairs(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"恢复核验 JSON 包含重复字段：{key}")
        result[key] = value
    return result


def _reject_json_constant(value: str) -> None:
    raise ValueError(f"恢复核验 JSON 包含非标准数值：{value}")


@dataclass(frozen=True)
class JsonDocument:
    path: Path
    raw: bytes
    value: dict
    state: tuple[int, int, int, int]

    @property
    def sha256(self) -> str:
        return hashlib.sha256(self.raw).hexdigest()

    def assert_unchanged(self) -> None:
        current = read_json_document(self.path)
        if current.state != self.state or current.raw != self.raw:
            raise ValueError(f"核验期间 JSON 输入被替换或修改：{self.path}")


def read_json_document(path: Path) -> JsonDocument:
    path = path.absolute()
    _reject_link_or_reparse(path)
    before = path.lstat()
    if not stat.S_ISREG(before.st_mode):
        raise ValueError(f"恢复核验输入必须是普通文件：{path}")
    if before.st_size == 0:
        raise ValueError(f"恢复核验输入不能为空：{path}")
    if before.st_size > MAX_JSON_BYTES:
        raise ValueError(f"恢复核验输入超过 16 MiB：{path}")
    with path.open("rb") as stream:
        opened = os.fstat(stream.fileno())
        if _file_state(opened) != _file_state(before) or not stat.S_ISREG(opened.st_mode):
            raise ValueError(f"恢复核验输入在打开前被替换：{path}")
        raw = stream.read(MAX_JSON_BYTES + 1)
        after_read = os.fstat(stream.fileno())
    after = path.lstat()
    state = _file_state(before)
    if len(raw) > MAX_JSON_BYTES or _file_state(after_read) != state or _file_state(after) != state:
        raise ValueError(f"恢复核验输入在读取期间被替换或修改：{path}")
    try:
        text = raw.decode("utf-8", errors="strict")
        value = json.loads(
            text,
            object_pairs_hook=_strict_object_pairs,
            parse_constant=_reject_json_constant,
        )
    except UnicodeDecodeError as error:
        raise ValueError(f"恢复核验输入不是 UTF-8：{path}") from error
    except json.JSONDecodeError as error:
        raise ValueError(f"恢复核验输入不是严格 JSON：{path}") from error
    if not isinstance(value, dict):
        raise ValueError(f"恢复核验输入必须是 JSON 对象：{path}")
    return JsonDocument(path, raw, value, state)


def read_json(path: Path) -> dict:
    return read_json_document(path).value


def _exact_fields(value: object, fields: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        raise ValueError(f"{label}字段必须精确匹配当前格式")
    return value


def _canonical_endpoint(url: object, path: str, label: str) -> str:
    if not isinstance(url, str):
        raise ValueError(f"{label}必须是字符串")
    _family, host, port = endpoint(url)
    parsed = urlsplit(url)
    if parsed.port is None or parsed.path != path:
        raise ValueError(f"{label}必须使用显式端口和精确路径 {path or '/'}")
    authority = f"[{host}]" if ":" in host else host
    canonical = f"{parsed.scheme}://{authority}:{port}{path}"
    if url != canonical:
        raise ValueError(f"{label}必须使用规范化 loopback 地址")
    return canonical


def validate_authority(value: object) -> dict:
    authority = _exact_fields(value, AUTHORITY_FIELDS, "权威恢复上下文")
    if authority["format_version"] != 1 or authority["kind"] != "restore-runtime-authority":
        raise ValueError("权威恢复上下文版本或类型不匹配")
    for field in ("restore_id", "backup_id", "scope_id"):
        if not isinstance(authority[field], str) or not IDENTIFIER.fullmatch(authority[field]):
            raise ValueError(f"权威恢复上下文的 {field} 无效")
    for field, pattern in (("plan_hash", HEX_64), ("backend_sha", HEX_40), ("frontend_sha", HEX_40)):
        if not isinstance(authority[field], str) or not pattern.fullmatch(authority[field]):
            raise ValueError(f"权威恢复上下文的 {field} 无效")
    timestamp = authority["data_verified_at"]
    if not isinstance(timestamp, str):
        raise ValueError("权威恢复上下文缺少 data_verified_at")
    try:
        parsed_time = datetime.fromisoformat(timestamp.replace("Z", "+00:00"))
    except ValueError as error:
        raise ValueError("权威恢复上下文的 data_verified_at 无效") from error
    if parsed_time.tzinfo is None or parsed_time.utcoffset() is None:
        raise ValueError("权威恢复上下文的 data_verified_at 必须包含时区")
    endpoints = {
        "api": _canonical_endpoint(authority["api_endpoint"], "/readyz", "API 端点"),
        "worker": _canonical_endpoint(authority["worker_endpoint"], "/readyz", "Worker 端点"),
        "frontend": _canonical_endpoint(authority["frontend_endpoint"], "", "前端端点"),
    }
    if len({urlsplit(url).port for url in endpoints.values()}) != 3:
        raise ValueError("API、Worker 与前端必须使用三个互异的显式端口")
    return authority


def read_authority(stream=None) -> dict:
    source = stream if stream is not None else sys.stdin
    raw = source.buffer.read(MAX_JSON_BYTES + 1) if hasattr(source, "buffer") else source.read(MAX_JSON_BYTES + 1)
    if isinstance(raw, str):
        raw = raw.encode("utf-8")
    if not raw or len(raw) > MAX_JSON_BYTES:
        raise ValueError("verify 必须从 stdin 接收非空且不超过 16 MiB 的权威恢复上下文")
    try:
        value = json.loads(
            raw.decode("utf-8", errors="strict"),
            object_pairs_hook=_strict_object_pairs,
            parse_constant=_reject_json_constant,
        )
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("stdin 权威恢复上下文必须是 UTF-8 严格 JSON") from error
    return validate_authority(value)


def require_bindings(bindings: dict) -> tuple[dict, dict]:
    record, manifest = bindings["record"], bindings["manifest"]
    if (record["status"] != "data_verified" or record["plan"]["backup_id"] != manifest["id"]
            or not re.fullmatch(r"[a-f0-9]{64}", record["plan_hash"])
            or not re.fullmatch(r"[a-f0-9]{40}", manifest["source_sha"])
            or not re.fullmatch(r"[a-f0-9]{40}", record["plan"]["frontend_sha"])):
        raise ValueError("运行产物必须绑定已完成数据校验的恢复演练")
    return record, manifest


def frontend_files(root: Path) -> list[dict]:
    dist = (root / "dist").resolve(strict=True)
    files = []
    for path in dist.rglob("*"):
        if path.is_symlink() or not path.resolve().is_relative_to(dist):
            raise ValueError("前端产物包含符号链接或越界路径")
        if path.is_file() and path.relative_to(dist).as_posix() != FRONTEND_RECEIPT:
            files.append({"path": path.relative_to(dist).as_posix(), **file_digest(path)})
    return sorted(files, key=lambda value: value["path"])


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def verify_frontend(root: Path, receipt: dict, sha: str, base_url: str) -> None:
    if (receipt.get("format_version") != 1 or receipt.get("kind") != "restore-frontend-build"
            or receipt.get("source", {}).get("head") != sha or not receipt.get("source", {}).get("clean")
            or receipt["source"] != source_snapshot(root)):
        raise ValueError("恢复前端必须使用精确干净 SHA 的生产构建")
    verify_frontend_artifacts(root, receipt, base_url)


def verify_frontend_artifacts(root: Path, receipt: dict, base_url: str) -> None:
    endpoint(base_url)
    if urlsplit(base_url).path not in ("", "/"):
        raise ValueError("前端地址必须是独立 loopback 站点根地址")
    if receipt.get("format_version") != 1 or receipt.get("kind") != "restore-frontend-build":
        raise ValueError("前端必须使用生产构建收据")
    files = frontend_files(root)
    if files != sorted(receipt.get("files", []), key=lambda value: value["path"]):
        raise ValueError("前端生产构建与文件收据不一致")
    if not {"index.html", ".vite/manifest.json"}.issubset({file["path"] for file in files}):
        raise ValueError("恢复前端缺少生产首页或 Vite manifest")
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    for file in files:
        # Vite 内部 manifest 不对外提供；全部实际发布文件必须来自当前构建。
        if file["path"].startswith(".vite/"):
            continue
        url = base_url.rstrip("/") + "/" + quote(file["path"], safe="/")
        request = urllib.request.Request(url, headers={"Accept-Encoding": "identity"})
        with opener.open(request, timeout=10) as response:
            data = response.read(file["bytes"] + 1)
            if response.status != 200 or len(data) != file["bytes"] or hashlib.sha256(data).hexdigest() != file["sha256"]:
                raise ValueError("恢复站点实际返回的资源与生产构建不一致")


def verify_processes(receipt: dict, record: dict) -> None:
    scope = record["plan"]["scope_id"]
    directory = Path(receipt["runtime_dir"])
    if set(receipt.get("processes", {})) != {"api", "worker"}:
        raise ValueError("恢复运行收据缺少 API 或 Worker")
    for role in ("api", "worker"):
        identity = receipt["processes"][role]
        if (read_process(directory, role, scope) != identity
                or process_identity(identity["pid"]) != identity
                or Path(identity["executable"]).resolve() != Path(receipt["backend"]["artifacts"][role]["executable"]).resolve()):
            raise ValueError("恢复期间进程退出、重启或替换了构建产物")
        url = record["plan"][f"{role}_ready_url"]
        if urlsplit(url).path != "/readyz":
            raise ValueError("恢复进程必须绑定其 readyz 探针")
        verify_listener(identity["pid"], url)


def bind(backend: Path, frontend: Path, build_path: Path, runtime: Path,
         bindings_path: Path, frontend_url: str) -> dict:
    bindings = read_json(bindings_path)
    record, _manifest = require_bindings(bindings)
    receipt = {"format_version": 1, "kind": "restore-runtime",
               "restore_id": record["plan"]["id"], "plan_hash": record["plan_hash"],
               "scope_id": record["plan"]["scope_id"], "bindings_sha256": file_digest(bindings_path)["sha256"],
               "backend_root": str(backend), "frontend_root": str(frontend), "runtime_dir": str(runtime),
               "frontend_url": frontend_url.rstrip("/"), "backend": read_json(build_path),
               "frontend": read_json(frontend / "dist" / FRONTEND_RECEIPT),
               "processes": {role: read_process(runtime, role, record["plan"]["scope_id"])
                             for role in ("api", "worker")}}
    verify(receipt, backend, frontend, bindings_path, frontend_url)
    return receipt


def verify(receipt: dict, backend: Path, frontend: Path, bindings_path: Path, frontend_url: str) -> None:
    record, manifest = require_bindings(read_json(bindings_path))
    expected = {"format_version": 1, "kind": "restore-runtime", "restore_id": record["plan"]["id"],
                "scope_id": record["plan"]["scope_id"], "plan_hash": record["plan_hash"],
                "bindings_sha256": file_digest(bindings_path)["sha256"], "backend_root": str(backend),
                "frontend_root": str(frontend), "frontend_url": frontend_url.rstrip("/")}
    if any(receipt.get(key) != value for key, value in expected.items()):
        raise ValueError("运行收据与恢复演练、源码目录或浏览器地址不匹配")
    verify_build(backend, receipt["backend"], manifest["source_sha"])
    if read_json(frontend / "dist" / FRONTEND_RECEIPT) != receipt["frontend"]:
        raise ValueError("前端构建收据已变化")
    verify_frontend(frontend, receipt["frontend"], record["plan"]["frontend_sha"], frontend_url)
    verify_processes(receipt, record)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    for operation in ("build", "bind", "verify"):
        command = subparsers.add_parser(operation)
        command.add_argument("--backend-dir", type=Path, required=True)
        if operation in ("build", "bind"):
            command.add_argument("--output", type=Path, required=True)
            command.add_argument("--write", action="store_true", required=True)
        if operation in ("bind", "verify"):
            command.add_argument("--frontend-dir", type=Path, required=True)
            command.add_argument("--bindings", type=Path, required=True)
            command.add_argument("--frontend-url", required=True)
        if operation == "bind":
            command.add_argument("--build-receipt", type=Path, required=True)
            command.add_argument("--runtime-dir", type=Path, required=True)
        if operation == "verify":
            command.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    backend = args.backend_dir.resolve()
    output = validate_new_output(args.output, backend) if hasattr(args, "output") else None
    if args.command == "build":
        receipt = build(backend)
    elif args.command == "bind":
        receipt = bind(backend, args.frontend_dir.resolve(), args.build_receipt.resolve(),
                       args.runtime_dir.resolve(), args.bindings.resolve(), args.frontend_url)
    else:
        raw = args.receipt.read_bytes()
        verify(read_json(args.receipt), backend, args.frontend_dir.resolve(), args.bindings.resolve(), args.frontend_url)
        if raw != args.receipt.read_bytes():
            raise ValueError("核验期间运行收据被替换")
        print(json.dumps({"runtime_receipt_sha256": hashlib.sha256(raw).hexdigest()}))
        return
    write_new(output, receipt, backend)
    print(json.dumps({"output": str(output)}))


if __name__ == "__main__":
    main()
