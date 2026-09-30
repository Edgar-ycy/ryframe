"""只按严格构建收据提供恢复验收的前端生产文件。"""

from __future__ import annotations

import argparse
import hashlib
import mimetypes
import re
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path, PurePosixPath
from urllib.parse import unquote_to_bytes, urlsplit

from restore_frontend_build import FRONTEND_RECEIPT, frontend_snapshots
from restore_runtime_evidence import directory, read_json_document, validate_frontend_receipt


def _decode_path(target: str) -> str:
    raw = urlsplit(target).path
    if re.search(r"%(?![0-9a-fA-F]{2})", raw):
        raise ValueError("前端请求包含无效百分号转义")
    try:
        decoded = unquote_to_bytes(raw).decode("utf-8", errors="strict")
    except UnicodeDecodeError as error:
        raise ValueError("前端请求路径不是 UTF-8") from error
    relative = decoded.removeprefix("/")
    path = PurePosixPath(relative)
    if (
        not decoded.startswith("/")
        or decoded.startswith("//")
        or "\\" in decoded
        or ".." in path.parts
        or (relative and path.as_posix() != relative)
    ):
        raise ValueError("前端请求路径不规范或越界")
    return relative


def load_site(frontend: Path, receipt_path: Path) -> dict[str, bytes]:
    """一次性核验并读取白名单文件，服务期间不跟随新路径或链接。"""
    root = directory(frontend, "前端运行来源")
    dist = directory(root / "dist", "前端生产目录")
    expected_receipt = (dist / FRONTEND_RECEIPT).absolute()
    if receipt_path.absolute() != expected_receipt:
        raise ValueError("前端站点只接受来源目录内的标准构建收据")
    document = read_json_document(expected_receipt)
    receipt = validate_frontend_receipt(document.value)
    files, snapshots = frontend_snapshots(root)
    if receipt["files"] != files or "index.html" not in {item["path"] for item in files}:
        raise ValueError("前端站点文件与构建收据不一致或缺少入口")
    content = {}
    for item, snapshot in zip(files, snapshots, strict=True):
        data = snapshot.path.read_bytes()
        if len(data) != item["bytes"] or hashlib.sha256(data).hexdigest() != item["sha256"]:
            raise ValueError("前端站点文件在读取期间发生变化")
        snapshot.assert_unchanged()
        content[item["path"]] = data
    document.assert_unchanged()
    return content


def _handler(content: dict[str, bytes]):
    class ReceiptHandler(BaseHTTPRequestHandler):
        server_version = "RyFrameRestoreFrontend/1"

        def _respond(self, include_body: bool) -> None:
            try:
                requested = _decode_path(self.path)
            except ValueError:
                self.send_error(400)
                return
            relative = requested or "index.html"
            body = content.get(relative)
            if body is None and "text/html" in self.headers.get("Accept", "") \
                    and not PurePosixPath(relative).suffix:
                relative, body = "index.html", content["index.html"]
            if body is None:
                self.send_error(404)
                return
            media_type = mimetypes.guess_type(relative)[0] or "application/octet-stream"
            if media_type.startswith("text/") or media_type in {"application/javascript", "application/json"}:
                media_type += "; charset=utf-8"
            self.send_response(200)
            self.send_header("Content-Type", media_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("X-Content-Type-Options", "nosniff")
            self.end_headers()
            if include_body:
                self.wfile.write(body)

        def do_GET(self) -> None:
            self._respond(True)

        def do_HEAD(self) -> None:
            self._respond(False)

        def log_message(self, format: str, *args) -> None:
            super().log_message(format, *args)

    return ReceiptHandler


def serve(frontend: Path, receipt: Path, host: str, port: int) -> None:
    if host != "127.0.0.1" or type(port) is not int or not 1 <= port <= 65535:
        raise ValueError("前端恢复站点必须使用明确的本机 IPv4 和有效端口")
    content = load_site(frontend, receipt)
    with ThreadingHTTPServer((host, port), _handler(content)) as server:
        server.daemon_threads = True
        server.serve_forever()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("__serve",))
    parser.add_argument("--frontend", type=Path, required=True)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("--host", required=True)
    parser.add_argument("--port", type=int, required=True)
    args = parser.parse_args()
    serve(args.frontend, args.receipt, args.host, args.port)


if __name__ == "__main__":
    main()
