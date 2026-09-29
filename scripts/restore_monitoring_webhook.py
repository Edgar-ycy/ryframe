"""隔离告警验收的本地 webhook；只接受 loopback Alertmanager 投递。"""

from __future__ import annotations

import argparse
import json
import os
import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from restore_runtime_evidence import decode_object, reject_link_or_reparse

MAX_BODY = 1024 * 1024


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds").replace("+00:00", "Z")


def _append(path: Path, stream, identity: tuple[int, int], lock: threading.Lock, remote: str, payload: dict) -> None:
    value = json.dumps(
        {"received_at": _now(), "remote": remote, "payload": payload},
        ensure_ascii=False,
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8") + b"\n"
    with lock:
        before = path.stat()
        opened = os.fstat(stream.fileno())
        if (before.st_dev, before.st_ino) != identity or (opened.st_dev, opened.st_ino) != identity:
            raise ValueError("监控 webhook 事件文件身份发生变化")
        stream.write(value)
        os.fsync(stream.fileno())
        after = path.stat()
        if (after.st_dev, after.st_ino) != identity:
            raise ValueError("监控 webhook 事件文件在写入期间发生变化")


def handler(events: Path, stream, identity: tuple[int, int], lock: threading.Lock):
    class LocalAlertHandler(BaseHTTPRequestHandler):
        server_version = "RyFrameMonitoringWebhook/1"

        def _response(self, status: int, body: bytes = b"") -> None:
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            if body:
                self.wfile.write(body)

        def do_GET(self) -> None:
            if self.path != "/readyz":
                self._response(404)
                return
            self._response(200, b'{"status":"ready"}')

        def do_POST(self) -> None:
            remote = self.client_address[0]
            if self.path != "/alerts" or remote not in {"127.0.0.1", "::1"}:
                self._response(404)
                return
            if self.headers.get("Content-Type", "").partition(";")[0].strip() != "application/json":
                self._response(415)
                return
            raw_length = self.headers.get("Content-Length", "")
            if not raw_length.isdigit() or not 0 < int(raw_length) <= MAX_BODY:
                self._response(413)
                return
            raw = self.rfile.read(int(raw_length))
            try:
                payload = decode_object(raw, "Alertmanager webhook")
            except ValueError:
                self._response(400)
                return
            try:
                _append(events, stream, identity, lock, remote, payload)
            except (OSError, ValueError):
                self._response(500)
                return
            self._response(204)

        def log_message(self, *_args) -> None:
            return

    return LocalAlertHandler


def server(host: str, port: int, events: Path, stream) -> ThreadingHTTPServer:
    if host not in {"127.0.0.1", "::1"} or type(port) is not int or not 1024 <= port <= 65535:
        raise ValueError("监控 webhook 只能绑定明确 loopback 端口")
    events = events.absolute()
    reject_link_or_reparse(events)
    if not events.is_file() or events.stat().st_size != 0:
        raise ValueError("监控 webhook 要求预先创建的空事件文件")
    metadata = os.fstat(stream.fileno())
    return ThreadingHTTPServer(
        (host, port), handler(events, stream, (metadata.st_dev, metadata.st_ino), threading.Lock())
    )


def main(arguments: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(add_help=False, allow_abbrev=False)
    parser.add_argument("--host", required=True)
    parser.add_argument("--port", required=True, type=int)
    parser.add_argument("--events", required=True, type=Path)
    args = parser.parse_args(arguments)
    with args.events.open("ab", buffering=0) as stream:
        with server(args.host, args.port, args.events, stream) as instance:
            instance.serve_forever(poll_interval=0.1)


if __name__ == "__main__":
    main()
