"""已发布 seed 的缓存工具换代只继承已验证 successor，不改变资源与旧登记。"""
from pathlib import Path
from unittest.mock import patch
import devex_clone_cache_request as model
from devex_clone_run_state import binding
from restore_reference_plan import plan_hash
from test_devex_clone_cache import CacheFixture


class SuccessorRequestTests(CacheFixture):
    def successor(self):
        successor = self.json("successor.json", {"kind": "devex-clone-seed-review-successor"})
        Path(self.request["process"]["wsl"]["path"]).write_bytes(b"upgraded-wsl")
        wsl = binding(Path(self.request["process"]["wsl"]["path"]))
        self.tools = {
            "wsl": {key: wsl[key] for key in ("path", "sha256")},
            "redis_python": {"distribution": "Ubuntu-24.04", "path": "/usr/bin/python3",
                             "resolved_path": "/usr/bin/python3.12", "sha256": "d" * 64},
            "redis_server": {"distribution": "Ubuntu-24.04", "resolved_path": self.previous["executable"],
                             "sha256": self.previous["sha256"]},
        }
        self.ready = self.json("ready-review.json", {"tools": self.tools})
        self.ready["canonical_sha256"] = plan_hash({"tools":self.tools})
        self.source = {"directory": self.directory, "manifest": self.value, "seed_target": self.original,
                       "initialization": self.initial,
                       "review_successor": {"successor_review": self.ready, "predecessor_review": self.original["review"]}}
        return successor

    def successor_patches(self):
        stack = self.patches()
        stack.enter_context(patch("reference_fixture_successor._source_with_loader", return_value=self.source))
        stack.enter_context(patch("reference_fixture_environment.validate_current_review_tools", side_effect=lambda _: self.tools))
        stack.enter_context(patch.object(model, "predecessor_request_binding", return_value=(self.review, self.selected)))
        return stack

    def test_frozen_successor_validates_structure_without_observing_current_tool_files(self):
        successor = self.successor()
        with (self.successor_patches(),
              patch("reference_fixture_environment.validated_recorded_review_tools", return_value=self.tools),
              patch("devex_clone_cache_process.validate") as validate,
              patch.object(model, "process_binding", wraps=model.process_binding) as process_bound):
            model.successor_process(
                self.backend, self.directory, self.value, self.request, successor,
                observe_current_tools=False)
        self.assertEqual(validate.call_count, 3)
        self.assertEqual(process_bound.call_args.kwargs["review_binding"], self.review_binding)
        self.assertNotEqual(process_bound.call_args.kwargs["review_binding"]["path"], self.ready["path"])
        self.assertEqual([call.kwargs for call in validate.call_args_list],
                         [{"files": False}, {"files": False}, {"files": False}])

    def test_frozen_successor_survives_later_launcher_change_but_live_path_rejects_it(self):
        successor = self.successor()
        with (self.successor_patches(),
              patch("reference_fixture_environment.validated_recorded_review_tools", return_value=self.tools)):
            Path(self.request["process"]["wsl"]["path"]).write_bytes(b"later-wsl-generation")
            model.successor_process(
                self.backend, self.directory, self.value, self.request, successor,
                observe_current_tools=False)
            with self.assertRaises(ValueError):
                model.successor_process(
                    self.backend, self.directory, self.value, self.request, successor)
