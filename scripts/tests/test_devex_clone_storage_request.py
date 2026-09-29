"""存储请求与两侧原始事实的边界，全部使用隔离文件和进程观察替身。"""
import copy
from pathlib import Path
import unittest
from unittest.mock import patch

import devex_clone_storage_request as model
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding
from full_stack_process import write_receipt
from restore_reference_plan import plan_hash
import test_devex_clone_storage as fixtures


class RequestTests(unittest.TestCase):
    setUp = fixtures.StorageTests.setUp
    file = fixtures.StorageTests.file
    json = fixtures.StorageTests.json

    def target_request(self):
        request = copy.deepcopy(self.request)
        request["side"] = "target"
        previous = request["previous"]
        old = read_json(Path(previous["process_receipt"]["path"]))
        old["lifecycle"] = "running"
        process = self.json("target-process.json", old)
        launch = self.json("target-launch.json", {"format_version": 1, "scope_id": request["scope_id"], "identity": self.old,
            "arguments": model.arguments(request), "environment": model.configuration(request), "credential_files": self.credentials})
        previous.update(process_receipt=process, ready_receipt=process, launch_receipt=launch)
        review = {"services": {"rustfs": {"scope_id": request["scope_id"], "data_dir": str(self.data),
                   "api": request["api_url"], "console": request["console_url"]}}}
        review_file = self.json("target-review.json", review)
        target = {"review": {**review_file, "canonical_sha256": plan_hash(review)},
                  "target": {"s3": {"endpoint": request["api_url"], "access_key_env": "KEY", "secret_key_env": "SECRET"}},
                  "storage": {"rustfs": {"identity": self.old, "sha256": request["executable"]["sha256"],
                                         "process_receipt": process, "launch_receipt": launch}}}
        target_file = self.json("target.json", target)
        initialized = self.root / "initial"
        initialized.mkdir()
        write_json(initialized / "request.json", target)
        write_json(initialized / "prepare.json", {"request": target_file, "request_sha256": plan_hash(target)})
        write_json(initialized / "initialized.json", {"status": "fresh_target_initialized",
                                                      "prepare_sha256": binding(initialized / "prepare.json")["sha256"]})
        self.value["initialized"] = binding(initialized / "initialized.json")
        write_receipt(self.directory / "manifest.json", self.value)
        request.update(manifest=binding(self.directory / "manifest.json"), original=self.value["initialized"])
        return request

    def test_target_accepts_only_original_init_storage_and_exact_review(self):
        request = self.target_request()
        self.assertEqual(model.validate_request(self.backend, self.directory, self.value, request, "target"), self.private)
        for key, change in (("console_url", "http://127.0.0.1:18292"), ("scope_id", "another-scope")):
            with self.subTest(key=key), self.assertRaises(ValueError):
                model.validate_request(self.backend, self.directory, self.value, {**request, key: change}, "target")

    def test_target_cannot_replace_original_process_receipt_with_new_normalized_receipt(self):
        request = self.target_request()
        request["previous"]["process_receipt"] = self.json("substituted.json", read_json(Path(request["previous"]["process_receipt"]["path"])))
        request["previous"]["ready_receipt"] = request["previous"]["process_receipt"]
        with self.assertRaises(ValueError):
            model.validate_request(self.backend, self.directory, self.value, request, "target")

    def test_directory_replacement_is_rejected_even_at_same_path(self):
        self.data.rename(self.root / "original-data")
        self.data.mkdir()
        with self.assertRaises(ValueError):
            model.directory_identity(self.backend, self.request["data_directory"])

    def test_directory_link_is_rejected(self):
        with patch("devex_clone_model.linked", side_effect=lambda path: path == self.data):
            with self.assertRaises(ValueError):
                model.directory_identity(self.backend, self.request["data_directory"])

    def producers(self):
        runtime = self.json("source-runtime.json", {"worker_ready_url": "http://127.0.0.1:18301/readyz"})
        producers = [{"name": role, "identity": {**self.old, "pid": 620001 + index}} for index, role in enumerate(("api", "worker", "dataset"))]
        registry = self.json("registry.json", {"format_version": 1, "scope_id": "source-scope",
            "runtime_sha256": runtime["sha256"], "processes": producers})
        source = {"source": {"scope_id": "source-scope", "api_url": "http://127.0.0.1:18300"}, "runtime": runtime,
                  "producers_registry": registry, "processes": {item["name"]: self.json("source-" + item["name"] + ".json", {"identity": item["identity"]})
                    for item in producers[:2]}}
        self.value["source_request"] = self.json("source-complete.json", source)
        target_runtime = self.root / "target-runtime"
        target_runtime.mkdir()
        selected = {"runtime_dir": str(target_runtime), "api_url": "http://127.0.0.1:18400", "worker_ready_url": "http://127.0.0.1:18401/readyz"}
        return producers, selected

    def test_all_declared_source_producers_and_both_target_ports_are_checked(self):
        producers, selected = self.producers()
        with patch("devex_clone_factory_context.initialization_history", return_value=({}, {"target": {"scope_id": "target-scope"}})), patch("devex_clone_target_binding.request_binding", return_value=({}, selected)), patch("devex_clone_source_proof.process_identity", return_value=None) as observe, patch.object(model, "require_closed_port") as port:
            model.producers_stopped(self.backend, self.directory, self.value)
            self.assertEqual(observe.call_count, 3)
            self.assertEqual(port.call_count, 4)
        with patch("devex_clone_source_proof.process_identity", return_value=producers[-1]["identity"]):
            with self.assertRaises(ValueError):
                model.producers_stopped(self.backend, self.directory, self.value)


if __name__ == "__main__":
    unittest.main()
