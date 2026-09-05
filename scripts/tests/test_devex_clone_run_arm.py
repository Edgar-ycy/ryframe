"""seed_to_arm 统一清单与继承存储的离线约束。"""
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_run as run
from devex_clone_capture import write_json
from devex_clone_run_state import binding


class RunArmTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = tempfile.TemporaryDirectory(dir=base)
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.local = self.backend / ".local-tests"
        self.local.mkdir()
        self.directory = self.local / "run"
        self.directory.mkdir()
        self.value = {
            "format_version": 1,
            "kind": "devex-clone-run",
            "id": "arm-base",
            "source_request": self.file("source-request", {"source": True}),
            "source_export": None,
            "initialized": self.file("initialized", {"initialized": True}),
            "source_environment": self.file("source-environment", {"environment": {}}),
            "target_environment": self.file("target-environment", {"environment": {}}),
            "copy_directory": str(self.local / "copy"),
            "copy_stage": "source_to_seed",
            "build_bridges": [],
        }
        self.target_registration = self.file("target-registration", {"fresh": True})
        self.target_initialized_files = self.file("target-files", {"files": True})
        self.target_storage_run = {
            "path": str(self.local / "storage-run"),
            "manifest": self.file("target-storage-manifest", {"run": True}),
            "storage": {"generation": "rustfs"}, "cache": {"generation": "redis"},
        }

    def file(self, name: str, value: dict) -> dict:
        path = self.local / f"{name}.json"
        write_json(path, value)
        return binding(path)

    def test_seed_to_arm_manifest_requires_published_source_registration(self):
        value = {**self.value, "copy_stage": "seed_to_arm"}
        with self.assertRaises(ValueError):
            run.manifest(self.backend, value)
        value |= {
            "source_registration": self.file("source-registration", {"published": True}),
            "target_registration": self.target_registration,
            "target_initialized_files": self.target_initialized_files,
            "target_storage_run": self.target_storage_run,
        }
        with patch.object(run, "target_lifecycle_binding", return_value={}):
            self.assertEqual(run.manifest(self.backend, value), value)
        with self.assertRaises(ValueError):
            run.manifest(self.backend, {**value, "copy_stage": "source_to_seed"})

    def test_seed_to_arm_forbids_local_source_storage_and_uses_registered_generation(self):
        value = {
            **self.value,
            "copy_stage": "seed_to_arm",
            "source_registration": self.file("source-registration", {"published": True}),
        }
        with patch("devex_clone_storage.current_storage_binding", return_value={"local": True}), \
                patch("devex_clone_seed_source.published_source") as published, \
                self.assertRaises(ValueError):
            run.source_storage_binding(self.backend, self.directory, value)
        published.assert_not_called()

        inherited = {"generation": "seed-storage"}
        source = {
            "registration": {"source_request": value["source_request"]},
            "storage": {"storage": inherited},
        }
        with patch("devex_clone_storage.current_storage_binding", return_value=None), \
                patch("devex_clone_seed_source.published_source", return_value=source) as published:
            self.assertEqual(run.source_storage_binding(self.backend, self.directory, value), inherited)
        published.assert_called_once_with(
            self.backend, value["source_registration"], live_storage=True
        )

        source["registration"]["source_request"] = self.file("other-request", {"other": True})
        with patch("devex_clone_storage.current_storage_binding", return_value=None), \
                patch("devex_clone_seed_source.published_source", return_value=source), \
                self.assertRaises(ValueError):
            run.source_storage_binding(self.backend, self.directory, value)

    def test_explicit_source_to_seed_stage_does_not_require_outer_run_manifest(self):
        directory = self.local / "direct-copy-source"
        with patch("devex_clone_storage.current_storage_binding", return_value=None) as current:
            self.assertIsNone(run.source_storage_binding(
                self.backend, directory, copy_stage="source_to_seed",
            ))
        current.assert_called_once_with(self.backend, directory, "source")


if __name__ == "__main__":
    unittest.main()
