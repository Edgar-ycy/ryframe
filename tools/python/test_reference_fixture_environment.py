"""隔离参考夹具计划不得回退读取历史复制来源。"""
from __future__ import annotations

import copy
import hashlib
import subprocess
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import patch
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
import reference_fixture_environment as environment
from devex_clone_target_binding import REVIEW_FILE_TOOLS, validate_review
from reference_fixture_service_context import service_run
from restore_build import file_digest
from restore_reference_plan import plan_hash
from source_inventory import snapshot


def publish_execution_fixture(case):
    original = case.root / "original"
    backend, frontend = original / "backend", original / "frontend"
    contract = (json.dumps({"info": {"title": "RyFrame API", "version": "0.0.1"},
                           "openapi": "3.1.0", "paths": {}}, ensure_ascii=False,
                          sort_keys=True, indent=2) + "\n").encode()
    sources = {}
    for name, root in (("backend", backend), ("frontend", frontend)):
        (root / "openapi").mkdir(parents=True)
        (root / "src").mkdir()
        (root / ".gitignore").write_text(".local-tests/\ntarget/\nnode_modules/\ndist/\n", encoding="utf-8")
        (root / "src/lib.rs").write_text("// original\n", encoding="utf-8")
        (root / "openapi/openapi.json").write_bytes(contract)
        if name == "backend":
            (root / "xtask").mkdir()
            (root / "xtask/fixture.txt").write_text("fixture\n", encoding="utf-8")
            (root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
        else:
            (root / "openapi/source.json").write_text(json.dumps({"schema_version": 1,
                "backend_repository": "fixture/backend", "backend_commit": sources["backend"]["head"],
                "openapi_path": "openapi/openapi.json", "openapi_version": "3.1.0",
                "sha256": hashlib.sha256(contract).hexdigest()}, sort_keys=True, indent=2) + "\n", encoding="utf-8")
        for args in (("init", "--quiet", "--template="), ("add", "."),
                     ("-c", "user.name=Fixture", "-c", "user.email=fixture@ryframe.invalid",
                      "-c", "commit.gpgSign=false", "-c", "core.hooksPath=" + str(case.root / "no-hooks"),
                      "commit", "--quiet", "-m", "test(fixtures): 初始化发布来源夹具")):
            subprocess.run(["git", "-C", str(root), "-c", "core.autocrlf=false", *args],
                           check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=30)
        sources[name] = snapshot(root)[0]
    (backend / "src/lib.rs").write_text("// generated\n", encoding="utf-8")
    model = backend / "crates/order-business/src/resources/mod.rs"
    model.parent.mkdir(parents=True)
    model.write_text("// Order\n", encoding="utf-8")
    case.fixture = {"format_version": 1, "fixture": "business", "status": "ready",
        "fixture_sha256": hashlib.sha256(model.read_bytes()).hexdigest(),
        "sources": sources, "paths": {"backend": str(backend), "frontend": str(frontend)},
        "generated": {name: snapshot(root)[0] for name, root in (("backend", backend), ("frontend", frontend))}}
    case.original_fixture = original / "fixture.json"
    case.original_fixture.write_text(json.dumps(case.fixture), encoding="utf-8")
    (backend / ".local-tests").mkdir()
    output = backend / ".local-tests/published"
    from rate_limit_tool import tool_binding, verified_tool
    with verified_tool(tool_binding()) as executable:
        subprocess.run([executable, "check", "recovery", "fixture", "source-pair", "publish-execution",
            "--output-dir", str(output), "--write"], cwd=backend, check=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
    case.execution_source = output / "execution-source.json"
    case.publication = json.loads(case.execution_source.read_text(encoding="utf-8"))
    execution = case.publication["published"]["backend"]["path"]
    case.review["execution_source"] = environment.bound(case.execution_source)
    case.review["sources"] = {name + "_head": sources[name]["head"] for name in ("backend", "frontend")}
    for scope in case.review["scopes"].values():
        scope["backend_dir"] = execution
    case.review["future_root"] = str(Path(execution) / ".local-tests/reference-fixture/run-r1")
    (Path(execution) / ".local-tests/reference-fixture").mkdir(parents=True)


class ReferenceFixtureEnvironmentTests(unittest.TestCase):
    def write(self, name: str, value: dict) -> Path:
        path = self.root / name
        path.write_text(json.dumps(value), encoding="utf-8")
        return path

    def static_ready(self, predecessor: Path, output: Path) -> dict:
        """构造冻结预检测试数据，不执行生产续签或系统观察。"""
        ready = copy.deepcopy(self.review)
        observed = self.observed_tools()
        ready["ready_for_execution"] = True
        ready["tools"].update(observed)
        ready["tools"].pop("redis_python", None)
        ready["services"]["rustfs"].update(
            scope_id="services-" + ready["scopes"]["seed"]["scope_id"],
            process_receipt=str(service_run(ready) / "rustfs/process.json"))
        ready["preflight"] = {"format_version": 1, "kind": "reference-fixture-tool-preflight",
                              "status": "verified", "supersedes": environment.bound(predecessor),
                              "tools": observed}
        output.write_text(json.dumps(ready), encoding="utf-8")
        return ready

    def observed_tools(self) -> dict:
        observed = {name: copy.deepcopy(self.review["tools"][name]) for name in REVIEW_FILE_TOOLS}
        # 与 Rust native_lifecycle 的工具夹具同形；只替代系统观察，收据守卫真实执行。
        sources = [{"path": "xtask/source-inventory.json", "sha256": "a" * 64}]
        observed.update({
            "redis_server": {**self.review["tools"]["redis_server"], "path": "/usr/bin/redis-server",
                             "version": "Redis server v=7.0.15"},
            "wsl": {"path": str(self.wsl.resolve()), "sha256": file_digest(self.wsl)["sha256"]},
            "native_tool": {"distribution": "Ubuntu", "user": "fixture", "uid": 1000,
                            "program": "D:/fixture/xtask", "linux_program": "/mnt/d/fixture/xtask",
                            "source_root": "D:/fixture/source", "linux_source_root": "/mnt/d/fixture/source",
                            "source_commit": "b" * 40, "sha256": "c" * 64,
                            "source_sha256": plan_hash(sources), "sources": sources},
        })
        return observed

    def setUp(self):
        self.backend = Path(__file__).resolve().parents[2]
        self.temporary = WorkspaceDirectory(dir=self.backend / ".local-tests/python-unit")
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.tools = {}
        for name in REVIEW_FILE_TOOLS:
            tool = self.root / f"{name}.bin"
            tool.write_bytes(name.encode())
            self.tools[name] = tool
        self.wsl = self.root / "wsl.bin"; self.wsl.write_bytes(b"wsl")
        defaults = self.root / "mysql.cnf"; defaults.write_text("[client]", encoding="utf-8")
        self.review = {"kind": "review-only-perf-resource-plan-with-readonly-preflight", "ready_for_execution": True,
                       "reference": {name: {"databases": []} for name in ("source", "protected_target")},
                       "tools": {name: {"path": str(self.tools[name]), "sha256": file_digest(self.tools[name])["sha256"]}
                                 for name in REVIEW_FILE_TOOLS},
                       "services": {"rustfs": {"api": "http://127.0.0.1:29200", "console": "http://127.0.0.1:29201", "data_dir": str(self.root / "data")},
                                    "redis": {"directory": str(self.root / "redis")}}, "scopes": {}}
        self.review["tools"]["redis_server"] = {"distribution": "Ubuntu", "resolved_path": "/usr/bin/redis-server", "sha256": "a" * 64}
        for side in ("seed", "base", "candidate"):
            scope = "fixture-" + side
            self.review["scopes"][side] = {"scope_id": scope, "runtime_dir": str(self.root / (side + "-runtime")),
                "backend_dir": str(self.root / "business-backend"),
                "identity_ledger": str(self.root / (side + "-identities")), "api_url": "http://127.0.0.1:18210",
                "worker_ready_url": "http://127.0.0.1:19210/readyz", "frontend_url": "http://127.0.0.1:4190",
                "objects": {"endpoint": "http://127.0.0.1:29200", "region": "us-east-1"},
                "redis": {"url": "redis://127.0.0.1:16390/0", "namespace": f"ryframe:{{{scope}}}:",
                          "ownership_key": f"ryframe:{{{scope}}}:.ryframe-owner", "ownership_value": "owner"},
                "databases": [{"key": key, "database": f"fixture_{side}_{key.replace('-', '_')}",
                               "mode": "shared" if key in ("shared-control", "shared") else "dedicated",
                               "expected_server_uuid": "uuid", "connection_file": str(defaults), "host": "127.0.0.1", "port": 3306}
                               for key in ("shared-control", "shared", "dedicated-a", "dedicated-b")]}
        self.review["future_root"] = str(self.root / "business-backend/.local-tests/reference-fixture/run-r1")
        self.fixture = {"format_version": 1, "fixture": "business", "status": "ready", "sources": {"backend": {"head": "a" * 40}, "frontend": {"head": "b" * 40}},
                        "paths": {"backend": str(self.root / "business-backend"), "frontend": str(self.root / "business-frontend")}}
        self.maintenance = {"format_version": 1, "kind": "devex-clone-tool-build", "resources_modified": False, "artifacts":
                            {key: {"executable": str(self.tools["mysql"])} for key in ("reset", "migrate", "tenant-data")}}

    def test_plan_is_read_only_and_excludes_historical_data(self):
        publish_execution_fixture(self)
        result = environment.plan(self.backend, self.write("review.json", self.review), self.execution_source, self.write("build.json", self.maintenance))
        self.assertFalse(result["historical_data_used"])
        self.assertEqual(result["remote_writes"], 0)
        self.assertEqual(result["side"], "seed")
        base = environment.plan(self.backend, self.write("base-review.json", self.review), self.execution_source,
                                self.write("base-build.json", self.maintenance), "base")
        self.assertEqual((base["side"], base["scope_id"]), ("base", "fixture-base"))

    def test_public_plan_receives_the_exact_native_binding_and_writes_nothing(self):
        publish_execution_fixture(self)
        from rate_limit_tool import tool_binding, verified_tool
        import subprocess
        review = self.write("cli-review.json", self.review)
        fixture = self.execution_source
        maintenance = self.write("cli-build.json", self.maintenance)
        before = {str(path.relative_to(self.root)): path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        with verified_tool(tool_binding()) as executable:
            completed = subprocess.run([executable, "check", "recovery", "fixture", "environment", "plan",
                "--review", str(review), "--execution-source", str(fixture), "--maintenance-build", str(maintenance),
                "--side", "base"], cwd=self.backend, capture_output=True, check=False)
        self.assertEqual(completed.returncode, 0, completed.stderr.decode("utf-8", errors="replace"))
        lines = [line for line in completed.stdout.decode("utf-8").splitlines() if line.startswith("{")]
        self.assertEqual(len(lines), 1, completed.stdout.decode("utf-8", errors="replace"))
        self.assertEqual(json.loads(lines[0]), {"status": "planned", "services_started": False, "remote_writes": 0})
        self.assertEqual(before, {str(path.relative_to(self.root)): path.read_bytes() for path in self.root.rglob("*") if path.is_file()})

    def test_plan_rejects_historical_source_database(self):
        publish_execution_fixture(self)
        self.review["reference"]["source"]["databases"] = [{"database": "old"}]
        with self.assertRaisesRegex(ValueError, "历史来源"):
            environment.plan(self.backend, self.write("review.json", self.review), self.execution_source, self.write("build.json", self.maintenance))

    def test_plan_accepts_project_relative_evidence_paths(self):
        publish_execution_fixture(self)
        review = self.write("review.json", self.review)
        fixture = self.execution_source
        maintenance = self.write("build.json", self.maintenance)
        result = environment.plan(self.backend, review.relative_to(self.backend), fixture.relative_to(self.backend), maintenance.relative_to(self.backend))
        self.assertEqual(result["scope_id"], "fixture-seed")

    def test_current_and_recorded_review_tool_validation(self):
        predecessor = self.write("pending-tools.json", self.review)
        ready = self.static_ready(predecessor, self.root / "ready-tools.json")
        observed = self.observed_tools()
        environment._preflight_binding(ready)
        with patch.object(environment, "_preflight", return_value=observed):
            self.assertEqual(environment.validate_current_review_tools(ready), observed)
        with patch.object(environment, "_preflight", side_effect=AssertionError("冻结消费者不得重探本机工具")):
            self.assertEqual(environment.validated_recorded_review_tools(ready), observed)
        for role, field, changed in (("rustfs", "sha256", "f" * 64),
                                     ("redis_server", "version", "Redis server v=7.2.0")):
            drifted = copy.deepcopy(observed)
            drifted[role][field] = changed
            with (self.subTest(role=role),
                  patch.object(environment, "_preflight", return_value=drifted),
                  self.assertRaisesRegex(ValueError, "当前工具")):
                environment.validate_current_review_tools(ready)
        for field in ("mysqldump", "native_tool"):
            invalid = copy.deepcopy(ready)
            del invalid["preflight"]["tools"][field]
            with self.subTest(missing=field), self.assertRaises(ValueError):
                environment._preflight_binding(invalid)
        invalid = copy.deepcopy(ready)
        invalid["tools"]["native_tool"]["source_sha256"] = "f" * 64
        invalid["preflight"]["tools"]["native_tool"] = copy.deepcopy(invalid["tools"]["native_tool"])
        with self.assertRaises(ValueError):
            environment._preflight_binding(invalid)

    def test_review_requires_mysqldump_but_accepts_historical_extra_tools(self):
        for name in (*REVIEW_FILE_TOOLS, "redis_server"):
            with self.subTest(missing=name):
                value = copy.deepcopy(self.review)
                del value["tools"][name]
                with self.assertRaisesRegex(ValueError, "工具"):
                    validate_review(value)
        extra = copy.deepcopy(self.review)
        extra["tools"]["legacy-wrapper"] = {"path": "unused", "sha256": "f" * 64}
        validate_review(extra)

    def test_service_run_rejects_roots_outside_the_business_fixture(self):
        self.assertEqual(
            service_run(self.review),
            Path(self.review["future_root"]) / "service-run",
        )
        for value in (str(self.root / "outside"), str(Path(self.review["future_root"]) / "nested")):
            with self.subTest(value=value):
                invalid = dict(self.review, future_root=value)
                with self.assertRaisesRegex(ValueError, "future_root"):
                    service_run(invalid)

    def test_missing_binding_fails_before_any_process(self):
        import reference_fixture_environment_native as native
        with patch.object(native, "tool_binding", return_value=None), patch.object(native.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "正式原生工具绑定"):
                native.invoke("fixture", {})
        run.assert_not_called()

    def test_unknown_process_outcomes_do_not_retry_or_echo_input(self):
        import reference_fixture_environment_native as native
        import subprocess
        for error in (OSError("credential-marker"), subprocess.TimeoutExpired("credential-marker", 1)):
            with patch.object(native, "tool_binding", return_value={}), patch.object(native, "_run", side_effect=error) as run:
                with self.assertRaisesRegex(ValueError, "不自动重放") as raised:
                    native.invoke("reset-password", {"admin_password": "credential-marker"})
            self.assertNotIn("credential-marker", str(raised.exception))
            self.assertEqual(run.call_count, 1)

    def test_duplicate_response_keys_are_rejected(self):
        import reference_fixture_environment_native as native
        with self.assertRaisesRegex(ValueError, "重复字段"):
            json.loads('{"ok":true,"ok":false}', object_pairs_hook=native._unique_object)


if __name__ == "__main__":
    unittest.main()
