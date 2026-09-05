"""阶段登记只收集已绑定二进制角色；复用不替代现有请求验证。"""
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import artifact_digests
import devex_clone_run as run


class BinaryBindingTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name)
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.binary = lambda role: {"path": str(self.local / (role + ".exe")), "sha256": "a" * 64}
        self.api = self.build("api", "restore-backend-build", ("api", "worker"))
        self.maintenance = self.build("maintenance", "devex-clone-tool-build", ("reset", "migrate", "tenant-data"))
        self.source = {"tools": {role: self.binary(role) for role in ("aws", "mysql", "mysqldump", "node")},
                       "backend_build": self.api, "maintenance_build": self.maintenance,
                       "business_payload": {"path": str(self.local / "payload.sql"), "sha256": "b" * 64}}
        self.target = {"tools": {role: self.binary(role) for role in ("aws", "mysql")},
                       "maintenance_build": self.maintenance,
                       "storage": {"rustfs": {"identity": {"executable": self.binary("rustfs")["path"]},
                                               "sha256": "a" * 64}, "redis": {"wsl": self.binary("wsl")}}}
        self.value = {"source_request": self.file("source", self.source), "initialized": self.file("initialized", {})}

    def file(self, name, value):
        path = self.local / (name + ".json")
        raw = json.dumps(value).encode()
        path.write_bytes(raw)
        return {"path": str(path), "bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}

    def build(self, name, kind, roles):
        return self.file(name, {"kind": kind, "artifacts": {
            role: {"executable": self.binary(role)["path"], "sha256": "a" * 64} for role in roles}})

    @contextmanager
    def verified_requests(self):
        with patch("devex_clone_source_proof.validate_request") as source, \
                patch.object(run, "initialization_history", return_value=({}, self.target)) as history, \
                patch("devex_clone_target_binding.request_binding") as target:
            yield source, history, target

    def test_registered_roles_deduplicate_to_eleven_binaries_and_exclude_payload(self):
        with self.verified_requests() as (source, history, target):
            values = run.copy_binary_bindings(self.backend, self.value)
        source.assert_called_once_with(self.backend, self.source)
        history.assert_called_once_with(self.backend, Path(self.value["initialized"]["path"]))
        target.assert_called_once_with(self.backend, self.target)
        self.assertEqual(len(values), 16)
        self.assertEqual({Path(item["path"]).stem for item in values}, {
            "aws", "mysql", "mysqldump", "node", "api", "worker", "reset", "migrate", "tenant-data", "rustfs", "wsl"})
        self.assertTrue(all(set(item) == {"path", "sha256"} for item in values))

    def test_wrong_build_kind_missing_role_and_changed_binding_are_rejected(self):
        for kind, roles in (("other", ("api", "worker")), ("restore-backend-build", ("api",))):
            self.source["backend_build"] = self.build("api", kind, roles)
            self.value["source_request"] = self.file("source", self.source)
            with self.verified_requests(), self.assertRaises(ValueError):
                run.copy_binary_bindings(self.backend, self.value)
        Path(self.value["source_request"]["path"]).write_bytes(b"{}")
        with self.verified_requests() as (source, _, _), self.assertRaises(ValueError):
            run.copy_binary_bindings(self.backend, self.value)
        source.assert_not_called()

    def test_original_source_and_initialization_rejections_still_propagate(self):
        with self.verified_requests() as (source, history, _):
            source.side_effect = ValueError("source rejected")
            with self.assertRaisesRegex(ValueError, "source rejected"):
                run.copy_binary_bindings(self.backend, self.value)
            history.assert_not_called()
        with self.verified_requests() as (_, history, target):
            history.side_effect = ValueError("history rejected")
            with self.assertRaisesRegex(ValueError, "history rejected"):
                run.copy_binary_bindings(self.backend, self.value)
            target.assert_not_called()


class ReleaseTests(unittest.TestCase):
    def test_release_attempts_every_handle_even_when_one_close_fails(self):
        closed = []
        class File:
            def __init__(self, value): self.value = value
            def close(self):
                closed.append(self.value)
                if self.value == 2: raise OSError("fixture")
        error = artifact_digests._release([File(1), File(2), File(3)])
        self.assertIsInstance(error, OSError)
        self.assertEqual(closed, [3, 2, 1])


if __name__ == "__main__":
    unittest.main()
