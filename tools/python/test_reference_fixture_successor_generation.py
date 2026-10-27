"""历史来源请求的只读重算与谱系边界；不保留旧 Python 发布入口。"""
from types import SimpleNamespace
from contextlib import ExitStack, contextmanager
import copy
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))

import reference_fixture_successor_generation as producer
import devex_clone_seed_generation as generation
import devex_clone_seed_generation_lineage_recovery as lineage
import test_devex_clone_seed_generation as fixtures
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import binding


class GenerationRequestTests(unittest.TestCase):
    def setUp(self):
        self.f = fixtures.GenerationTests()
        self.f.setUp()
        self.addCleanup(self.f.doCleanups)
        self.build = read_json(Path(self.f.build["path"]))
        self.values = (Path(self.f.successor["path"]), self.f.backend, "a" * 40, Path(self.f.build["path"]),
                       Path(self.f.request["maintenance_build"]["path"]), Path(self.f.request["source_environment"]["path"]), "r24-source")

    @contextmanager
    def inputs(self):
        with ExitStack() as stack:
            published = stack.enter_context(patch.object(producer, "published_source", return_value=self.f.source))
            registered = stack.enter_context(patch.object(producer, "registered_inputs", return_value=(self.f.backend, self.build)))
            stack.enter_context(patch.object(producer, "verify_evidence", return_value={"source": self.build["sources"]["full"]["source"]}))
            stack.enter_context(patch.object(producer, 'native_invoke', return_value=(SimpleNamespace(returncode=0), {"ok": True, "result": {"value": self.f.source["generation"]["physical_binding"]}})))
            yield published, registered

    def test_preview_recomputes_current_source_and_never_writes_any_file_or_git_object(self):
        before = {str(path): path.read_bytes() for path in self.f.directory.rglob("*") if path.is_file()}
        with self.inputs() as (published, registered), patch(
            "devex_clone_seed_generation_lineage_recovery.pending_lineage_request"
        ) as recovery, patch("subprocess.run", side_effect=AssertionError("preview 不能执行写入命令")):
            value = producer.build(self.f.backend, *self.values)
        self.assertEqual(value, self.f.request)
        self.assertEqual(published.call_count, 2)
        recovery.assert_not_called()
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.f.directory.rglob("*") if path.is_file()})

    def test_pending_c68_uses_narrow_reregistration_and_rechecks_it(self):
        ordinary = ValueError("seed 发布后存在失败或未收尾阶段")
        verification = (
            patch.object(producer, "registered_inputs", return_value=(self.f.backend, self.build)),
            patch.object(
                producer,
                "verify_evidence",
                return_value={"source": self.build["sources"]["full"]["source"]},
            ),
            patch.object(
                producer,
                'native_invoke',
                return_value=(SimpleNamespace(returncode=0), {"ok": True, "result": {"value": self.f.source["generation"]["physical_binding"]}}),
            ),
        )
        with patch.object(producer, "published_source", side_effect=ordinary) as published, patch(
            "reference_fixture_successor._source_with_loader",
            return_value={"directory": self.f.directory},
        ) as registered, patch(
            "devex_clone_seed_generation_lineage_recovery.pending_lineage_request",
            side_effect=[(self.f.request, self.f.source), (self.f.request, self.f.source)],
        ) as recovery, verification[0], verification[1], verification[2]:
            value = producer.build(self.f.backend, *self.values)
        self.assertEqual(value, self.f.request)
        published.assert_called_once()
        registered.assert_called_once()
        self.assertEqual(recovery.call_count, 2)

    def test_non_c68_failure_preserves_normal_published_source_rejection(self):
        ordinary = ValueError("普通来源拒绝")
        with patch.object(producer, "published_source", side_effect=ordinary), patch(
            "reference_fixture_successor._source_with_loader",
            return_value={"directory": self.f.directory},
        ), patch(
            "devex_clone_seed_generation_lineage_recovery.pending_lineage_request",
            return_value=None,
        ) as recovery, self.assertRaisesRegex(ValueError, "普通来源拒绝") as raised:
            producer.build(self.f.backend, *self.values)
        self.assertIs(raised.exception, ordinary)
        recovery.assert_called_once()

    def pending_fixture(self):
        directory = self.f.directory / "pending-lineage"
        directory.mkdir()
        state = {
            "attempts": [
                {
                    "number": 68,
                    "stage": "seed-runtime",
                    "mode": "source-generation-start",
                    "status": "failed",
                    "result": None,
                    "error_type": "ValueError",
                }
            ]
        }
        write_json(directory / "state.json", state)
        execution = self.f.directory / "execution"
        maintenance = execution / ".local-tests/maintenance"
        maintenance.mkdir(parents=True)
        paths = {}
        for name, root in (
            ("old-runtime", self.f.directory),
            ("new-runtime", self.f.directory),
            ("environment", self.f.directory),
            ("old-maintenance", maintenance),
            ("new-maintenance", maintenance),
        ):
            path = root / f"{name}.json"
            write_json(path, {"name": name})
            paths[name] = path
        previous = {
            **copy.deepcopy(self.f.request),
            "execution_backend": str(execution),
            "backend_build": binding(paths["old-runtime"]),
            "maintenance_build": binding(paths["old-maintenance"]),
            "source_environment": binding(paths["environment"]),
        }
        request_path = directory / "g0068/request.json"
        request_path.parent.mkdir()
        write_json(request_path, previous)
        return directory, state, execution, paths, previous, binding(request_path)

    def test_pending_lineage_request_only_replaces_both_build_bindings(self):
        directory, state, execution, paths, previous, previous_binding = self.pending_fixture()
        with patch.object(lineage, "load_state", return_value=state), patch.object(
            lineage, "_history", return_value=({}, ("fixed-tail",))
        ), patch.object(
            lineage,
            "_request",
            return_value=(previous, previous_binding, previous_binding, previous),
        ), patch.object(lineage, "_source", return_value=self.f.source) as source, patch.object(
            lineage, "_verify_request"
        ) as verify, patch.object(generation, "_reregistered_builds") as equivalent:
            result = lineage.pending_lineage_request(
                self.f.backend,
                directory,
                self.f.successor,
                execution,
                "a" * 40,
                paths["new-runtime"],
                paths["new-maintenance"],
                paths["environment"],
                "r24-source",
            )
        request, recovered_source = result
        self.assertEqual(recovered_source, self.f.source)
        self.assertEqual(
            {field for field in request if request[field] != previous[field]},
            {"backend_build", "maintenance_build"},
        )
        equivalent.assert_called_once_with(self.f.backend, previous, request)
        source.assert_called_once_with(
            self.f.backend, directory, state["attempts"], request, ("fixed-tail",)
        )
        verify.assert_called_once_with(self.f.backend, request, self.f.source)

        with patch.object(lineage, "load_state", return_value={"attempts": []}):
            self.assertIsNone(
                lineage.pending_lineage_request(
                    self.f.backend,
                    directory,
                    self.f.successor,
                    execution,
                    "a" * 40,
                    paths["new-runtime"],
                    paths["new-maintenance"],
                    paths["environment"],
                    "r24-source",
                )
            )

    def test_pending_lineage_request_rejects_unpaired_fields_and_state_drift(self):
        directory, state, execution, paths, previous, previous_binding = self.pending_fixture()

        def patches():
            return (
                patch.object(lineage, "load_state", return_value=state),
                patch.object(lineage, "_history", return_value=({}, ("fixed-tail",))),
                patch.object(
                    lineage,
                    "_request",
                    return_value=(previous, previous_binding, previous_binding, previous),
                ),
                patch.object(lineage, "_source", return_value=self.f.source),
                patch.object(lineage, "_verify_request"),
                patch.object(generation, "_reregistered_builds"),
            )

        for request_id, runtime in (
            ("other-request", paths["new-runtime"]),
            ("r24-source", Path(previous["backend_build"]["path"])),
        ):
            contexts = patches()
            with contexts[0], contexts[1], contexts[2], contexts[3], contexts[4], contexts[5], \
                    self.subTest(request_id=request_id, runtime=runtime), self.assertRaises(ValueError):
                lineage.pending_lineage_request(
                    self.f.backend,
                    directory,
                    self.f.successor,
                    execution,
                    "a" * 40,
                    runtime,
                    paths["new-maintenance"],
                    paths["environment"],
                    request_id,
                )

        def drift(*_args):
            (directory / "state.json").write_text('{"changed":true}\n', encoding="utf-8")

        contexts = list(patches())
        contexts[4] = patch.object(lineage, "_verify_request", side_effect=drift)
        with contexts[0], contexts[1], contexts[2], contexts[3], contexts[4], contexts[5], \
                self.assertRaisesRegex(ValueError, "账本或输入"):
            lineage.pending_lineage_request(
                self.f.backend,
                directory,
                self.f.successor,
                execution,
                "a" * 40,
                paths["new-runtime"],
                paths["new-maintenance"],
                paths["environment"],
                "r24-source",
            )

    def test_historical_rebuild_rejects_source_and_physical_configuration_drift(self):
        with self.inputs() as (published, _registered):
            changed = copy.deepcopy(self.f.source)
            changed["storage"]["storage"] = {"different": True}
            published.side_effect = [self.f.source, changed]
            with self.assertRaisesRegex(ValueError, "来源在只读构造期间变化"):
                producer.build(self.f.backend, *self.values)
        with self.inputs(), patch.object(producer, 'native_invoke', return_value=(SimpleNamespace(returncode=0), {"ok": True, "result": {"value": {"different": "physical source"}}})), self.assertRaises(ValueError):
            producer.build(self.f.backend, *self.values)

if __name__ == "__main__":
    unittest.main()
