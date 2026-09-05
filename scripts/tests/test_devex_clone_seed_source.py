"""seed source-register 的不可变证据链与失败关闭。"""
import argparse
from contextlib import nullcontext
import copy
import json
from pathlib import Path
import shutil
import unittest
from unittest.mock import patch
import uuid

import devex_clone_run_cli as cli
import devex_clone_seed_runtime as seed_runtime
import devex_clone_seed_source as source
import devex_clone_source_proof as source_proof
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding


def test_directory(label):
    root = Path.cwd().resolve() / ".local-tests/test-python"
    root.mkdir(parents=True, exist_ok=True)
    directory = root / f"{label}-{uuid.uuid4().hex}"
    directory.mkdir()
    return directory


class SeedSourceTests(unittest.TestCase):
    def setUp(self):
        self.backend = Path.cwd().resolve()
        self.directory = test_directory("seed-source")
        self.addCleanup(self.cleanup)
        (self.directory / "seed-runtime/runtime").mkdir(parents=True)
        self.runtime = self.directory / "seed-runtime/runtime"
        for name in ("runtime", "api", "worker"):
            write_json(self.runtime / f"{name}.json", {"name": name})
        self.identities = [
            {"pid": 101, "started": "1001", "executable": str(self.backend / "api.exe")},
            {"pid": 102, "started": "1002", "executable": str(self.backend / "worker.exe")},
            {"pid": 101, "started": "1003", "executable": str(self.backend / "node.exe")},
        ]
        roles = [{"role": role, "identity": self.identities[index], "receipt": {"fixture": role}}
                 for index, role in enumerate(("api", "worker"))]
        self.lineage = {"format_version": 1, "kind": "devex-clone-producer-lineage",
                        "run_directory": str(self.directory), "run_manifest": {"fixture": "manifest"},
                        "runtimes": [{"runtime_directory": str(self.runtime), "role_receipts": roles}],
                        "node_producers": [],
                        "identities": [{"identity": item, "occurrences": []}
                                       for item in self.identities]}
        self.anchors = {
            "run_manifest": {"fixture": "manifest"},
            "source_to_seed": {"copy_stage_receipt": {"fixture": "copy-stage"},
                               "copy_result": {"fixture": "copy"},
                               "ledger_head": {"fixture": "ledger"}},
            "post_copy": {"fixture": "post"}, "post_verify": {"fixture": "post-verify"},
            "seed_registration": {"fixture": "seed"},
            "seed_registration_stage": {"fixture": "seed-stage"},
            "capacity": {"fixture": "capacity"}, "departments": {"fixture": "departments"},
            "identity": {"fixture": "identity"}, "seed_handoff": {"fixture": "handoff"},
            "seed_close": {"fixture": "close"}, "lineage": self.lineage,
            "storage": {"fixture": "storage"}, "source_environment": {"fixture": "environment"},
            "post": {"node": {"path": "node"}}, "runtime": self.runtime,
            "handoff": {"scope_id": "seed-scope"}, "private": {"APP_ENV": "test"},
        }
        self.request = {"format_version": 1, "kind": "devex-clone-source-export",
                        "id": "seed-request"}
        self.generation = {"request_sha256": "a" * 64}

    def cleanup(self):
        root = self.backend / ".local-tests/test-python"
        if self.directory.exists() and self.directory.resolve().parent == root.resolve():
            shutil.rmtree(self.directory)

    def test_registry_keeps_pid_reuse_as_distinct_creation_generation(self):
        result = source.producer_registry(self.lineage, self.runtime,
                                          binding(self.runtime / "runtime.json"), "seed-scope")
        self.assertEqual([item["name"] for item in result["processes"]],
                         ["api", "worker", "history-0001"])
        self.assertEqual([item["identity"] for item in result["processes"]], self.identities)

    def test_source_id_is_stable_for_maximum_length_run_id(self):
        original = "a" * 48
        derived = source._source_id(original)
        self.assertEqual(derived, source._source_id(original))
        self.assertLessEqual(len(derived), 48)
        self.assertNotEqual(derived, source._source_id("b" * 48))

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

    def test_registry_rejects_missing_lineage_or_latest_runtime_roles(self):
        for mutate in (
            lambda value: value["identities"].pop(),
            lambda value: value["runtimes"][0]["role_receipts"].pop(),
            lambda value: value["runtimes"].clear(),
        ):
            value = copy.deepcopy(self.lineage)
            mutate(value)
            with self.subTest(value=value), self.assertRaises(ValueError):
                source.producer_registry(value, self.runtime,
                                         binding(self.runtime / "runtime.json"), "seed-scope")

    def test_latest_close_requires_latest_published_drained_result_and_no_later_writer(self):
        result_path = self.directory / "results/0001.json"
        result_path.parent.mkdir()
        write_json(result_path, {"status": "seed_runtime_closed", "outbox_drained": True})
        close = {"number": 1, "stage": "seed-runtime", "mode": "close", "status": "passed",
                 "result": binding(result_path)}
        with patch.object(source, "validate_close_result"):
            self.assertEqual(source._latest_close(self.backend, self.directory,
                {"attempts": [close]}, self.runtime, {}, 2), binding(result_path))
            failed = {**close, "status": "failed"}
            with self.assertRaisesRegex(ValueError, "尚未成功"):
                source._latest_close(self.backend, self.directory, {"attempts": [failed]},
                                     self.runtime, {}, 2)
            result_path.write_text(json.dumps(
                {"status": "seed_runtime_closed", "outbox_drained": False}), encoding="utf-8")
            close["result"] = binding(result_path)
            with self.assertRaisesRegex(ValueError, "排空"):
                source._latest_close(self.backend, self.directory, {"attempts": [close]},
                                     self.runtime, {}, 2)
            result_path.write_text(json.dumps(
                {"status": "seed_runtime_closed", "outbox_drained": True}), encoding="utf-8")
            close["result"] = binding(result_path)
            later = {"number": 2, "stage": "seed-runtime", "mode": "identities-verify",
                     "status": "passed", "result": {"fixture": True}}
            with self.assertRaisesRegex(ValueError, "未重新排空"):
                source._latest_close(self.backend, self.directory,
                                     {"attempts": [close, later]}, self.runtime, {}, 3)

    def test_attempt_result_cannot_cross_run(self):
        other = test_directory("other-run")
        self.addCleanup(lambda: shutil.rmtree(other) if other.exists() else None)
        (other / "results").mkdir()
        path = other / "results/0001.json"
        write_json(path, {"status": "passed"})
        attempt = {"number": 1, "result": binding(path)}
        with self.assertRaisesRegex(ValueError, "不属于固定 run"):
            source._attempt_binding(self.backend, self.directory, attempt, "fixture")

    def test_storage_rejects_wrong_endpoint_or_binary(self):
        initialized = self.directory / "initialized.json"
        write_json(initialized, {"fixture": True})
        executable = self.directory / "fixed-rustfs.exe"
        executable.write_bytes(b"rustfs")
        target = {"target": {"s3": {"endpoint": "http://127.0.0.1:29200"}},
                  "storage": {"rustfs": {"identity": {"executable": str(executable)},
                                           "sha256": "a" * 64}}}
        observed = {"api_url": "http://127.0.0.1:29200",
                    "storage": {"identity": {"executable": str(executable)},
                                "sha256": "a" * 64}}
        value = {"initialized": binding(initialized)}
        with patch.object(source, "initialization_history", return_value=({}, target)), \
                patch.object(source, "request_binding"), \
                patch.object(source, "current_storage_binding", return_value=observed):
            self.assertEqual(source._storage(self.backend, self.directory, value), observed)
            for changed in ({**observed, "api_url": "http://127.0.0.1:29201"},
                            {**observed, "storage": {**observed["storage"], "sha256": "b" * 64}}):
                with self.subTest(changed=changed), \
                        patch.object(source, "current_storage_binding", return_value=changed), \
                        self.assertRaisesRegex(ValueError, "端点或二进制"):
                    source._storage(self.backend, self.directory, value)

    def test_register_source_publishes_only_local_bound_evidence_and_rechecks(self):
        number = 7
        state = {"attempts": [{"number": number, "stage": "seed-runtime",
                               "mode": "source-register", "status": "running"}]}
        with patch.object(source, "load_state", return_value=state), \
                patch.object(source, "_anchors", side_effect=[copy.deepcopy(self.anchors),
                                                               copy.deepcopy(self.anchors)]) as anchors, \
                patch.object(source, "_source_request", return_value=self.request) as request, \
                patch.object(source, "verify_generation", return_value=self.generation) as generation:
            result = source.register_source(self.backend, self.directory,
                                            {"copy_stage": "source_to_seed"}, number,
                                            run="fixed-runner")
        self.assertEqual(result["status"], "seed_source_registered")
        self.assertEqual(result["remote_writes"], 0)
        self.assertTrue(result["outbox_drained"])
        self.assertFalse(result["restore_qualified"])
        registration = read_json(Path(result["registration"]["path"]))
        self.assertEqual(set(registration), source.REGISTRATION_FIELDS)
        self.assertEqual(registration["producer_lineage"],
                         binding(self.directory / "seed-runtime/attempt-0007/producer-lineage.json"))
        self.assertEqual(anchors.call_count, 2)
        self.assertEqual(request.call_count, 2)
        self.assertEqual(generation.call_count, 2)
        for call in generation.call_args_list:
            self.assertEqual(call.args[2], "fixed-runner")

    def test_register_source_rejects_duplicate_published_registration(self):
        state = {"attempts": [
            {"number": 1, "stage": "seed-runtime", "mode": "source-register", "status": "passed"},
            {"number": 2, "stage": "seed-runtime", "mode": "source-register", "status": "running"},
        ]}
        with patch.object(source, "load_state", return_value=state), \
                self.assertRaisesRegex(ValueError, "不能生成第二份"):
            source.register_source(self.backend, self.directory, {}, 2)

    def test_base_exception_after_complete_local_registration_reuses_same_binding(self):
        class OuterResultLost(BaseException):
            pass

        state = {"attempts": [{"number": 7, "stage": "seed-runtime",
                               "mode": "source-register", "status": "running"}]}
        original = source._materialize

        def publish_then_interrupt(*args, **kwargs):
            original(*args, **kwargs)
            raise OuterResultLost()

        with patch.object(source, "load_state", side_effect=lambda *_: state), \
                patch.object(source, "_anchors", return_value=copy.deepcopy(self.anchors)), \
                patch.object(source, "_source_request", return_value=self.request), \
                patch.object(source, "verify_generation", return_value=self.generation):
            with patch.object(source, "_materialize", side_effect=publish_then_interrupt), \
                    self.assertRaises(OuterResultLost):
                source.register_source(self.backend, self.directory,
                                       {"copy_stage": "source_to_seed"}, 7,
                                       run="fixed-runner")
            registration = self.directory / "seed-runtime/attempt-0007/source-registration.json"
            original_binding = binding(registration)
            original_files = {item.name: binding(item) for item in registration.parent.iterdir()}
            state["attempts"] = [
                {"number": 7, "stage": "seed-runtime", "mode": "source-register",
                 "status": "failed"},
                {"number": 8, "stage": "seed-runtime", "mode": "source-register",
                 "status": "running"},
            ]
            result = source.register_source(self.backend, self.directory,
                                            {"copy_stage": "source_to_seed"}, 8,
                                            run="fixed-runner")
        self.assertEqual(result["registration"], original_binding)
        self.assertEqual({item.name: binding(item) for item in registration.parent.iterdir()},
                         original_files)
        self.assertFalse((self.directory / "seed-runtime/attempt-0008").exists())

    def test_partial_prior_attempt_is_resumed_in_place_without_overwrite(self):
        state = {"attempts": [{"number": 7, "stage": "seed-runtime",
                               "mode": "source-register", "status": "running"}]}
        with patch.object(source, "load_state", side_effect=lambda *_: state), \
                patch.object(source, "_anchors", return_value=copy.deepcopy(self.anchors)), \
                patch.object(source, "_source_request", return_value=self.request), \
                patch.object(source, "verify_generation", side_effect=RuntimeError("interrupted")):
            with self.assertRaises(RuntimeError):
                source.register_source(self.backend, self.directory,
                                       {"copy_stage": "source_to_seed"}, 7)
        output = self.directory / "seed-runtime/attempt-0007"
        retained = {item.name: binding(item) for item in output.iterdir()}
        self.assertEqual(set(retained), {"producer-lineage.json", "source-storage.json",
                                        "producers.json", "source-request.json"})
        state["attempts"] = [
            {"number": 7, "stage": "seed-runtime", "mode": "source-register", "status": "failed"},
            {"number": 8, "stage": "seed-runtime", "mode": "source-register", "status": "running"},
        ]
        with patch.object(source, "load_state", side_effect=lambda *_: state), \
                patch.object(source, "_anchors", return_value=copy.deepcopy(self.anchors)), \
                patch.object(source, "_source_request", return_value=self.request), \
                patch.object(source, "verify_generation", return_value=self.generation):
            result = source.register_source(self.backend, self.directory,
                                            {"copy_stage": "source_to_seed"}, 8)
        self.assertEqual({name: binding(output / name) for name in retained}, retained)
        self.assertEqual(Path(result["registration"]["path"]).parent, output)
        self.assertFalse((self.directory / "seed-runtime/attempt-0008").exists())

    def test_non_prefix_prior_evidence_is_preserved_and_rejected(self):
        output = self.directory / "seed-runtime/attempt-0007"
        output.mkdir()
        retained = output / "source-request.json"
        write_json(retained, self.request)
        original = binding(retained)
        state = {"attempts": [
            {"number": 7, "stage": "seed-runtime", "mode": "source-register",
             "status": "failed"},
            {"number": 8, "stage": "seed-runtime", "mode": "source-register",
             "status": "running"},
        ]}
        with patch.object(source, "load_state", return_value=state), \
                patch.object(source, "_anchors") as anchors, \
                self.assertRaisesRegex(ValueError, "不是可恢复的写入前缀"):
            source.register_source(self.backend, self.directory,
                                   {"copy_stage": "source_to_seed"}, 8)
        self.assertEqual(binding(retained), original)
        self.assertEqual({item.name for item in output.iterdir()}, {retained.name})
        self.assertFalse((self.directory / "seed-runtime/attempt-0008").exists())
        anchors.assert_not_called()


class SourceRegisterDispatchTests(unittest.TestCase):
    def test_cli_requires_write_and_routes_without_request(self):
        parser = argparse.ArgumentParser()
        cli.add_commands(parser.add_subparsers(dest="command", required=True))
        backend = Path.cwd().resolve()
        directory = backend / ".local-tests/seed-source-cli"
        base = ["seed-runtime", "--backend-dir", str(backend), "--run-dir", str(directory),
                "--operation", "source-register"]
        args = parser.parse_args(base)
        with patch.object(cli, "execute") as execute, self.assertRaisesRegex(ValueError, "显式 --write"):
            cli.dispatch(args, backend)
        execute.assert_not_called()
        outer = {"status": "stage_finished", "stage": "seed-runtime", "mode": "source-register",
                 "attempt": 9, "restore_qualified": False}
        with patch.object(cli, "execute", return_value=outer) as execute:
            self.assertEqual(cli.dispatch(parser.parse_args([*base, "--write"]), backend), outer)
        execute.assert_called_once_with(backend, directory, "seed-runtime", "source-register",
                                        seed_request=None, producer_binding=None)

    def test_seed_runtime_dispatches_before_context_construction(self):
        backend, directory, value = Path.cwd(), Path.cwd() / ".local-tests/run", {"manifest": True}
        expected = {"status": "seed_source_registered"}
        with patch.object(seed_runtime, "require_quiet"), \
                patch.object(seed_runtime, "target_lock", return_value=nullcontext()), \
                patch.object(seed_runtime, "Context") as context, \
                patch.object(source, "register_source", return_value=expected) as register:
            self.assertEqual(seed_runtime.execute_seed(backend, directory, value,
                                                       "source-register", 11), expected)
        context.assert_not_called()
        register.assert_called_once_with(backend, directory, value, 11)


if __name__ == "__main__":
    unittest.main()
