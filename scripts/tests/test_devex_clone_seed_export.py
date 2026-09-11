"""seed 单次导出、明确失败采用及双侧共享发布的离线回归。"""
from contextlib import ExitStack, nullcontext
import argparse
import copy
from pathlib import Path
import unittest
from unittest.mock import patch

from workspace_directory import WorkspaceDirectory
import devex_clone_seed_export as export
import devex_clone_export_recovery as recovery
import devex_clone_run as run
import devex_clone_run_state as state
from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import Environments
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

    def capture(self, backend, request, output):
        self.assertEqual(backend, self.backend)
        self.assertEqual(request, Path(self.request["path"]))
        output.mkdir()
        write_json(output / "export.json", self.export_value)

    def context(self):
        stack = ExitStack()
        stack.enter_context(patch.object(run, "_require_owned_run"))
        stack.enter_context(patch.object(export, "_source", return_value=copy.deepcopy(self.source)))
        stack.enter_context(patch.object(export, "artifact_sources", return_value=nullcontext()))
        stack.enter_context(patch.object(export, "verify_source_export", side_effect=lambda _, value: self.verified(value)))
        return stack

    def complete(self, *, failed=False):
        number = self.start("source-export")
        with self.context(), patch.object(export, "export_source", side_effect=self.capture) as capture:
            result = export.execute_export(self.backend, self.directory, number)
        capture.assert_called_once()
        state.finish(self.directory, number, result=result, error=RuntimeError("outer publish") if failed else None)
        return number, result, state.binding(self.directory / f"results/{number:04d}.json")

    def test_single_source_export_publishes_full_seal_and_cannot_run_twice(self):
        _, result, descriptor = self.complete()
        self.assertEqual(result["source_rebind"], self.rebound)
        self.assertEqual(result["source_registration"], self.registration)
        self.assertEqual(result["remote_writes"], 0)
        self.assertTrue((self.directory / "export-0001.verified.json").is_file())
        self.assertEqual(export.published_export(self.backend, descriptor, self.source), result)
        before = state.load_state(self.directory)
        with self.assertRaises(ValueError):
            export.preflight(self.directory, "source-export")
        self.assertEqual(before, state.load_state(self.directory))

    def test_both_arms_bind_one_export_and_each_export_stage_only_verifies(self):
        _, result, descriptor = self.complete()
        for side in ("base", "candidate"):
            value = {"id": side, "source_export": result["export"], "source_export_result": descriptor,
                     "source_request": self.request}
            export.require_export_binding(self.backend, self.source, value)
            with patch.object(run, "source_storage_binding", return_value=self.storage), \
                    patch("devex_clone_source.export_source") as capture, \
                    patch("devex_clone_export_verify.verify_source_export", side_effect=lambda _, item: self.verified(item)) as verify:
                observed = run.run_export(self.backend, self.directory, value, Environments({}, {}), 4, "run", self.sources)
            self.assertEqual(observed["export"], result["export"])
            capture.assert_not_called()
            verify.assert_called_once_with(self.backend, result["export"])
        with self.assertRaises(ValueError):
            export.require_export_binding(self.backend, self.source, {"source_export_result": descriptor, "source_export": self.request})

    def test_complete_failed_attempt_is_reconciled_without_reexport(self):
        _, original, _ = self.complete(failed=True)
        export.preflight(self.directory, "source-export-reconcile")
        number = self.start("source-export-reconcile")
        with self.context(), patch.object(export, "export_source") as capture, \
                patch("devex_clone_export_verify.verify_source_export", side_effect=lambda _, item: self.verified(item)), \
                patch("devex_clone_storage.current_storage_binding", return_value=self.storage) as storage, \
                patch.object(recovery, "verify_generation", return_value=self.generation):
            adopted = export.execute_export(self.backend, self.directory, number, reconcile=True)
        capture.assert_not_called()
        self.assertEqual([call.args[2] for call in storage.call_args_list], ["target", "target"])
        self.assertEqual(adopted, original)
        state.finish(self.directory, number, result=adopted)
        descriptor = state.binding(self.directory / f"results/{number:04d}.json")
        self.assertEqual(export.published_export(self.backend, descriptor, self.source), adopted)
        history = state.load_state(self.directory)
        self.assertTrue(export.reconciles_failed_export(self.directory, history, history["attempts"][0], None, self.registration))
        self.assertFalse(export.reconciles_failed_export(self.directory, history, history["attempts"][0], None, self.request))
        with self.assertRaises(ValueError):
            export.preflight(self.directory, "source-export-reconcile")

    def test_partial_failed_attempt_never_reexports_or_adopts_missing_candidate(self):
        number = self.start("source-export")
        state.finish(self.directory, number, error=RuntimeError("partial"))
        for mode in export.MODES:
            with self.subTest(mode=mode), self.assertRaises(ValueError):
                export.preflight(self.directory, mode)
        self.assertEqual(len(state.load_state(self.directory)["attempts"]), 1)

    def test_export_generation_or_source_change_leaves_unpublished_evidence(self):
        number = self.start("source-export")
        changed = {**self.source, "source_rebind": self.successor}
        with self.context(), patch.object(export, "export_source", side_effect=self.capture), \
                patch.object(export, "_source", side_effect=[self.source, changed]), self.assertRaises(ValueError):
            export.execute_export(self.backend, self.directory, number)
        self.assertTrue((self.directory / "e0001/export.json").is_file())
        self.assertTrue((self.directory / "export-0001.verified.json").is_file())
        self.assertFalse((self.directory / "results/0001.json").exists())
        state.finish(self.directory, number, error=RuntimeError("source changed"))

    def test_outer_publication_and_export_content_cannot_drift(self):
        _, result, descriptor = self.complete()
        for field in ("source_registration", "source_rebind", "review_successor", "source_request", "source_storage"):
            changed = copy.deepcopy(self.source)
            if field == "source_registration":
                changed["review_successor"]["source_result"] = self.request
            elif field == "review_successor":
                changed["review_successor_binding"] = self.request
            elif field == "source_request":
                changed["registration"]["source_request"] = self.successor
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
        seal.write_text('{"unexpected":"seal"}', encoding="utf-8")
        with self.assertRaises(ValueError):
            export.published_export(self.backend, descriptor, self.source)

    def test_reconcile_rejects_changed_storage_and_generation_without_exporter(self):
        self.complete(failed=True)
        number = self.start("source-export-reconcile")
        for storage, generation in (({"generation": "replaced"}, self.generation),
                                    (self.storage, {"physical": "changed"})):
            with self.subTest(storage=storage, generation=generation), self.context(), \
                    patch.object(export, "export_source") as capture, \
                    patch("devex_clone_export_verify.verify_source_export", side_effect=lambda _, item: self.verified(item)), \
                    patch("devex_clone_storage.current_storage_binding", return_value=storage), \
                    patch.object(recovery, "verify_generation", return_value=generation), self.assertRaises(ValueError):
                export.execute_export(self.backend, self.directory, number, reconcile=True)
            capture.assert_not_called()
        state.finish(self.directory, number, error=RuntimeError("mismatched source"))

    def test_cli_export_modes_require_write_and_reject_unneeded_request(self):
        import devex_clone_run_cli as cli
        import devex_clone_seed_runtime as runtime

        parser = argparse.ArgumentParser()
        cli.add_commands(parser.add_subparsers(dest="command", required=True))
        for mode in sorted(export.MODES):
            args = ["seed-runtime", "--backend-dir", str(self.backend), "--run-dir", str(self.directory), "--operation", mode]
            with patch.object(cli, "execute") as execute:
                for extra in ([], ["--request", self.successor["path"], "--write"]):
                    with self.subTest(mode=mode, extra=extra), self.assertRaises(ValueError):
                        cli.dispatch(parser.parse_args(args + extra), self.backend)
                execute.assert_not_called()
            completed = {"status": "stage_finished", "stage": "seed-runtime", "mode": mode,
                         "attempt": 3, "restore_qualified": False}
            with patch.object(cli, "execute", return_value=completed) as execute:
                self.assertEqual(cli.dispatch(parser.parse_args(args + ["--write"]), self.backend), completed)
            self.assertIsNone(execute.call_args.kwargs["seed_request"])
            with patch.object(runtime, "require_quiet"), patch.object(export, "execute_export", return_value=completed) as stage:
                self.assertEqual(runtime.execute_seed(self.backend, self.directory, {}, mode, 3), completed)
            stage.assert_called_once_with(self.backend, self.directory, 3, reconcile=mode.endswith("-reconcile"))


if __name__ == "__main__":
    unittest.main()
