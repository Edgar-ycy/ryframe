"""保留来源登记的 Python registry 桥接与 CLI 边界；写入职责由原生 Runner 承担。"""
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import devex_clone_run_cli as cli
import devex_clone_source_proof as source_proof


class SeedSourceTests(unittest.TestCase):
    def setUp(self):
        backend = Path.cwd().resolve()
        self.identities = [
            {"pid": 101, "started": "1001", "executable": str(backend / "api.exe")},
            {"pid": 102, "started": "1002", "executable": str(backend / "worker.exe")},
            {"pid": 101, "started": "1003", "executable": str(backend / "node.exe")},
        ]

    def test_source_proof_accepts_distinct_generations_of_same_pid(self):
        request = {"source": {"scope_id": "seed-scope"},
                   "runtime": {"sha256": "a" * 64}}
        registry = {"format_version": 1, "scope_id": "seed-scope",
                    "runtime_sha256": "a" * 64,
                    "processes": [{"name": name, "identity": identity}
                                  for name, identity in zip(
                                      ("api", "worker", "history"), self.identities)]}
        processes = {"api": self.identities[0], "worker": self.identities[1]}
        with patch.object(source_proof, "require_recorded_producer_stopped") as stopped:
            self.assertEqual(source_proof.producer_identities(
                request, registry, processes, run="fixed-runner"),
                sorted(registry["processes"], key=lambda item: item["name"]))
        self.assertEqual(stopped.call_count, 3)
        registry["processes"][2]["identity"] = self.identities[0]
        with self.assertRaisesRegex(ValueError, "重复"):
            source_proof.producer_identities(request, registry, processes, run="fixed-runner")


class SourceRegisterDispatchTests(unittest.TestCase):
    def test_cli_requires_write_and_routes_without_request(self):
        backend = Path.cwd().resolve()
        directory = backend / ".local-tests/seed-source-cli"
        args = SimpleNamespace(command="seed-runtime", run_dir=directory,
                               operation="source-register", request=None,
                               producer_binding=None, write=False)
        with patch.object(cli, "execute") as execute, self.assertRaisesRegex(ValueError, "显式 --write"):
            cli.dispatch(args, backend)
        execute.assert_not_called()
        outer = {"status": "stage_finished", "stage": "seed-runtime", "mode": "source-register",
                 "attempt": 9, "restore_qualified": False}
        with patch.object(cli, "execute", return_value=outer) as execute:
            args.write = True
            self.assertEqual(cli.dispatch(args, backend), outer)
        execute.assert_called_once_with(backend, directory, "seed-runtime", "source-register",
                                        seed_request=None, producer_binding=None)


if __name__ == "__main__":
    unittest.main()
