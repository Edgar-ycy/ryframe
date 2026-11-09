"""用原格式冻结证据核对仍被 ARM 消费的共享导出读取。"""
import copy
from pathlib import Path
from types import SimpleNamespace
import unittest

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_export as export
import devex_clone_export_recovery as recovery
import devex_clone_run as run
import devex_clone_run_state as state
from devex_clone_capture import read_json, write_json
from restore_reference_plan import plan_hash


class SeedExportTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/tmp", prefix="seed-export-")
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        write_json(self.directory / "manifest.json", {"copy_stage": "source_to_seed"})
        state.initialize_state(self.directory)
        self.request = self.file("source-request.json", {"kind": "source-fixture"})
        self.registration = self.file("C52.json", {"frozen": "registration"})
        self.rebound = self.file("rebind.json", {"frozen": "generation"})
        self.successor = self.file("successor.json", {"frozen": "relationship"})
        self.storage = {"generation": "same-rustfs", "request": "frozen"}
        self.sources = {"fingerprints": {"product": {"sha256": "a" * 64, "files": 7}}, "tools": "same"}
        self.generation = {"physical": "same-source"}
        self.source = {
            "directory": self.directory, "registration": {"source_request": self.request},
            "source_request": self.request, "source_generation": self.file("generation.json", {"v2": True}),
            "source_rebind": self.rebound, "review_successor": {"source_result": self.registration},
            "review_successor_binding": self.successor, "storage": {"storage": self.storage},
            "environment": {"environment": {}}, "manifest": {"build_bridges": []},
            "request": read_json(Path(self.request["path"])),
        }
        self.export_value = {"request": self.request, "logical_inventory_sha256": "c" * 64,
                             "databases": [{"key": "control"}], "objects": [{"entries": []}]}

    def file(self, name, value):
        path = self.directory / name
        write_json(path, value)
        return state.binding(path)

    def start(self, mode):
        number = state.begin(self.directory, "seed-runtime", mode, self.sources)
        attempt = state.load_state(self.directory)["attempts"][-1]
        self.file(f"controller-{number:04d}.json", {"format_version": 1, "kind": "devex-stage-controller",
                  "owner": {"directory": str(self.directory), "manifest_sha256": state.binding(self.directory / "manifest.json")["sha256"]},
                  "attempt": number, "attempt_sha256": plan_hash(attempt)})
        return number

    def verified(self, descriptor):
        return {"export": read_json(Path(descriptor["path"])), "binding": descriptor,
                "request": self.source["request"], "generation": self.generation, "proof_files": {}}

    def complete(self, *, failed=False, sealed=True):
        number = self.start("source-export")
        output = recovery.prepare_attempt(self.backend, self.directory, {"source_request": self.request},
                                          number, self.sources, self.storage, seed=True)
        output.mkdir()
        write_json(output / "export.json", self.export_value)
        exported = state.binding(output / "export.json")
        intent_file = self.directory / f"export-{number:04d}.intent.json"
        verified = self.verified(exported)
        if sealed:
            recovery.record_verified(self.directory, number, read_json(intent_file), exported, verified)
        result = {
            "status": "seed_source_export_published", "origin_attempt": number,
            "origin_intent": state.binding(intent_file), "source_registration": self.registration,
            "source_rebind": self.rebound, "review_successor": self.successor,
            "source_generation": self.source["source_generation"], "source_request": self.request,
            "source_storage": self.storage, "export": exported,
            "summary": recovery.complete_summary(verified), "remote_writes": 0,
            "restore_qualified": False,
        }
        state.finish(self.directory, number, result=result, error=RuntimeError("outer publish") if failed else None)
        return number, result, state.binding(self.directory / f"results/{number:04d}.json")

    def test_both_arms_bind_one_export_and_reject_wrong_binding(self):
        _, result, descriptor = self.complete()
        for side in ("base", "candidate"):
            value = {"id": side, "source_export": result["export"], "source_export_result": descriptor,
                     "source_request": self.request}
            export.require_export_binding(self.backend, self.source, value)
        with self.assertRaises(ValueError):
            export.require_export_binding(self.backend, self.source, {"source_export_result": descriptor, "source_export": self.request})

    def test_readers_accept_only_identical_adoption_of_original_failure(self):
        original, result, _ = self.complete(failed=True, sealed=False)
        first = self.start("source-export-reconcile")
        state.finish(self.directory, first, result=result, error=RuntimeError("outer publish"))
        second = self.start("source-export-reconcile")
        state.finish(self.directory, second, result=result)
        descriptor = state.binding(self.directory / f"results/{second:04d}.json")
        self.assertEqual(export.published_export(self.backend, descriptor, self.source), result)
        history = state.load_state(self.directory)
        self.assertTrue(export.reconciles_failed_export(
            self.directory, history, history["attempts"][0], None, self.registration))
        self.assertFalse(export.reconciles_failed_export(
            self.directory, history, history["attempts"][0], None, self.request))
        self.assertFalse((self.directory / f"export-{original:04d}.verified.json").exists())

    def test_outer_publication_and_export_content_cannot_drift(self):
        _, result, descriptor = self.complete()
        for field in ("source_registration", "source_rebind", "review_successor", "source_request", "source_storage"):
            changed = copy.deepcopy(self.source)
            if field == "source_registration":
                changed["review_successor"]["source_result"] = self.request
            elif field == "review_successor":
                changed["review_successor_binding"] = self.request
            elif field == "source_request":
                changed["source_request"] = self.successor
            elif field == "source_storage":
                changed["storage"]["storage"] = {"generation": "replaced"}
            else:
                changed[field] = self.request
            with self.subTest(field=field), self.assertRaises(ValueError):
                export.published_export(self.backend, descriptor, changed)
        Path(result["export"]["path"]).write_text('{"tampered":true}', encoding="utf-8")
        with self.assertRaises(ValueError):
            export.published_export(self.backend, descriptor, self.source)

    def test_unpublished_and_wrong_result_descriptors_cannot_arm(self):
        _, _, descriptor = self.complete(failed=True)
        for value in (descriptor, self.registration):
            with self.subTest(value=value), self.assertRaises(ValueError):
                export.published_export(self.backend, value, self.source)

    def test_missing_or_tampered_verification_seal_cannot_arm(self):
        _, _, descriptor = self.complete()
        seal = self.directory / "export-0001.verified.json"
        seal.unlink()
        with self.assertRaises(ValueError):
            export.published_export(self.backend, descriptor, self.source)
        seal.write_text('{"unexpected":"seal"}', encoding="utf-8")
        with self.assertRaises(ValueError):
            export.published_export(self.backend, descriptor, self.source)

    def test_shared_export_modes_are_no_longer_accepted_by_python_dispatch(self):
        import devex_clone_run_cli as cli

        for mode in sorted(export.MODES):
            args = SimpleNamespace(command="seed-runtime", run_dir=self.directory,
                                   operation=mode, request=None, producer_binding=None, write=True)
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                cli.dispatch(args, self.backend)


if __name__ == "__main__":
    unittest.main()
