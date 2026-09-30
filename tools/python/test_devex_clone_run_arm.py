"""seed_to_arm 统一清单与继承存储的离线约束。"""
from contextlib import contextmanager, nullcontext
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from workspace_directory import WorkspaceDirectory
import devex_clone_run as run
from devex_clone_capture import write_json
from devex_clone_run_state import binding


class RunArmTests(unittest.TestCase):
    def setUp(self):
        base = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
        base.mkdir(parents=True, exist_ok=True)
        temporary = WorkspaceDirectory(base, "devex-clone-run-arm-")
        self.addCleanup(temporary.cleanup)
        self.backend = temporary.path
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

    def storage_run(self) -> Path:
        directory = self.local / "guarded-storage-run"
        directory.mkdir()
        write_json(directory / "manifest.json", {"run": True})
        write_json(directory / "state.json", {"generation": 1})
        return directory

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

        successor = self.file("review-successor", {"successor": True})
        source_generation = self.file("source-generation", {"published": True})
        successor_value = {**value, "review_successor": successor,
                           "source_export_result": self.file("export-result", {"published": True}),
                           "source_generation": source_generation}
        with patch.object(run, "_published_seed_source", return_value={}), \
                patch.object(run, "target_lifecycle_binding", return_value={}):
            self.assertEqual(
                run.manifest(self.backend, successor_value), successor_value
            )

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
            "source_request": value["source_request"],
            "storage": {"storage": inherited},
        }
        with patch("devex_clone_storage.current_storage_binding", return_value=None), \
                patch("devex_clone_seed_source.published_source", return_value=source) as published:
            self.assertEqual(run.source_storage_binding(self.backend, self.directory, value), inherited)
        published.assert_called_once_with(
            self.backend, value["source_registration"], live_storage=True
        )

        source["source_request"] = self.file("other-request", {"other": True})
        with patch("devex_clone_storage.current_storage_binding", return_value=None), \
                patch("devex_clone_seed_source.published_source", return_value=source), \
                self.assertRaises(ValueError):
            run.source_storage_binding(self.backend, self.directory, value)

    def test_successor_seed_to_arm_routes_through_dedicated_published_source(self):
        source_registration = self.file("source-registration", {"published": True})
        review_successor = self.file("review-successor", {"successor": True})
        source_generation = self.file("source-generation", {"published": True})
        value = {
            **self.value,
            "copy_stage": "seed_to_arm",
            "source_registration": source_registration,
            "review_successor": review_successor,
            "source_export_result": self.file("export-result", {"published": True}),
            "source_generation": source_generation,
        }
        inherited = {"generation": "seed-storage"}
        source = {
            "registration": {"source_request": value["source_request"]},
            "source_request": value["source_request"],
            "storage": {"storage": inherited},
            "review_successor": {"source_result": source_registration},
            "review_successor_binding": review_successor,
            "source_generation": source_generation,
        }
        with (
            patch("devex_clone_storage.current_storage_binding", return_value=None),
            patch("devex_clone_seed_export.require_export_binding"),
            patch(
                "reference_fixture_successor.published_source", return_value=source
            ) as published,
        ):
            self.assertEqual(
                run.source_storage_binding(self.backend, self.directory, value),
                inherited,
            )
        published.assert_called_once_with(
            self.backend, review_successor, live_storage=True
        )

        source["review_successor"]["source_result"] = self.file(
            "other-source", {"other": True}
        )
        with (
            patch("devex_clone_storage.current_storage_binding", return_value=None),
            patch(
                "reference_fixture_successor.published_source", return_value=source
            ),
            self.assertRaisesRegex(ValueError, "successor 与 C52"),
        ):
            run.source_storage_binding(self.backend, self.directory, value)

    def test_explicit_source_to_seed_stage_does_not_require_outer_run_manifest(self):
        directory = self.local / "direct-copy-source"
        with patch("devex_clone_storage.current_storage_binding", return_value=None) as current:
            self.assertIsNone(run.source_storage_binding(
                self.backend, directory, copy_stage="source_to_seed",
            ))
        current.assert_called_once_with(self.backend, directory, "source")

    def test_source_to_seed_storage_control_keeps_current_run_without_extra_guard(self):
        with (
            patch.object(run, "target_storage_run", return_value=None),
            patch.object(run, "_target_storage_snapshot") as snapshot,
            patch.object(run, "run_lock") as lock,
            run.target_storage_control(self.backend, self.directory, self.value) as selected,
        ):
            self.assertEqual(selected, self.directory)
        snapshot.assert_not_called()
        lock.assert_not_called()

    def test_ordinary_seed_to_arm_storage_control_keeps_valid_external_run(self):
        storage = self.storage_run()
        value = {**self.value, "copy_stage": "seed_to_arm",
                 "target_storage_run": self.target_storage_run}
        with patch.object(run, "target_storage_run", return_value=storage):
            with run.target_storage_control(self.backend, self.directory, value) as selected:
                self.assertEqual(selected, storage)
        self.assertFalse((storage / "run.lock").exists())

    def test_target_storage_snapshot_distinguishes_same_content_directory_replacement(self):
        storage = self.storage_run()
        original = run._target_storage_snapshot(storage)
        replaced = storage.with_name("replaced-storage-run")
        storage.rename(replaced)
        storage.mkdir()
        write_json(storage / "state.json", {"generation": 1})
        current = run._target_storage_snapshot(storage)
        self.assertEqual(current[1], original[1])
        self.assertNotEqual(current[0], original[0])

    def test_target_storage_control_rejects_state_change_while_acquiring_lock(self):
        storage = self.storage_run()
        value = {**self.value, "copy_stage": "seed_to_arm",
                 "target_storage_run": self.target_storage_run,
                 "review_successor": {"path": "successor.json"}}

        @contextmanager
        def changed_before_lock(_directory):
            (storage / "state.json").write_text('{"generation": 2}\n', encoding="utf-8")
            yield

        with (
            patch.object(run, "target_storage_run", return_value=storage),
            patch.object(run, "run_lock", changed_before_lock),
            self.assertRaisesRegex(ValueError, "取得控制锁前变化"),
        ):
            with run.target_storage_control(self.backend, self.directory, value):
                self.fail("锁前变化后不能进入发布区")

    def test_target_storage_control_rejects_state_change_after_publication(self):
        storage = self.storage_run()
        value = {**self.value, "copy_stage": "seed_to_arm",
                 "target_storage_run": self.target_storage_run,
                 "review_successor": {"path": "successor.json"}}
        with (
            patch.object(run, "target_storage_run", return_value=storage),
            self.assertRaisesRegex(ValueError, "阶段期间变化"),
        ):
            with run.target_storage_control(self.backend, self.directory, value):
                (storage / "state.json").write_text('{"generation": 2}\n', encoding="utf-8")

    def test_target_storage_control_rejects_directory_identity_change_inside_lock(self):
        storage = self.storage_run()
        state = binding(storage / "state.json")
        stable = ((1, 2), state)
        replaced = ((1, 3), state)
        snapshots = [stable, stable, stable, stable, replaced]
        value = {**self.value, "copy_stage": "seed_to_arm",
                 "target_storage_run": self.target_storage_run,
                 "review_successor": {"path": "successor.json"}}
        with (
            patch.object(run, "target_storage_run", return_value=storage),
            patch.object(run, "_target_storage_snapshot", side_effect=snapshots),
            patch.object(run, "run_lock", return_value=nullcontext()),
            self.assertRaisesRegex(ValueError, "阶段期间变化"),
        ):
            with run.target_storage_control(self.backend, self.directory, value):
                pass


if __name__ == "__main__":
    unittest.main()
