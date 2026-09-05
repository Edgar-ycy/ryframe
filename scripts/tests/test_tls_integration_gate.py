from __future__ import annotations

import importlib.util
import http.client
import os
import shutil
import socket
import socketserver
import ssl
import sys
import threading
import unittest
import uuid
from contextlib import contextmanager
from pathlib import Path
from unittest import mock


SCRIPT = Path(__file__).resolve().parents[1] / "tls_integration_gate.py"
SPEC = importlib.util.spec_from_file_location("tls_integration_gate", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = MODULE
sys.path.insert(0, str(SCRIPT.parent))
SPEC.loader.exec_module(MODULE)
BACKEND_ROOT = SCRIPT.parents[1]
TEMP_ROOT = BACKEND_ROOT / ".local-tests/python-unit"
TEMP_ROOT.mkdir(parents=True, exist_ok=True)


@contextmanager
def test_directory():
    path = TEMP_ROOT / f"tls-gate-{uuid.uuid4().hex}"
    path.mkdir()
    try:
        yield path
    finally:
        shutil.rmtree(path)


class FakeFixtures:
    redis_port = 41001
    https_port = 41002

    def __init__(self) -> None:
        self.entered = False
        self.stopped = False

    def __enter__(self):
        self.entered = True
        return self

    def __exit__(self, *_: object) -> None:
        self.stopped = True


class RedisPingHandler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        self.request.recv(1024)
        self.request.sendall(b"+PONG\r\n")


class TlsIntegrationGateTests(unittest.TestCase):
    def test_https_fixture_rejects_non_loopback_and_invalid_ports_before_loading_certificates(
        self,
    ) -> None:
        for host, port, message in [
            ("0.0.0.0", 0, "只允许监听回环地址"),
            ("127.0.0.1", -1, "端口必须在 0 到 65535 之间"),
            ("::1", 65_536, "端口必须在 0 到 65535 之间"),
        ]:
            with self.subTest(host=host, port=port), self.assertRaisesRegex(ValueError, message):
                MODULE.create_server(Path("missing.crt"), Path("missing.key"), host, port)

    def test_commands_run_only_the_three_exact_tls_tests(self) -> None:
        commands = [
            MODULE.test_command(spec, Path("target/ci/backend"), 4)
            for spec in MODULE.TESTS
        ]
        self.assertEqual(
            commands[0],
            [
                "cargo",
                "test",
                "--locked",
                "--target-dir",
                str(Path("target/ci/backend")),
                "-p",
                "ryframe-adapters",
                "--features",
                "redis-api",
                "--test",
                "redis_real_protocol",
                "--jobs",
                "4",
                "tls_connection_round_trip_uses_real_redis",
                "--",
                "--exact",
                "--nocapture",
            ],
        )
        self.assertEqual(
            [
                (command[command.index("--test") + 1], command[-4])
                for command in commands[1:]
            ],
            [
                ("s3_https_real", "s3_client_reaches_a_real_https_endpoint"),
                ("otel_https_real", "otlp_exporter_flushes_over_a_real_https_endpoint"),
            ],
        )
        self.assertTrue(all(command[-2:] == ["--exact", "--nocapture"] for command in commands))

    def test_remote_redis_is_rejected_before_any_fixture_starts(self) -> None:
        with mock.patch.dict(
            os.environ,
            {"RYFRAME_REDIS_HOST": "redis.shared.example", "RYFRAME_REDIS_PORT": "6379"},
            clear=True,
        ):
            with self.assertRaisesRegex(MODULE.TlsIntegrationError, "只允许连接回环 Redis"):
                MODULE._redis_endpoint()

    def test_fixture_environment_uses_dynamic_loopback_ports_and_test_ca(self) -> None:
        fixture = FakeFixtures()
        with mock.patch.dict(
            os.environ,
            {
                "RYFRAME_REDIS_TLS_CLIENT_CERT": "ambient-cert",
                "RYFRAME_REDIS_TLS_CLIENT_KEY": "ambient-key",
                "SSL_CERT_DIR": "ambient-roots",
            },
            clear=True,
        ):
            environment = MODULE._test_environment(Path("fixture-ca.pem"), fixture)

        self.assertEqual(environment["RYFRAME_REDIS_HOST"], "127.0.0.1")
        self.assertEqual(environment["RYFRAME_REDIS_PORT"], "41001")
        self.assertEqual(environment["RYFRAME_REDIS_DATABASE"], "15")
        self.assertEqual(environment["RYFRAME_REDIS_TLS_INTEGRATION"], "1")
        self.assertEqual(environment["SSL_CERT_FILE"], "fixture-ca.pem")
        self.assertNotIn("SSL_CERT_DIR", environment)
        self.assertEqual(
            environment["RYFRAME_S3_HTTPS_ENDPOINT"], "https://127.0.0.1:41002"
        )
        self.assertEqual(
            environment["RYFRAME_OTLP_HTTPS_ENDPOINT"],
            "https://127.0.0.1:41002/v1/traces",
        )
        self.assertNotIn("RYFRAME_REDIS_TLS_CLIENT_CERT", environment)
        self.assertNotIn("RYFRAME_REDIS_TLS_CLIENT_KEY", environment)

    def test_failure_runs_all_tests_stops_fixtures_and_preserves_summary(self) -> None:
        with test_directory() as root:
            fixture = FakeFixtures()
            environment = {
                "RYFRAME_REDIS_HOST": "127.0.0.1",
                "RYFRAME_REDIS_PORT": "6379",
                "RYFRAME_INTEGRATION_RUN_ID": "run-42",
            }
            with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(
                MODULE, "_check_redis"
            ), mock.patch.object(
                MODULE,
                "generate_certificates",
                return_value=(root / "ca.pem", root / "server-key.pem"),
            ), mock.patch.object(
                MODULE, "FixtureServers", return_value=fixture
            ), mock.patch.object(
                MODULE, "_run_test", side_effect=[None, "退出码 9", None]
            ) as run_test:
                with self.assertRaisesRegex(MODULE.TlsIntegrationError, "s3-https"):
                    MODULE.run_gate(
                        BACKEND_ROOT,
                        Path("target/ci/backend"),
                        4,
                        60,
                        root / "artifacts",
                    )

            self.assertTrue(fixture.entered)
            self.assertTrue(fixture.stopped)
            self.assertEqual(run_test.call_count, 3)
            output_dirs = list((root / "artifacts").iterdir())
            self.assertEqual(len(output_dirs), 1)
            summary = (output_dirs[0] / "summary.txt").read_text(encoding="utf-8")
            self.assertIn("s3-https: 退出码 9", summary)
            self.assertFalse((output_dirs[0] / "fixture-material").exists())

    def test_existing_artifact_directory_is_never_overwritten(self) -> None:
        with test_directory() as root:
            first = MODULE._reserve_output_dir(root, "same/run")
            (first / "evidence.log").write_text("first", encoding="ascii")
            second = MODULE._reserve_output_dir(root, "same/run")
            self.assertNotEqual(first, second)
            self.assertEqual((first / "evidence.log").read_text(encoding="ascii"), "first")

    def test_generated_ca_protects_both_loopback_fixtures(self) -> None:
        if shutil.which(os.environ.get("RYFRAME_OPENSSL", "openssl")) is None:
            self.skipTest("本机没有 OpenSSL")
        with test_directory() as root:
            ca_cert, server_key = MODULE.generate_certificates(root, root / "openssl.log")
            upstream = socketserver.TCPServer(("127.0.0.1", 0), RedisPingHandler)
            upstream_thread = threading.Thread(target=upstream.serve_forever, daemon=True)
            upstream_thread.start()
            try:
                with MODULE.FixtureServers(
                    root / "server.pem",
                    server_key,
                    ("127.0.0.1", upstream.server_address[1]),
                    root / "fixtures.log",
                ) as fixtures:
                    context = ssl.create_default_context(cafile=str(ca_cert))
                    with socket.create_connection(("127.0.0.1", fixtures.redis_port)) as raw:
                        with context.wrap_socket(raw, server_hostname="127.0.0.1") as tls:
                            tls.sendall(b"*1\r\n$4\r\nPING\r\n")
                            self.assertEqual(tls.recv(64), b"+PONG\r\n")
                    connection = http.client.HTTPSConnection(
                        "127.0.0.1",
                        fixtures.https_port,
                        context=context,
                        timeout=3,
                    )
                    connection.request("GET", "/fixture")
                    response = connection.getresponse()
                    self.assertEqual(response.read(), b"ryframe-https-ok")
                    connection.close()
            finally:
                upstream.shutdown()
                upstream.server_close()
                upstream_thread.join(timeout=3)
            fixture_log = (root / "fixtures.log").read_text(encoding="utf-8")
            self.assertIn("redis_tcp_accept=", fixture_log)
            self.assertIn("fixtures_stopped=1", fixture_log)


class TlsIntegrationPolicyTests(unittest.TestCase):
    def test_xtask_workflow_and_artifact_policy_remain_connected(self) -> None:
        ci_source = (BACKEND_ROOT / "xtask/src/ci.rs").read_text(encoding="utf-8")
        workflow = (BACKEND_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn('"scripts/tls_integration_gate.py"', ci_source)
        integration = workflow.split("\n  integration:\n", 1)[1].split(
            "\n  windows-smoke:\n", 1
        )[0]
        self.assertIn("cargo xtask check ci integration", integration)
        self.assertIn('RYFRAME_MYSQL_TLS_INTEGRATION: "1"', integration)
        self.assertIn("RYFRAME_TLS_ARTIFACT_DIR", integration)
        self.assertIn("tls-integration-${{ github.run_id }}", integration)
        self.assertIn("if: ${{ always() }}", integration)


if __name__ == "__main__":
    unittest.main()
