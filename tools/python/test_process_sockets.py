import os
import socket
import sys
import unittest
from unittest.mock import patch
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import process_sockets
from process_sockets import endpoint, verify_listener, verify_windows_port_idle


class ProcessSocketTests(unittest.TestCase):
    def test_actual_listener_is_bound_to_its_process_and_closes(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            url = f"http://127.0.0.1:{listener.getsockname()[1]}/readyz"
            verify_listener(os.getpid(), url)
            with self.assertRaises(ValueError):
                verify_listener(os.getppid(), url)
        with self.assertRaises(ValueError):
            verify_listener(os.getpid(), url)

    @unittest.skipUnless(socket.has_ipv6, "当前平台未提供 IPv6")
    def test_actual_ipv6_listener_has_the_same_process_binding(self):
        with socket.socket(socket.AF_INET6) as listener:
            listener.bind(("::1", 0))
            listener.listen()
            verify_listener(os.getpid(), f"http://[::1]:{listener.getsockname()[1]}/readyz")

    def test_remote_ambiguous_or_credentialed_urls_are_rejected(self):
        for url in ("http://localhost:8080/readyz", "http://192.0.2.1/readyz", "file:///tmp/x",
                    "http://user@127.0.0.1/readyz", "http://127.0.0.1/readyz?secret=value"):
            with self.subTest(url=url), self.assertRaises(ValueError):
                endpoint(url)

    @unittest.skipUnless(os.name == "nt", "Windows 原生监听表验收")
    def test_actual_windows_listener_must_be_closed_before_idle_proof(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            url = f"http://127.0.0.1:{listener.getsockname()[1]}"
            with self.assertRaises(ValueError):
                verify_windows_port_idle(url)
        verify_windows_port_idle(url)

    @unittest.skipUnless(os.name == "nt" and socket.has_ipv6, "Windows IPv6 监听表验收")
    def test_ipv6_wildcard_is_conservatively_rejected_for_ipv4(self):
        with socket.socket(socket.AF_INET6) as listener:
            listener.bind(("::", 0))
            listener.listen()
            port = listener.getsockname()[1]
            with self.assertRaises(ValueError):
                verify_windows_port_idle(f"http://127.0.0.1:{port}")

    @unittest.skipUnless(os.name == "nt" and socket.has_ipv6, "Windows IPv4-mapped 监听表验收")
    def test_mapped_ipv6_listener_reachable_from_ipv4_is_not_idle(self):
        with socket.socket(socket.AF_INET6) as listener:
            listener.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
            listener.bind(("::ffff:127.0.0.1", 0))
            listener.listen()
            port = listener.getsockname()[1]
            with socket.create_connection(("127.0.0.1", port), timeout=2):
                with self.assertRaises(ValueError):
                    verify_windows_port_idle(f"http://127.0.0.1:{port}")

    @unittest.skipUnless(os.name == "nt", "Windows 原生监听表验收")
    def test_kernel_observation_error_is_not_absence(self):
        for error in (PermissionError, OSError, ValueError):
            with patch.object(process_sockets, "_windows_listener", side_effect=error), self.assertRaises(error):
                verify_windows_port_idle("http://127.0.0.1:18210")


if __name__ == "__main__":
    unittest.main()
