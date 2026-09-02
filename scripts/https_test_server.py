#!/usr/bin/env python3
"""为出站连接集成测试提供仅监听回环地址的最小 HTTPS 服务。"""

from __future__ import annotations

import argparse
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


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cert", type=Path, required=True)
    parser.add_argument("--key", type=Path, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, required=True)
    return parser.parse_args()


def create_server(
    cert: Path,
    key: Path,
    host: str,
    port: int,
) -> ThreadingHTTPServer:
    """创建 HTTPS 服务；端口 0 仅供同进程 fixture 原子分配动态端口。"""

    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(cert, key)

    server = ThreadingHTTPServer((host, port), HttpsFixtureHandler)
    server.daemon_threads = True
    server.socket = context.wrap_socket(server.socket, server_side=True)
    return server


def main() -> None:
    args = parse_args()
    if args.host not in {"127.0.0.1", "::1"}:
        raise SystemExit("HTTPS 测试服务只允许监听回环地址")
    if not 1 <= args.port <= 65_535:
        raise SystemExit("HTTPS 测试服务端口必须在 1 到 65535 之间")

    server = create_server(args.cert, args.key, args.host, args.port)
    print(f"ready=https://{args.host}:{args.port}", flush=True)
    try:
        server.serve_forever(poll_interval=0.1)
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
