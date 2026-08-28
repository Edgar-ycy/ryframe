#!/usr/bin/env python3
"""运行 Redis、S3 与 OTLP 的隔离 AWS-LC TLS 出站集成门禁。"""

from __future__ import annotations

import argparse
import os
import re
import select
import shutil
import socket
import socketserver
import ssl
import subprocess
import threading
import traceback
from dataclasses import dataclass
from pathlib import Path
from types import TracebackType
from typing import BinaryIO

from https_test_server import create_server


LOOPBACK_HOST = "127.0.0.1"
LOOPBACK_NAMES = frozenset({"127.0.0.1", "::1", "localhost"})
DEFAULT_REDIS_PORT = 6379
DEFAULT_REDIS_DATABASE = 15
DEFAULT_TIMEOUT_SECONDS = 900
MAX_LOG_TAIL_LINES = 80
RUN_ID_PATTERN = re.compile(r"[^a-zA-Z0-9_.-]+")


class TlsIntegrationError(RuntimeError):
    """TLS 集成 fixture 或测试无法安全完成。"""


@dataclass(frozen=True)
class TestSpec:
    name: str
    target: str
    test_name: str
    features: str | None


TESTS = (
    TestSpec(
        "redis-tls",
        "redis_real_protocol",
        "tls_connection_round_trip_uses_real_redis",
        "redis",
    ),
    TestSpec(
        "s3-https",
        "s3_https_real",
        "s3_client_reaches_a_real_https_endpoint",
        None,
    ),
    TestSpec(
        "otlp-https",
        "otel_https_real",
        "otlp_exporter_flushes_over_a_real_https_endpoint",
        "otel",
    ),
)


class RedisTlsProxyHandler(socketserver.BaseRequestHandler):
    """双向转发一个 TLS Redis 连接，不解析或枚举任何 Redis key。"""

    server: RedisTlsProxyServer

    def handle(self) -> None:
        try:
            with socket.create_connection(self.server.upstream, timeout=5) as upstream:
                self.request.settimeout(None)
                upstream.settimeout(None)
                self._forward(upstream)
        except OSError as error:
            self.server.write_log(f"redis_proxy_error={error}\n")

    def _forward(self, upstream: socket.socket) -> None:
        sockets = (self.request, upstream)
        while True:
            readable, _, _ = select.select(sockets, (), (), 1)
            for source in readable:
                data = source.recv(64 * 1024)
                if not data:
                    return
                destination = upstream if source is self.request else self.request
                destination.sendall(data)


class RedisTlsProxyServer(socketserver.ThreadingTCPServer):
    """只监听 IPv4 回环地址的 Redis TLS 代理。"""

    allow_reuse_address = False
    daemon_threads = True

    def __init__(
        self,
        context: ssl.SSLContext,
        upstream: tuple[str, int],
        log: BinaryIO,
    ) -> None:
        self.upstream = upstream
        self._log = log
        self._log_lock = threading.Lock()
        super().__init__((LOOPBACK_HOST, 0), RedisTlsProxyHandler)
        self.socket = context.wrap_socket(self.socket, server_side=True)

    def write_log(self, message: str) -> None:
        with self._log_lock:
            self._log.write(message.encode("utf-8", errors="replace"))
            self._log.flush()

    def handle_error(
        self,
        request: socket.socket,
        client_address: tuple[str, int],
    ) -> None:
        del request
        self.write_log(
            f"redis_proxy_client_error={client_address!r}\n{traceback.format_exc()}"
        )


class FixtureServers:
    """在上下文退出时始终关闭两个监听端口和工作线程。"""

    def __init__(
        self,
        certificate: Path,
        private_key: Path,
        upstream: tuple[str, int],
        log_path: Path,
    ) -> None:
        self.certificate = certificate
        self.private_key = private_key
        self.upstream = upstream
        self.log_path = log_path
        self.log: BinaryIO | None = None
        self.redis: RedisTlsProxyServer | None = None
        self.https: socketserver.BaseServer | None = None
        self.threads: list[threading.Thread] = []

    def __enter__(self) -> FixtureServers:
        self.log = self.log_path.open("ab")
        try:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.minimum_version = ssl.TLSVersion.TLSv1_2
            context.load_cert_chain(self.certificate, self.private_key)
            self.redis = RedisTlsProxyServer(context, self.upstream, self.log)
            self.https = create_server(
                self.certificate,
                self.private_key,
                LOOPBACK_HOST,
                0,
            )
            for name, server in (("redis-tls", self.redis), ("https", self.https)):
                thread = threading.Thread(
                    target=server.serve_forever,
                    name=f"ryframe-{name}-fixture",
                    daemon=True,
                )
                thread.start()
                self.threads.append(thread)
            self._write_ready_log()
            return self
        except BaseException:
            self._stop()
            raise

    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        exc_traceback: TracebackType | None,
    ) -> None:
        del exc_type, exc_value, exc_traceback
        self._stop()

    @property
    def redis_port(self) -> int:
        assert self.redis is not None
        return int(self.redis.server_address[1])

    @property
    def https_port(self) -> int:
        assert self.https is not None
        return int(self.https.server_address[1])

    def _write_ready_log(self) -> None:
        assert self.log is not None
        self.log.write(
            (
                f"redis_tls_ready={LOOPBACK_HOST}:{self.redis_port}\n"
                f"https_ready={LOOPBACK_HOST}:{self.https_port}\n"
            ).encode("ascii")
        )
        self.log.flush()

    def _stop(self) -> None:
        started = len(self.threads)
        servers = (self.redis, self.https)
        for server in servers[:started]:
            if server is not None:
                server.shutdown()
        for server in servers:
            if server is not None:
                server.server_close()
        for thread in self.threads:
            thread.join(timeout=5)
        if self.log is not None and not self.log.closed:
            self.log.write(b"fixtures_stopped=1\n")
            self.log.close()


def test_command(spec: TestSpec, target_dir: Path, jobs: int) -> list[str]:
    command = [
        "cargo",
        "test",
        "--locked",
        "--target-dir",
        str(target_dir),
        "-p",
        "ryframe-adapters",
    ]
    if spec.features is not None:
        command.extend(("--features", spec.features))
    command.extend(
        (
            "--test",
            spec.target,
            "--jobs",
            str(max(1, jobs)),
            spec.test_name,
            "--",
            "--exact",
            "--nocapture",
        )
    )
    return command


def _write_openssl_config(directory: Path) -> tuple[Path, Path]:
    ca_config = directory / "ca.cnf"
    server_config = directory / "server.cnf"
    ca_config.write_text(
        """[req]
prompt = no
distinguished_name = dn
x509_extensions = v3_ca
[dn]
CN = RyFrame Integration CA
[v3_ca]
basicConstraints = critical, CA:TRUE, pathlen:0
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
""",
        encoding="ascii",
        newline="\n",
    )
    server_config.write_text(
        """[req]
prompt = no
distinguished_name = dn
req_extensions = v3_req
[dn]
CN = localhost
[v3_req]
basicConstraints = critical, CA:FALSE
keyUsage = critical, digitalSignature, keyEncipherment
extendedKeyUsage = serverAuth
subjectAltName = @alt_names
[alt_names]
DNS.1 = localhost
IP.1 = 127.0.0.1
""",
        encoding="ascii",
        newline="\n",
    )
    return ca_config, server_config


def generate_certificates(directory: Path, log_path: Path) -> tuple[Path, Path]:
    """用本机 OpenSSL 生成短期测试 CA 与仅对回环地址有效的服务证书。"""

    openssl = os.environ.get("RYFRAME_OPENSSL", "openssl")
    if shutil.which(openssl) is None:
        raise TlsIntegrationError(
            "未找到 OpenSSL；Windows 可使用 Git for Windows 附带的 openssl，"
            "或通过 RYFRAME_OPENSSL 指定绝对路径"
        )
    ca_config, server_config = _write_openssl_config(directory)
    ca_key = directory / "ca-key.pem"
    ca_cert = directory / "ca.pem"
    server_key = directory / "server-key.pem"
    server_request = directory / "server.csr"
    server_cert = directory / "server.pem"
    commands = (
        [
            openssl,
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-sha256",
            "-nodes",
            "-days",
            "2",
            "-config",
            str(ca_config),
            "-keyout",
            str(ca_key),
            "-out",
            str(ca_cert),
        ],
        [
            openssl,
            "req",
            "-new",
            "-newkey",
            "rsa:2048",
            "-sha256",
            "-nodes",
            "-config",
            str(server_config),
            "-keyout",
            str(server_key),
            "-out",
            str(server_request),
        ],
        [
            openssl,
            "x509",
            "-req",
            "-sha256",
            "-days",
            "2",
            "-in",
            str(server_request),
            "-CA",
            str(ca_cert),
            "-CAkey",
            str(ca_key),
            "-CAcreateserial",
            "-extfile",
            str(server_config),
            "-extensions",
            "v3_req",
            "-out",
            str(server_cert),
        ],
    )
    with log_path.open("ab") as log:
        for command in commands:
            completed = subprocess.run(
                command,
                check=False,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                timeout=30,
            )
            if completed.returncode != 0:
                raise TlsIntegrationError(
                    f"生成 TLS fixture 证书失败，退出码 {completed.returncode}；详见 {log_path}"
                )
    return ca_cert, server_key


def _reserve_output_dir(root: Path, run_id: str) -> Path:
    safe_run_id = RUN_ID_PATTERN.sub("-", run_id).strip("-.") or "local"
    root.mkdir(parents=True, exist_ok=True)
    for sequence in range(100):
        suffix = "" if sequence == 0 else f"-{sequence}"
        candidate = root / f"{safe_run_id}{suffix}"
        try:
            candidate.mkdir()
            return candidate
        except FileExistsError:
            continue
    raise TlsIntegrationError(f"无法在 {root} 创建不覆盖历史日志的运行目录")


def _redis_endpoint() -> tuple[str, int]:
    host = os.environ.get("RYFRAME_REDIS_HOST", LOOPBACK_HOST).strip()
    if host not in LOOPBACK_NAMES:
        raise TlsIntegrationError("TLS 门禁只允许连接回环 Redis，拒绝接触共享远程服务")
    raw_port = os.environ.get("RYFRAME_REDIS_PORT", str(DEFAULT_REDIS_PORT))
    try:
        port = int(raw_port)
    except ValueError as error:
        raise TlsIntegrationError(f"RYFRAME_REDIS_PORT 不是有效端口: {raw_port!r}") from error
    if not 1 <= port <= 65_535:
        raise TlsIntegrationError(f"RYFRAME_REDIS_PORT 超出范围: {port}")
    return host, port


def _check_redis(endpoint: tuple[str, int]) -> None:
    try:
        with socket.create_connection(endpoint, timeout=3) as connection:
            connection.sendall(b"*1\r\n$4\r\nPING\r\n")
            response = connection.recv(256)
    except OSError as error:
        raise TlsIntegrationError(f"回环 Redis {endpoint[0]}:{endpoint[1]} 不可用: {error}") from error
    if not response.startswith((b"+PONG", b"-NOAUTH")):
        raise TlsIntegrationError(f"回环端口未返回预期 Redis PING 响应: {response!r}")


def _test_environment(ca_cert: Path, fixtures: FixtureServers) -> dict[str, str]:
    environment = os.environ.copy()
    environment.update(
        {
            "NO_PROXY": "127.0.0.1,localhost",
            "no_proxy": "127.0.0.1,localhost",
            "RYFRAME_REDIS_HOST": LOOPBACK_HOST,
            "RYFRAME_REDIS_PORT": str(fixtures.redis_port),
            "RYFRAME_REDIS_DATABASE": os.environ.get(
                "RYFRAME_REDIS_DATABASE", str(DEFAULT_REDIS_DATABASE)
            ),
            "RYFRAME_REDIS_TLS": "1",
            "RYFRAME_REDIS_TLS_CA": str(ca_cert),
            "RYFRAME_REDIS_TLS_INTEGRATION": "1",
            "RYFRAME_HTTPS_CA_PEM": str(ca_cert),
            "RYFRAME_S3_HTTPS_INTEGRATION": "1",
            "RYFRAME_S3_HTTPS_ENDPOINT": f"https://{LOOPBACK_HOST}:{fixtures.https_port}",
            "RYFRAME_S3_HTTPS_REGION": "us-east-1",
            "RYFRAME_OTLP_HTTPS_INTEGRATION": "1",
            "RYFRAME_OTLP_HTTPS_ENDPOINT": (
                f"https://{LOOPBACK_HOST}:{fixtures.https_port}/v1/traces"
            ),
        }
    )
    environment.pop("RYFRAME_REDIS_TLS_CLIENT_CERT", None)
    environment.pop("RYFRAME_REDIS_TLS_CLIENT_KEY", None)
    return environment


def _run_test(
    command: list[str],
    environment: dict[str, str],
    log_path: Path,
    timeout_seconds: int,
    backend_root: Path,
) -> str | None:
    with log_path.open("wb") as log:
        log.write((f"command={' '.join(command)}\n").encode("utf-8"))
        log.flush()
        try:
            completed = subprocess.run(
                command,
                check=False,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
                env=environment,
                cwd=backend_root,
                timeout=timeout_seconds,
            )
        except subprocess.TimeoutExpired:
            return f"超时（{timeout_seconds} 秒）"
        except OSError as error:
            return f"无法启动: {error}"
    return None if completed.returncode == 0 else f"退出码 {completed.returncode}"


def _tail(path: Path) -> str:
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    return "\n".join(lines[-MAX_LOG_TAIL_LINES:])


def run_gate(
    backend_root: Path,
    target_dir: Path,
    jobs: int,
    timeout_seconds: int,
    artifact_root: Path,
) -> Path:
    run_id = os.environ.get("RYFRAME_INTEGRATION_RUN_ID", f"local-{os.getpid()}")
    output_dir = _reserve_output_dir(artifact_root, run_id)
    print(f"tls_integration_artifacts={output_dir.resolve()}", flush=True)
    failures: list[str] = []
    material_dir = output_dir / "fixture-material"
    try:
        upstream = _redis_endpoint()
        _check_redis(upstream)
        material_dir.mkdir()
        ca_cert, server_key = generate_certificates(
            material_dir, output_dir / "openssl.log"
        )
        server_cert = material_dir / "server.pem"
        with FixtureServers(
            server_cert,
            server_key,
            upstream,
            output_dir / "fixtures.log",
        ) as fixtures:
            environment = _test_environment(ca_cert, fixtures)
            for spec in TESTS:
                log_path = output_dir / f"{spec.name}.log"
                error = _run_test(
                    test_command(spec, target_dir, jobs),
                    environment,
                    log_path,
                    timeout_seconds,
                    backend_root,
                )
                if error is not None:
                    failures.append(f"{spec.name}: {error}（{log_path}）")
    except (OSError, subprocess.SubprocessError, TlsIntegrationError) as error:
        failures.append(str(error))
    finally:
        if material_dir.exists():
            try:
                shutil.rmtree(material_dir)
            except OSError as error:
                failures.append(
                    f"清理临时 TLS 证书目录失败（{material_dir}）：{error}"
                )

    summary = "\n".join(failures) if failures else "三个 AWS-LC TLS 出站测试全部通过"
    (output_dir / "summary.txt").write_text(summary + "\n", encoding="utf-8", newline="\n")
    if failures:
        for spec in TESTS:
            log_path = output_dir / f"{spec.name}.log"
            if log_path.is_file():
                print(f"--- {spec.name} 最近日志 ---\n{_tail(log_path)}", flush=True)
        raise TlsIntegrationError(summary)
    return output_dir


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--backend-root", type=Path, default=Path.cwd())
    parser.add_argument("--target-dir", type=Path, required=True)
    parser.add_argument("--jobs", type=int, required=True)
    parser.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    parser.add_argument(
        "--artifact-dir",
        type=Path,
        default=Path(
            os.environ.get(
                "RYFRAME_TLS_ARTIFACT_DIR", ".local-tests/integration/tls"
            )
        ),
    )
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs 必须为正整数")
    if args.timeout_seconds < 1:
        parser.error("--timeout-seconds 必须为正整数")
    return args


def main() -> None:
    args = parse_args()
    try:
        run_gate(
            args.backend_root.resolve(),
            args.target_dir,
            args.jobs,
            args.timeout_seconds,
            args.artifact_dir.resolve(),
        )
    except TlsIntegrationError as error:
        raise SystemExit(f"TLS 出站集成门禁失败: {error}") from error


if __name__ == "__main__":
    main()
