"""原始证据按步骤选择的离线回归；完整阶段仍核验全部登记对象。"""
import copy
from pathlib import Path
import unittest
from unittest.mock import patch

import devex_clone_export_verify as verify
from devex_clone_source import export_source
from devex_clone_source_fixture import SourceFixture
from restore_build import file_digest
from restore_reference_io import ExternalTools


class ProofDependencyTests(unittest.TestCase):
    def setUp(self):
        self.f = SourceFixture(self)
        original = self.f.inventory

        def inventory(command):
            value = original(command)
            entries = next(bucket["entries"] for bucket in value["objects"] if bucket["bucket"] == "uploads")
            entries.append({**entries[0], "key": self.f.scope + "/system/second.txt"})
            return value

        self.f.inventory = inventory
        self.export = export_source(self.f.backend, self.f.request_path, self.f.output, self.f)
        self.bound = {"path": str(self.f.output / "export.json"), **file_digest(self.f.output / "export.json")}
        self.loaded = verify.verify_source_export(self.f.backend, self.bound)
        entries = next(bucket["entries"] for bucket in self.export["objects"] if bucket["bucket"] == "uploads")
        self.a, self.b = [("uploads", item["key"]) for item in entries]
        self.directories = {("uploads", item["key"]): (self.f.output / item["capture"]["file"]).parent for item in entries}

    def check(self, selection):
        verify.verify_export_bindings(self.f.backend, self.loaded, selection=selection)

    def change(self, selected, filename="get.diagnostic.json"):
        (self.directories[selected] / filename).write_text("{}", encoding="utf-8")

    def test_selected_scope_reads_all_current_proofs_and_no_unrelated_object_files(self):
        opened, original = [], Path.open

        def observe(path, *args, **kwargs):
            opened.append(path)
            return original(path, *args, **kwargs)

        with patch.object(Path, "open", observe):
            self.check(self.a)
        expected = {self.directories[self.a] / name for name in verify.capture_proof_names()}
        self.assertTrue(expected <= set(opened))
        self.assertFalse(any(path.is_relative_to(self.directories[self.b]) for path in opened))

    def test_global_scope_reads_no_object_proof_files(self):
        opened, original = [], Path.open

        def observe(path, *args, **kwargs):
            opened.append(path)
            return original(path, *args, **kwargs)

        with patch.object(Path, "open", observe):
            self.check("global")
        self.assertFalse(any(path.is_relative_to(directory) for path in opened for directory in self.directories.values()))

    def test_unused_object_change_is_rejected_before_its_use_or_full_stage_completion(self):
        self.change(self.b)
        self.check("global")
        self.check(self.a)
        for selection in (self.b, "all"):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                self.check(selection)

    def test_current_object_original_proof_change_is_immediately_rejected(self):
        self.change(self.a)
        with self.assertRaises(ValueError):
            self.check(self.a)

    def test_global_change_is_rejected_in_every_scope(self):
        (self.f.output / "schema-before.json").write_text("{}", encoding="utf-8")
        for selection in ("global", self.a, self.b, "all"):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                self.check(selection)

    def test_missing_complete_proof_map_cannot_be_hidden_by_global_scope(self):
        self.loaded["proof_files"].pop(str(self.directories[self.b] / "get.diagnostic.json"))
        with self.assertRaisesRegex(ValueError, "集合不完整"):
            self.check("global")

    def test_scope_must_be_an_exact_known_object(self):
        for selection in (("uploads", "unknown"), [*self.a], "unknown", ("uploads",)):
            with self.subTest(selection=selection), self.assertRaises(ValueError):
                self.check(selection)

    def test_changed_capture_snapshot_is_detected_when_the_object_is_used(self):
        self.loaded["captures"][self.b[0]][self.b[1]]["metadata"]["ContentType"] = "application/json"
        self.check(self.a)
        with self.assertRaisesRegex(ValueError, "快照被修改"):
            self.check(self.b)

    def observation(self, change):
        calls = []

        def run(command, **_kwargs):
            calls.append(command)
            value = self.f.aws(command)
            if "head-object" in command:
                change()
            return value

        tools = ExternalTools({"source": copy.deepcopy(self.f.request["source"]), "tools": self.f.request["tools"]},
                              self.f.local, run)
        result = verify.observe_source_object(self.f.backend, tools, self.loaded, *self.a,
                                               self.f.local / "one-object-observation", environment=self.f.environment)
        self.assertEqual(sum("head-object" in command for command in calls), 1)
        self.assertFalse(any("list-objects-v2" in command for command in calls))
        return result

    def test_current_proof_change_during_head_is_rejected_after_response(self):
        with self.assertRaises(verify.SourceObjectObservationError):
            self.observation(lambda: self.change(self.a))

    def test_unused_proof_change_during_head_does_not_poison_current_observation_but_final_fails(self):
        result = self.observation(lambda: self.change(self.b))
        self.assertEqual(result["remote_writes"], 0)
        with self.assertRaises(ValueError):
            self.check("all")


if __name__ == "__main__":
    unittest.main()
