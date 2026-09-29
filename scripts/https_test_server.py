"""为出站连接集成测试提供仅监听回环地址的最小 HTTPS 服务。"""

from __future__ import annotations

import ssl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path


MAX_REQUEST_BYTES = 16 * 1024 * 1024


class HttpsFixtureHandler(BaseHTTPRequestHandler):
    """接受 S3 GET/HEAD 与 OTLP POST，并返回确定性的空成功响应。"""

    protocol_version = "HTTP/1.1"

    def do_HEAD(self) -> None:  # noqa: N802
        self._respond(b"")

    def do_GET(self) -> None:  # noqa: N802
        self._respond(b"ryframe-https-ok")

    def do_POST(self) -> None:  # noqa: N802
        raw_length = self.headers.get("Content-Length", "0")
        try:
            length = int(raw_length)
        except ValueError:
            self.send_error(400)
            return
        if length < 0 or length > MAX_REQUEST_BYTES:
            self.send_error(413)
            return
        if length:
            self.rfile.read(length)
        self._respond(b"")

    def _respond(self, body: bytes) -> None:
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        if body and self.command != "HEAD":
            self.wfile.write(body)

    def log_message(self, _format: str, *_args: object) -> None:
        return


def create_server(
    cert: Path,
    key: Path,
    host: str,
    port: int,
) -> ThreadingHTTPServer:
    """创建 HTTPS 服务；端口 0 仅供同进程 fixture 原子分配动态端口。"""

    if host not in {"127.0.0.1", "::1"}:
        raise ValueError("HTTPS 测试服务只允许监听回环地址")
    if not 0 <= port <= 65_535:
        raise ValueError("HTTPS 测试服务端口必须在 0 到 65535 之间")

    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(cert, key)

    server = ThreadingHTTPServer((host, port), HttpsFixtureHandler)
    server.daemon_threads = True
    server.socket = context.wrap_socket(server.socket, server_side=True)
    return server
