"""身份和新运行登记的离线边界；不连接实际资源。"""
import copy
import hashlib
import json
from pathlib import Path
import shutil
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch, Mock

import devex_clone_seed as seed
import devex_clone_seed_runtime as runtime
from devex_clone_capture import read_json, write_json
from devex_clone_post import FIELDS as POST_FIELDS
from devex_clone_run_state import binding, initialize_state, begin, finish, load_state
from restore_reference_plan import plan_hash


class SeedTests(unittest.TestCase):
    def setUp(self):
        repository = next(path for path in Path(__file__).resolve().parents if (path / "Cargo.toml").is_file())
        temporary = tempfile.TemporaryDirectory(dir=repository / ".local-tests/tmp", prefix="seed-evidence-")
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve() / "backend"
        self.directory = self.backend / ".local-tests/run"
        self.directory.mkdir(parents=True)
        self.old = self.directory / "post-copy/runtime"
        self.old.mkdir(parents=True)
        self.scope = "seed-scope"
        self.api = {"APP_ENV": "test", "APP_SCOPE_ID": self.scope, "APP_JOBS_MODE": "external",
                    "APP_CORS_ALLOW_ORIGINS": "http://127.0.0.1:4190", "RYFRAME_ADMIN": "private-admin"}
        self.identity = {"pid": 991, "started": "222", "executable": str(self.backend / "api.exe")}
        self.file("manifest", {"copy_directory": str(self.directory / "copy")})
        initialize_state(self.directory)
        copied = self.stage("copy", "resume", {"status": "data_steps_verified"})
        generic = self.file("generic", {"safe": True})
        self.post = {field: generic for field in POST_FIELDS}
        self.post.update(format_version=1, kind="devex-clone-post-copy", run_manifest=binding(self.directory / "manifest.json"),
            copy_stage_receipt=copied, copy_result=self.file("copy-result", {"status": "data_steps_verified"}),
            ledger_head=self.file("copy-head", {"sequence": 8}), api_environment=self.file("api-environment", {"environment": self.api}),
            frontend_root=str(self.backend.parent / "ryframe-vue3"), node={"path": str(self.backend / "node.exe"), "sha256": "1" * 64},
            source_admin={"subject_id": "101", "tenant_id": "system", "username": "admin"})
        self.file("post-copy", self.post)
        self.stage("post-copy", "register", {"status": "post_copy_registered", "registration": binding(self.directory / "post-copy.json")})
        post_verified = self.stage("post-copy", "verify", {"status": "post_copy_existing_data_verified",
            "registration": binding(self.directory / "post-copy.json"), "worker_must_remain_stopped": True})
        write_json(self.old / "runtime.json", {"scope_id": self.scope})
        write_json(self.old / "api.json", {"format_version": 1, "scope_id": self.scope, "role": "api", "identity": self.identity})
        write_json(self.old / "handoff.json", {"api_url": "http://127.0.0.1:18210"})
        document = self.file("environment-document", {"machine": "fixture"})
        groups = []
        for index in range(11):
            slot = "system" if index == 0 else f"tenant-{index:02d}"
            users = [{"user_slot": f"{slot}-{n:03d}", "username": f"user-{n:03d}",
                      "password_env": "RYFRAME_USERS", "client_address": f"198.18.{index + 40}.{n + 1}"}
                     for n in range(100 if index == 0 else 10)]
            permissions = (["monitor:schedule:add", "monitor:schedule:run", "system:message:publish",
                            "system:post:export"] if index == 0 else
                           ["system:post:add", "system:post:edit", "system:post:export", "system:post:list",
                            "system:post:remove", "system:user-import:add", "system:user-import:list"])
            groups.append({"slot": slot, "kind": "system" if index == 0 else "tenant",
                "tenant_id": slot, "role_code": "devex-system" if index == 0 else "devex-tenant",
                "permissions": permissions,
                "users": users, "admin": {"password_env": "RYFRAME_ADMIN"}})
        self.plan = {"format_version": 1, "kind": "devex-identities", "groups": groups, "environment": {
            "scope_id": self.scope, "backend_dir": str(self.backend), "frontend_dir": self.post["frontend_root"],
            "api_url": "http://127.0.0.1:18210", "frontend_url": "http://127.0.0.1:4190",
            "runtime": {"directory": str(self.old), "receipt_sha256": binding(self.old / "runtime.json")["sha256"],
                        "api_process_sha256": binding(self.old / "api.json")["sha256"]},
            "environment_document": {key: document[key] for key in ("path", "sha256")},
            "passwords": {"system_env": "RYFRAME_USERS", "tenant_env": "RYFRAME_USERS"}}}
        self.plan["plan_sha256"] = plan_hash(self.plan)
        self.request = {"format_version": 1, "kind": "devex-clone-seed-runtime",
            "run_manifest": binding(self.directory / "manifest.json"), "post_copy": binding(self.directory / "post-copy.json"),
            "post_verify": post_verified, "node": self.post["node"], "identity_plan": self.file("identity-plan", self.plan),
            "identity_state": str(self.directory / "seed-runtime/identities"),
            "identity_environment": self.file("identity-environment", {"environment": {**self.api, "RYFRAME_USERS": "private-users"}}),
            "api_process": binding(self.old / "api.json")}

    def file(self, name, value):
        path = self.directory / (name + ".json")
        path.write_text(json.dumps(value), encoding="utf-8")
        return binding(path)

    def stage(self, stage, mode, result=None, error=None):
        number = begin(self.directory, stage, mode, {})
        finish(self.directory, number, result=result, error=error)
        return load_state(self.directory)["attempts"][-1]["result"]

    def register(self):
        request_file = self.file("seed-request", self.request)
        number = begin(self.directory, "seed-runtime", "register", {})
        result = seed.register(self.backend, self.directory, Path(request_file["path"]))
        finish(self.directory, number, result=result)
        return result

    def evidence(self):
        root = Path(self.request["identity_state"])
        root.mkdir(parents=True, exist_ok=True)
        ledger = {"format_version": 1, "plan_sha256": self.plan["plan_sha256"], "status": "verified",
                  "entries": [], "roles": {}, "users": {},
                  "publication_nonce": "10000000-0000-4000-8000-000000000001"}
        identities = []
        next_id = 1000
        next_request = 10000
        for group in self.plan["groups"]:
            role_id = str(next_id)
            next_id += 1
            ledger["roles"][group["slot"]] = role_id
            ledger["entries"].extend((
                {"operation": "post_system_roles", "slot": group["slot"], "phase": "confirmed", "id": role_id},
                {"operation": "put_system_roles_by_id_permissions", "slot": group["slot"],
                 "phase": "confirmed", "id": role_id}))
            for user in group["users"]:
                user_id = str(next_id)
                next_id += 1
                ledger["users"][user["user_slot"]] = user_id
                ledger["entries"].extend((
                    {"operation": "post_system_users", "slot": user["user_slot"],
                     "phase": "confirmed", "id": user_id},
                    {"operation": "post_system_users_by_id_password_reset_requests", "slot": user["user_slot"],
                     "phase": "confirmed", "id": user_id, "request_id": str(next_request)},
                    {"operation": "post_auth_password_reset_complete", "slot": user["user_slot"],
                     "phase": "confirmed", "id": user_id}))
                next_request += 1
                identities.append({"tenant_slot": group["slot"], "user_slot": user["user_slot"],
                    "tenant_id": group["tenant_id"], "username": user["username"],
                    "client_address": user["client_address"], "password_env": user["password_env"],
                    "user_id": user_id, "roles_sha256": plan_hash([group["role_code"]]),
                    "permissions_sha256": plan_hash(sorted(group["permissions"]))})
        templates = []
        department_path = "0/基准部门"
        for index, group in enumerate(self.plan["groups"][1:], 1):
            path = root / f"template-{group['slot']}-00000000-0000-4000-8000-{index:012x}.xlsx"
            path.write_bytes(f"fixture-template-{index}".encode())
            templates.append({"tenant_slot": group["slot"], "path": str(path.resolve()),
                "template_sha256": binding(path)["sha256"], "department_path": department_path,
                "department_sha256": hashlib.sha256(department_path.encode()).hexdigest()})
        audience = [{"kind": "user", "target_id": item["user_id"]}
                    for item in identities if item["tenant_slot"] == "system"][:10]
        receipt = {"format_version": 1, "plan_sha256": self.plan["plan_sha256"], "scope_id": self.scope,
                   "status": "verified", "identities": identities,
                   "identity_pools": seed._expected_pools(identities), "templates": templates,
                   "message_audience": audience, "audience_sha256": plan_hash(audience),
                   "homepage_user_slots": [item["user_slot"] for item in identities
                       if item["tenant_slot"] != "system" and item["user_slot"].endswith("-001")]}
        intent_name, receipt_name = seed._publication_names(self.plan["plan_sha256"], root)
        ledger["verified_receipt"] = receipt_name
        write_json(root / "ledger.json", ledger)
        write_json(root / receipt_name, receipt)
        stored = read_json(root / "ledger.json")
        prepared = {"format_version": stored["format_version"], "plan_sha256": stored["plan_sha256"],
                    "status": "prepared", "entries": stored["entries"], "roles": stored["roles"],
                    "users": stored["users"]}
        write_json(root / intent_name, {"format_version": 1,
            "kind": "devex-identity-verification-publication", "plan_sha256": self.plan["plan_sha256"],
            "identity_state": str(root.resolve()), "publication_nonce": ledger["publication_nonce"],
            "ledger_sha256": seed._encoded_sha256(prepared),
            "receipt_file": receipt_name, "receipt_sha256": binding(root / receipt_name)["sha256"],
            "receipt": receipt})
        return seed.identity_evidence(self.backend, self.directory, self.request)

    def publication(self):
        root = Path(self.request["identity_state"])
        ledger = read_json(root / "ledger.json")
        receipt = root / ledger["verified_receipt"]
        intent = root / seed._publication_names(self.plan["plan_sha256"], root)[0]
        return root, ledger, intent, receipt

    @staticmethod
    def replace_json(path, value):
        path.write_text(json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2) + "\n", encoding="utf-8")

    def replace_receipt_and_intent(self, change):
        _, _, intent_path, receipt_path = self.publication()
        receipt = read_json(receipt_path)
        change(receipt)
        self.replace_json(receipt_path, receipt)
        intent = read_json(intent_path)
        intent["receipt"], intent["receipt_sha256"] = receipt, binding(receipt_path)["sha256"]
        self.replace_json(intent_path, intent)

    def test_register_preserves_old_api_and_does_not_authorize_worker(self):
        old = binding(self.old / "api.json")
        result = self.register()
        self.assertFalse(result["worker_authorized"])
        self.assertEqual(old, binding(self.old / "api.json"))
        self.assertEqual(seed.registered(self.backend, self.directory), self.request)
        self.assertFalse((self.directory / "seed-runtime/runtime").exists())

    def test_history_allows_legitimate_identity_and_outbox_data_evolution(self):
        self.register()
        self.file("current-business-image", {"identity_count": 200, "outbox_delivered": 123})
        self.assertEqual(seed.history(self.backend, self.directory, self.request), self.post)

    def test_wrong_run_scope_runtime_and_old_post_evidence_rejected(self):
        for key in ("post_copy", "post_verify", "run_manifest"):
            with self.subTest(key=key), self.assertRaises(ValueError):
                seed.history(self.backend, self.directory, {**self.request, key: self.file("wrong", {})})
        for key, value in (("scope_id", "other-scope"), ("api_url", "http://127.0.0.1:18211"),
                           ("frontend_url", "http://127.0.0.1:4191")):
            plan = copy.deepcopy(self.plan)
            plan["environment"][key] = value
            plan["plan_sha256"] = plan_hash({k: v for k, v in plan.items() if k != "plan_sha256"})
            request = {**self.request, "identity_plan": self.file("wrong-plan", plan)}
            with self.subTest(key=key), self.assertRaises(ValueError):
                seed.identity_inputs(self.backend, self.directory, request, self.post)

    def test_identity_inputs_use_the_bound_post_amendment_runtime(self):
        runtime = self.directory / "post-copy/runtime-0001"
        runtime.mkdir(parents=True)
        write_json(runtime / "runtime.json", {"scope_id": self.scope})
        write_json(runtime / "api.json", {"format_version": 1, "scope_id": self.scope,
                                           "role": "api", "identity": self.identity})
        write_json(runtime / "handoff.json", {"api_url": "http://127.0.0.1:18210"})
        amendment = self.file("post-amendment", {"sequence": 1})
        plan = copy.deepcopy(self.plan)
        plan["environment"]["runtime"] = {"directory": str(runtime),
            "receipt_sha256": binding(runtime / "runtime.json")["sha256"],
            "api_process_sha256": binding(runtime / "api.json")["sha256"]}
        plan["plan_sha256"] = plan_hash({key: value for key, value in plan.items() if key != "plan_sha256"})
        request = {**self.request, "post_copy": amendment, "identity_plan": self.file("amended-plan", plan),
                   "api_process": binding(runtime / "api.json")}
        active = SimpleNamespace(descriptor=amendment, request=self.post, runtime=runtime)
        with patch.object(seed, "post_registration", return_value=active):
            observed, private = seed.identity_inputs(self.backend, self.directory, request, self.post)
        self.assertEqual(observed, plan)
        self.assertEqual(private["APP_CORS_ALLOW_ORIGINS"], "http://127.0.0.1:4190")

    def test_later_failed_copy_or_post_does_not_select_old_success(self):
        for stage, mode in (("copy", "reconcile"), ("post-copy", "verify")):
            with self.subTest(stage=stage):
                self.stage(stage, mode, error=ValueError("fixture"))
                with self.assertRaises(ValueError):
                    seed.history(self.backend, self.directory, self.request)

    def test_identity_private_environment_allows_only_explicit_secrets(self):
        for update in ({"APP_SCOPE_ID": "other"}, {"PATH_EXTRA": "unbound"}, {"RYFRAME_USERS": ""},
                       {"APP_UNKNOWN": "new"}, {"RYFRAME_ADMIN": "changed"}):
            private = {**self.api, "RYFRAME_USERS": "private-users", **update}
            request = {**self.request, "identity_environment": self.file("wrong-private", {"environment": private})}
            with self.subTest(update=list(update)), self.assertRaises(ValueError):
                seed.identity_inputs(self.backend, self.directory, request, self.post)

    def test_unknown_identity_writes_or_stale_receipt_block_worker(self):
        self.evidence()
        root = Path(self.request["identity_state"])
        ledger = read_json(root / "ledger.json")
        for update in ({"status": "needs-reconciliation"}, {"entries": [{"phase": "started"}]},
                       {"verified_receipt": "../verified.json"}, {"plan_sha256": "a" * 64}):
            (root / "ledger.json").write_text(json.dumps({**ledger, **update}), encoding="utf-8")
            with self.subTest(update=update), self.assertRaises(ValueError):
                seed.identity_evidence(self.backend, self.directory, self.request)
        (root / "ledger.json").write_text(json.dumps(ledger), encoding="utf-8")
        (root / "lock").write_bytes(b"unreleased")
        with self.assertRaises(ValueError):
            seed.identity_evidence(self.backend, self.directory, self.request)

    def test_verified_ledger_requires_ordered_622_projection_and_publication_nonce(self):
        self.evidence()
        root, ledger, _, _ = self.publication()
        cases = []
        empty = copy.deepcopy(ledger)
        empty["entries"] = []
        cases.append(empty)
        reordered = copy.deepcopy(ledger)
        reordered["entries"][0], reordered["entries"][1] = reordered["entries"][1], reordered["entries"][0]
        cases.append(reordered)
        missing_nonce = copy.deepcopy(ledger)
        missing_nonce.pop("publication_nonce")
        cases.append(missing_nonce)
        for value in cases:
            with self.subTest(entries=len(value["entries"]), nonce="publication_nonce" in value):
                self.replace_json(root / "ledger.json", value)
                with self.assertRaises(ValueError):
                    seed.identity_evidence(self.backend, self.directory, self.request)
        self.replace_json(root / "ledger.json", ledger)
        self.assertEqual(seed.identity_evidence(self.backend, self.directory, self.request)["ledger"],
                         binding(root / "ledger.json"))

    def test_publication_requires_one_intent_receipt_and_no_pending_or_replacement(self):
        self.evidence()
        root, _, intent, receipt = self.publication()
        intent_bytes, receipt_bytes = intent.read_bytes(), receipt.read_bytes()
        intent.unlink()
        with self.assertRaises(ValueError):
            seed.identity_evidence(self.backend, self.directory, self.request)
        intent.write_bytes(intent_bytes)
        pending = root / (intent.name + ".pending")
        pending.write_bytes(intent_bytes)
        with self.assertRaises(ValueError):
            seed.identity_evidence(self.backend, self.directory, self.request)
        pending.unlink()
        extra = root / "verified-00000000-0000-5000-8000-000000000000.json"
        extra.write_text("{}\n", encoding="utf-8")
        with self.assertRaises(ValueError):
            seed.identity_evidence(self.backend, self.directory, self.request)
        extra.unlink()
        receipt.write_text('{"replacement":true}\n', encoding="utf-8")
        with self.assertRaises(ValueError):
            seed.identity_evidence(self.backend, self.directory, self.request)
        receipt.write_bytes(receipt_bytes)
        self.assertIn("publication_intent", seed.identity_evidence(
            self.backend, self.directory, self.request))

    def test_receipt_full_semantics_are_checked_after_matching_intent_digest(self):
        self.evidence()
        _, _, intent, receipt = self.publication()
        original_intent, original_receipt = intent.read_bytes(), receipt.read_bytes()
        changes = (
            lambda value: value["identities"].reverse(),
            lambda value: value.update(identity_pools={}),
            lambda value: value.update(message_audience=[], audience_sha256=plan_hash([])),
            lambda value: value.update(homepage_user_slots=[]),
            lambda value: value["templates"][0].update(department_sha256="0" * 64),
        )
        for index, change in enumerate(changes):
            with self.subTest(index=index):
                self.replace_receipt_and_intent(change)
                with self.assertRaises(ValueError):
                    seed.identity_evidence(self.backend, self.directory, self.request)
                receipt.write_bytes(original_receipt)
                intent.write_bytes(original_intent)

    def test_publication_copied_to_another_identity_root_is_rejected(self):
        self.evidence()
        root, _, _, _ = self.publication()
        copied = self.backend / ".local-tests/run/seed-runtime/copied-identities"
        shutil.copytree(root, copied)
        request = {**self.request, "identity_state": str(copied.resolve())}
        with self.assertRaises(ValueError):
            seed.identity_evidence(self.backend, self.directory, request)

    def test_identity_outer_failure_requires_explicit_verify_same_ledger(self):
        self.register()
        evidence = self.evidence()
        departments = {"fixture_departments": True}
        producer = self.file("producer", {"identity": self.identity})
        result = {"status": "seed_identities_prepared", "registration": binding(self.directory / "seed-runtime.json"),
                  "identity": {"state": "prepared"}, "producer": producer}
        with patch("devex_clone_department_model.department_evidence", return_value=departments):
            self.stage("seed-runtime", "identities-apply", result, ValueError("outer failure"))
            with self.assertRaises(ValueError):
                seed.verified_identity_stage(self.backend, self.directory, self.request)
            result = {**result, "status": "seed_identities_verified", "identity": evidence,
                      "departments": departments}
            self.stage("seed-runtime", "identities-verify", result)
            self.assertEqual(seed.verified_identity_stage(self.backend, self.directory, self.request)["identity"], evidence)
            self.stage("seed-runtime", "identities-verify", result, ValueError("new failure"))
            with self.assertRaises(ValueError):
                seed.verified_identity_stage(self.backend, self.directory, self.request)

    def test_old_api_must_really_stop_and_pid_reuse_rejected(self):
        context = SimpleNamespace(backend=self.backend, runtime=self.old, target_config={"scope_id": self.scope})
        for observed in (self.identity, {**self.identity, "started": "333"}):
            with patch.object(runtime, "process_identity", return_value=observed), self.assertRaises(ValueError):
                runtime.old_api_stopped(context, self.request)
        with patch.object(runtime, "process_identity", return_value=None):
            runtime.old_api_stopped(context, self.request)

    def test_current_schedules_unknown_or_enabled_rejected_before_control(self):
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, target={}, output=self.directory,
            selected={}, review={}, run=Mock(), target_config={"databases": [{"key": "shared-control"}]},
            environments=runtime.Environments({}, {}))
        resources = SimpleNamespace(tools=SimpleNamespace(mysql=Mock(return_value="0")))
        with patch.object(runtime, "verify_confirmations", return_value=["confirmed"]), patch.object(runtime, "require_schedule_stage"), patch.object(runtime, "Resources", return_value=resources):
            self.assertEqual(runtime.schedules_guard(context), ["confirmed"])
            for raw in ("", "1", "0\n0", "NULL"):
                resources.tools.mysql.return_value = raw
                with self.subTest(raw=raw), self.assertRaises(ValueError):
                    runtime.schedules_guard(context)

    def test_stop_uses_new_runtime_after_later_failure_and_tool_changes(self):
        self.register()
        root = self.directory / "seed-runtime/runtime"
        root.mkdir(parents=True)
        write_json(root / "runtime.json", {"scope_id": self.scope})
        write_json(root / "binaries.json", {"ryframe": "fixture.exe"})
        handoff = {"registration": binding(self.directory / "seed-runtime.json"), "api_url": "http://127.0.0.1:18210",
                   "runtime": binding(root / "runtime.json"), "binaries": binding(root / "binaries.json")}
        write_json(root / "handoff.json", handoff)
        self.stage("seed-runtime", "prepare", {"status": "seed_runtime_prepared", "handoff": binding(root / "handoff.json")})
        self.stage("seed-runtime", "start", error=ValueError("outer failed after start"))
        self.file("identity-plan", {"later_tool_unavailable": True})
        with patch("devex_clone_runtime.control", return_value={"stopped": True}) as control, patch.object(runtime, "require_quiet", side_effect=AssertionError("cleanup must not depend on Node or source")):
            result = runtime.execute_seed(self.backend, self.directory, None, "stop", 8)
            self.assertEqual(result["status"], "seed_runtime_stop")
            self.assertEqual(control.call_args.args[1:4], (root, "stop", ("api", "worker")))
        self.assertEqual(binding(self.old / "api.json"), self.request["api_process"])

    def test_failed_start_cannot_be_masked_by_successful_stop(self):
        self.stage("seed-runtime", "start", error=ValueError("failed"))
        self.stage("seed-runtime", "stop", {"status": "stopped"})
        context = SimpleNamespace(directory_root=self.directory, backend=self.backend)
        with patch("devex_clone_runtime.control") as control, self.assertRaises(ValueError):
            runtime.start(context, self.request, 999)
        control.assert_not_called()

    def test_stale_templates_or_user_receipt_rejected(self):
        self.evidence()
        root = Path(self.request["identity_state"])
        receipt = read_json(root / read_json(root / "ledger.json")["verified_receipt"])
        Path(receipt["templates"][0]["path"]).write_bytes(b"changed")
        with self.assertRaises(ValueError):
            seed.identity_evidence(self.backend, self.directory, self.request)


if __name__ == "__main__":
    unittest.main()
