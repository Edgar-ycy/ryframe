"""唯一 generation-request 生产者的纯预览、原子发布与失败关闭。"""
import argparse
from contextlib import ExitStack, contextmanager, redirect_stderr
import copy
from io import StringIO
from pathlib import Path
import unittest
from unittest.mock import patch

import reference_fixture_successor_generation as producer
import test_devex_clone_seed_generation as fixtures
from devex_clone_capture import read_json
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
            stack.enter_context(patch.object(producer, "source_binding", return_value=self.f.source["generation"]["physical_binding"]))
            yield published, registered

    def test_preview_recomputes_current_source_and_never_writes_any_file_or_git_object(self):
        before = {str(path): path.read_bytes() for path in self.f.directory.rglob("*") if path.is_file()}
        with self.inputs() as (published, registered), patch("subprocess.run", side_effect=AssertionError("preview 不能执行写入命令")):
            value = producer.build(self.f.backend, *self.values)
        self.assertEqual(value, self.f.request)
        self.assertEqual(published.call_count, 2)
        self.assertFalse(registered.call_args.kwargs["reconstruct"])
        self.assertEqual(before, {str(path): path.read_bytes() for path in self.f.directory.rglob("*") if path.is_file()})

    def test_publish_is_atomic_immutable_and_rejects_output_collision(self):
        output = self.f.directory / "published-request.json"
        with self.inputs() as (published, _registered):
            value = producer.publish(self.f.backend, output, *self.values)
            self.assertEqual(published.call_count, 6)
            self.assertEqual(read_json(output), value)
            descriptor = binding(output)
            with self.assertRaises(ValueError):
                producer.publish(self.f.backend, output, *self.values)
            self.assertEqual(binding(output), descriptor)
        self.assertFalse(any(path.name.endswith(".pending") for path in self.f.directory.iterdir()))

    def test_source_rebind_or_environment_drift_prevents_publication(self):
        output = self.f.directory / "never.json"
        with self.inputs() as (published, _registered):
            changed = copy.deepcopy(self.f.source)
            changed["storage"]["storage"] = {"different": True}
            published.side_effect = [self.f.source, changed]
            with self.assertRaisesRegex(ValueError, "来源在只读构造期间变化"):
                producer.publish(self.f.backend, output, *self.values)
        self.assertFalse(output.exists())
        with self.inputs(), patch.object(producer, "source_binding", return_value={"different": "physical source"}), self.assertRaises(ValueError):
            producer.build(self.f.backend, *self.values)

    def test_path_escape_and_reparse_output_are_rejected_before_source_reads(self):
        with self.inputs() as (published, _registered):
            for output in (self.f.backend.parent / "escape.json", self.f.directory / "missing-parent/out.json"):
                with self.subTest(path=output), self.assertRaises(ValueError):
                    producer.publish(self.f.backend, output, *self.values)
            target = self.f.directory / "reparse.json"
            with patch("devex_clone_model.linked", side_effect=lambda path: path == target), self.assertRaises(ValueError):
                producer.publish(self.f.backend, target, *self.values)
            published.assert_not_called()

    def test_parser_requires_exact_publish_pair_and_rejects_unknown_or_abbreviated_flags(self):
        parser = argparse.ArgumentParser(allow_abbrev=False)
        producer.arguments(parser)
        argv = ["--backend-dir", str(self.f.backend), "--successor", str(self.values[0]), "--source-backend", str(self.f.backend),
                "--expected-head", "a" * 40, "--backend-build", str(self.values[3]), "--maintenance-build", str(self.values[4]),
                "--source-environment", str(self.values[5]), "--id", "r24-source"]
        for extra in (["--write"], ["--output", str(self.f.directory / "bad.json")]):
            with redirect_stderr(StringIO()), self.assertRaises(SystemExit) as error:
                producer.execute(parser.parse_args(argv + extra), parser)
            self.assertEqual(error.exception.code, 2)
        for extra in (["--out", "bad"], ["--unknown", "value"]):
            with redirect_stderr(StringIO()), self.assertRaises(SystemExit) as error:
                parser.parse_args(argv + extra)
            self.assertEqual(error.exception.code, 2)
        with self.inputs():
            preview = producer.execute(parser.parse_args(argv), parser)
        self.assertEqual(preview["request"], self.f.request)
        self.assertEqual(preview["status"], "source_generation_request_planned")


if __name__ == "__main__":
    unittest.main()
