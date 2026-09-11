"""严格核验 Device preview 浏览器实际接收的静态响应。"""

from __future__ import annotations

from pathlib import Path, PurePosixPath
import re
from urllib.parse import quote, unquote

from restore_runtime_evidence import exact_fields, read_json_document


MAX_ENTRIES = 10_000
MAX_BYTES = 8 * 1024 * 1024 * 1024
SHA256 = re.compile(r"[a-f0-9]{64}")
DESTINATIONS = frozenset({
    "audio", "document", "embed", "font", "frame", "iframe", "image", "manifest",
    "object", "script", "sharedworker", "style", "track", "video", "worker",
})
URL_SAFE = "/-._~!$&'()*+,;=:@"


def _manifest_files(manifest: object) -> dict[str, dict]:
    value = exact_fields(
        manifest,
        {"format_version", "kind", "root", "limits", "total_files", "total_bytes", "files"},
        "Device 前端生产产物清单",
    )
    if value["format_version"] != 1 or value["kind"] != "bounded-artifact-manifest" \
            or not isinstance(value["files"], list):
        raise ValueError("Device 前端生产产物清单版本或文件列表无效")
    files = {}
    for item in value["files"]:
        entry = exact_fields(item, {"path", "bytes", "sha256"}, "Device 前端生产文件")
        path = entry["path"]
        if not isinstance(path, str) or not path or path in files:
            raise ValueError("Device 前端生产产物清单包含重复或无效路径")
        files[path] = entry
    return files


def _target(path: object) -> str:
    if not isinstance(path, str) or not path.startswith("/") or "\\" in path:
        raise ValueError("Device 静态响应路径不是规范化同源路径")
    try:
        decoded = unquote(path, errors="strict")
    except UnicodeDecodeError as error:
        raise ValueError("Device 静态响应路径不是规范化 UTF-8") from error
    if quote(decoded, safe=URL_SAFE) != path or "?" in decoded or "#" in decoded:
        raise ValueError("Device 静态响应路径包含查询、片段或非规范化转义")
    segments = decoded.removeprefix("/").split("/")
    if any(part in {".", ".."} or not part and index != len(segments) - 1
           for index, part in enumerate(segments)):
        raise ValueError("Device 静态响应路径包含空段或目录跳转")
    relative = PurePosixPath(decoded.removeprefix("/"))
    if decoded == "/" or not relative.suffix:
        return "index.html"
    return relative.as_posix()


def preview_responses(path: Path, binding: dict, manifest: object) -> dict:
    document = read_json_document(path)
    receipt = exact_fields(
        document.value,
        {"format_version", "kind", "status", "run_id", "scope_id", "limits",
         "total_entries", "total_bytes", "entries"},
        "Device preview 静态响应收据",
    )
    limits = exact_fields(receipt["limits"], {"entries", "bytes"}, "Device 静态响应上限")
    entries = receipt["entries"]
    if (
        receipt["format_version"] != 1
        or receipt["kind"] != "device-preview-static-responses"
        or receipt["status"] != "complete"
        or receipt["run_id"] != binding["run_id"]
        or receipt["scope_id"] != binding["scope_id"]
        or limits != {"entries": MAX_ENTRIES, "bytes": MAX_BYTES}
        or not isinstance(entries, list)
        or not 1 <= len(entries) <= MAX_ENTRIES
        or receipt["total_entries"] != len(entries)
        or type(receipt["total_bytes"]) is not int
    ):
        raise ValueError("Device preview 静态响应收据身份、状态或上限无效")
    files = _manifest_files(manifest)
    total = 0
    targets = set()
    for index, raw in enumerate(entries, 1):
        entry = exact_fields(
            raw,
            {"sequence", "method", "path", "destination", "status", "bytes", "sha256",
             "representation"},
            "Device preview 静态响应",
        )
        target = _target(entry["path"])
        expected = files.get(target)
        if (
            entry["sequence"] != index
            or entry["method"] != "GET"
            or entry["destination"] not in DESTINATIONS
            or entry["status"] != 200
            or entry["representation"] != "identity"
            or type(entry["bytes"]) is not int
            or entry["bytes"] < 0
            or not isinstance(entry["sha256"], str)
            or SHA256.fullmatch(entry["sha256"]) is None
            or expected is None
            or entry["bytes"] != expected["bytes"]
            or entry["sha256"] != expected["sha256"]
        ):
            raise ValueError("Device preview 静态响应与绑定生产产物不一致")
        total += entry["bytes"]
        targets.add(target)
    if total != receipt["total_bytes"] or total > MAX_BYTES:
        raise ValueError("Device preview 静态响应字节汇总无效")
    if "index.html" not in targets or not any(target.endswith(".js") for target in targets):
        raise ValueError("Device preview 没有实际加载入口 HTML 与 JavaScript")
    document.assert_unchanged()
    return {"receipt": receipt, "descriptor": {
        "path": str(document.path), "bytes": len(document.raw), "sha256": document.sha256,
    }}
