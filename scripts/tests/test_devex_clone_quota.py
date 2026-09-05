"""配额准备的纯前后像、未知结果和外层阶段门禁；不访问服务。"""
import copy
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import patch, Mock

import devex_clone_quota as quota
import devex_clone_quota_model as model
import devex_clone_seed_runtime as runtime
from devex_clone_capture import read_json
from devex_clone_run_state import binding, begin, finish
from restore_reference_plan import plan_hash
import test_devex_clone_seed as fixtures


class FakeBridge:
    def __init__(self, targets, *, satisfied=False):
        self.calls = []
        self.auth = {"subject_id": "101", "tenant_id": "system", "username": "admin", "is_super_admin": True, "permissions": []}
        self.rows = {}
        self.failure = None
        self.clock = 0
        for target in targets:
            self.rows[target["tenant_id"]] = {"tenant_id": target["tenant_id"], "status": "enabled", "name": "原租户",
                "domain": None, "expire_at": None, "max_users": target["max_users"] if satisfied else 20,
                "max_roles": 10, "max_storage_mb": 1024, "max_requests_per_min": 1000}

    def login(self, **_kwargs):
        self.calls.append(("login", None))
        return self.auth

    def usage(self, tenant):
        self.clock += 1
        row = self.rows[tenant]
        return {"tenant_id": tenant, "calculated_at": str(self.clock), "users": {"used": 2, "limit": row["max_users"]},
                "roles": {"used": 2, "limit": row["max_roles"]}, "request_window": {"current": self.clock}}

    def call(self, operation, **values):
        tenant = values.get("tenant_id")
        self.calls.append((operation, tenant))
        if operation == "close":
            return {"closed": True}
        if operation == "get":
            return {"code": 200, "message": "成功", "data": {**copy.deepcopy(self.rows[tenant]),
                "usage": self.usage(tenant), "expiration_status": "never", "capacity_status": "normal"}}
        if operation == "usage":
            return {"code": 200, "message": "成功", "data": self.usage(tenant)}
        if self.failure == "before":
            raise TimeoutError("未知 PUT")
        self.rows[tenant].update(copy.deepcopy(values["body"]))
        if self.failure in {"after", "refresh"}:
            raise TimeoutError("响应或刷新未知")
        auth = {**self.auth, "subject_id": "999"} if self.failure == "wrong-auth" else self.auth
        return {"response": {"code": 200, "message": "成功", "data": copy.deepcopy(self.rows[tenant])}, "authorization": auth}

    def close(self):
        self.calls.append(("producer-close", None))


class QuotaTests(unittest.TestCase):
    file = fixtures.SeedTests.file
    stage = fixtures.SeedTests.stage
    register = fixtures.SeedTests.register

    def setUp(self):
        fixtures.SeedTests.setUp(self)
        for index, group in enumerate(self.plan["groups"]):
            group.update(kind="system" if index == 0 else "tenant", count=100 if index == 0 else 10)
        self.plan["environment"].update(quota={"system_max_users": 200, "tenant_max_users": 100, "import_headroom_per_tenant": 60},
            tenants=[{"tenant_id": group["tenant_id"]} for group in self.plan["groups"][1:]])
        self.plan["plan_sha256"] = plan_hash({key: value for key, value in self.plan.items() if key != "plan_sha256"})
        Path(self.request["identity_plan"]["path"]).write_text(json.dumps(self.plan), encoding="utf-8")
        self.request["identity_plan"] = binding(Path(self.request["identity_plan"]["path"]))
        self.post["source_admin"] = {"subject_id": "101", "tenant_id": "system", "username": "admin"}
        (self.directory / "post-copy.json").write_text(json.dumps(self.post), encoding="utf-8")
        self.request["post_copy"] = binding(self.directory / "post-copy.json")
        self.request["post_verify"] = self.stage("post-copy", "verify", {"status": "post_copy_existing_data_verified",
            "registration": self.request["post_copy"], "worker_must_remain_stopped": True})
        self.register()
        self.context = SimpleNamespace(backend=self.backend, directory_root=self.directory, request=self.post, administrator=Mock())
        self.bridge = FakeBridge(model.targets(self.plan))
        self.guard = patch.object(quota, "guard")
        self.guard.start()
        self.addCleanup(self.guard.stop)

    def run_mode(self, mode, *, fail_outer=False):
        number = begin(self.directory, "seed-runtime", mode, {})
        self.context.output = self.directory / "seed-runtime" / f"attempt-{number:04d}"
        self.context.output.mkdir(parents=True)
        try:
            result = quota.execute_body(self.context, self.request, self.bridge, mode)
        except BaseException as error:
            finish(self.directory, number, error=error)
            raise
        finish(self.directory, number, result=result, error=ValueError("外层失败") if fail_outer else None)
        return result

    def plan_actions(self):
        self.run_mode("quotas-plan")
        return model.load_plan(self.backend, self.directory, self.request)

    def test_plan_keeps_all_original_responses_and_explicit_headroom(self):
        plan = self.plan_actions()
        self.assertEqual(len(plan["actions"]), 11)
        self.assertEqual(plan["actions"][1]["target"]["import_headroom"], 60)
        self.assertEqual(plan["actions"][0]["before"]["tenant"]["name"], "原租户")
        self.assertEqual(plan["actions"][0]["observed"]["tenant"]["message"], "成功")
        self.assertFalse(any(call[0] == "update" for call in self.bridge.calls))

    def test_fixed_groups_duplicates_and_invalid_desired_quota_fail(self):
        for mutate in (lambda p: p["groups"][0].update(count=99),
                       lambda p: p["environment"]["tenants"][0].update(tenant_id="system"),
                       lambda p: p["environment"]["quota"].update(tenant_max_users=True),
                       lambda p: p["environment"]["quota"].update(import_headroom_per_tenant=0)):
            plan = copy.deepcopy(self.plan)
            mutate(plan)
            with self.assertRaises(ValueError): model.targets(plan)

    def test_increase_must_cover_actual_users_roles_and_headroom(self):
        target = model.targets(self.plan)[1]
        observed = quota.observe(self.bridge, target)
        for mutate in (lambda t, o: t.update(max_users=10), lambda t, o: t.update(max_users=71),
                       lambda t, o: o["usage"]["data"]["roles"].update(used=10),
                       lambda t, o: o["tenant"]["data"].update(max_users=0)):
            desired, original = copy.deepcopy(target), copy.deepcopy(observed)
            mutate(desired, original)
            with self.assertRaises(ValueError): model.action(desired, original)

    def test_projection_ignores_volatile_usage_but_retains_all_editable_fields(self):
        item = self.plan_actions()["actions"][0]
        self.assertEqual(model.classification(item, quota.observe(self.bridge, item)), "before")
        self.bridge.rows["system"]["domain"] = "changed.example"
        self.assertEqual(model.classification(item, quota.observe(self.bridge, item)), "mismatch")

    def test_expired_wrong_tenant_and_missing_embedded_usage_are_rejected(self):
        item = self.plan_actions()["actions"][0]
        for mutate in (lambda o: o["tenant"]["data"].update(expiration_status="expired"),
                       lambda o: o["usage"]["data"].update(tenant_id="unknown"),
                       lambda o: o["tenant"]["data"].update(usage=None)):
            observed = quota.observe(self.bridge, item)
            mutate(observed)
            with self.assertRaises(ValueError): model.classification(item, observed)

    def test_plan_publication_retry_keeps_original_bytes_and_rejects_new_before(self):
        self.run_mode("quotas-plan", fail_outer=True)
        original = binding(model.plan_path(self.directory))
        with self.assertRaises(ValueError): model.load_plan(self.backend, self.directory, self.request)
        self.run_mode("quotas-plan")
        self.assertEqual(binding(model.plan_path(self.directory)), original)
        self.bridge.rows["system"]["name"] = "变化"
        with self.assertRaises(ValueError): self.run_mode("quotas-plan")
        self.assertEqual(binding(model.plan_path(self.directory)), original)

    def test_apply_eleven_once_preserves_other_fields_and_binds_capacity(self):
        plan = self.plan_actions()
        result = self.run_mode("quotas-apply")
        self.assertEqual(result["status"], "seed_quotas_verified")
        self.assertEqual(len([c for c in self.bridge.calls if c[0] == "update"]), 11)
        evidence = model.capacity_evidence(self.backend, self.directory, self.request)
        self.assertEqual(len(evidence["confirmed"]), 11)
        self.run_mode("quotas-apply")
        self.assertEqual(len([c for c in self.bridge.calls if c[0] == "update"]), 11)
        for item in plan["actions"]:
            self.assertEqual(self.bridge.rows[item["target"]["tenant_id"]], item["after"]["tenant"])

    def test_already_satisfied_is_readonly_without_fabricated_api_response(self):
        self.bridge = FakeBridge(model.targets(self.plan), satisfied=True)
        plan = self.plan_actions()
        self.run_mode("quotas-apply")
        self.assertFalse(any(c[0] == "update" for c in self.bridge.calls))
        for item in plan["actions"]:
            attempt = model.directories(self.directory, item)[1][-1]
            confirmed = model.confirmation(self.backend, self.directory, plan, item, attempt)
            self.assertFalse(confirmed["put_attempted"])
            self.assertFalse(confirmed["api_response_proven"])

    def test_unknown_after_requires_readonly_reconcile_then_skips_confirmed_put(self):
        plan = self.plan_actions()
        self.bridge.failure = "after"
        with self.assertRaises(TimeoutError): self.run_mode("quotas-apply")
        count = len(self.bridge.calls)
        with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        self.assertEqual(len(self.bridge.calls), count)
        self.bridge.failure = None
        reconciled = self.run_mode("quotas-reconcile")
        self.assertEqual(reconciled["status"], "seed_quotas_reconciled")
        first = model.directories(self.directory, plan["actions"][0])[1][-1]
        confirmed = model.confirmation(self.backend, self.directory, plan, plan["actions"][0], first)
        self.assertFalse(confirmed["api_response_proven"])
        with self.assertRaises(ValueError): model.capacity_evidence(self.backend, self.directory, self.request)
        self.run_mode("quotas-apply")
        self.assertEqual(len([c for c in self.bridge.calls if c[0] == "update"]), 11)

    def test_unknown_before_needs_explicit_reconcile_and_new_attempt(self):
        plan = self.plan_actions()
        self.bridge.failure = "before"
        with self.assertRaises(TimeoutError): self.run_mode("quotas-apply")
        with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        self.bridge.failure = None
        self.run_mode("quotas-reconcile")
        self.run_mode("quotas-apply")
        self.assertEqual(len(model.directories(self.directory, plan["actions"][0])[1]), 2)

    def test_mismatch_after_unknown_blocks_without_put_retry(self):
        self.plan_actions()
        self.bridge.failure = "after"
        with self.assertRaises(TimeoutError): self.run_mode("quotas-apply")
        self.bridge.failure = None
        self.bridge.rows["system"]["name"] = "其他写入"
        with self.assertRaises(ValueError): self.run_mode("quotas-reconcile")
        with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        self.assertEqual(len([c for c in self.bridge.calls if c[0] == "update"]), 1)

    def test_refresh_failure_or_wrong_identity_remains_unknown(self):
        self.plan_actions()
        self.bridge.failure = "wrong-auth"
        with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        self.assertEqual(len([c for c in self.bridge.calls if c[0] == "update"]), 1)

    def test_refresh_transport_failure_is_not_retried(self):
        self.plan_actions()
        self.bridge.failure = "refresh"
        with self.assertRaises(TimeoutError): self.run_mode("quotas-apply")
        with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        self.assertEqual(len([c for c in self.bridge.calls if c[0] == "update"]), 1)

    def test_confirmation_evidence_tampering_blocks_capacity(self):
        plan = self.plan_actions()
        self.run_mode("quotas-apply")
        first = model.directories(self.directory, plan["actions"][0])[1][-1]
        (first / "api-response.json").write_text('{}', encoding="utf-8")
        with self.assertRaises(ValueError): model.capacity_evidence(self.backend, self.directory, self.request)

    def test_unpublished_confirmation_after_guard_failure_can_reconcile(self):
        self.plan_actions()
        original = quota.confirm
        with patch.object(quota, "confirm", side_effect=ValueError("写入后守卫失败")):
            with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        self.assertIs(quota.confirm, original)
        self.run_mode("quotas-reconcile")
        self.run_mode("quotas-apply")
        self.assertEqual(len([c for c in self.bridge.calls if c[0] == "update"]), 11)

    def test_interrupted_noop_confirmation_can_resume_without_put(self):
        self.bridge = FakeBridge(model.targets(self.plan), satisfied=True)
        self.plan_actions()
        with patch.object(quota, "confirm", side_effect=ValueError("只读确认中断")):
            with self.assertRaises(ValueError): self.run_mode("quotas-apply")
        self.run_mode("quotas-apply")
        self.assertFalse(any(call[0] == "update" for call in self.bridge.calls))

    def test_reconcile_cannot_borrow_observation_from_another_intent(self):
        plan = self.plan_actions()
        self.bridge.failure = "before"
        with self.assertRaises(TimeoutError): self.run_mode("quotas-apply")
        self.bridge.failure = None
        self.run_mode("quotas-reconcile")
        attempt = model.directories(self.directory, plan["actions"][0])[1][-1]
        filename = sorted(attempt.glob("reconcile-*.json"))[-1]
        value = read_json(filename)
        value["observed"] = self.file("borrowed-observation", quota.observe(self.bridge, plan["actions"][0]))
        filename.write_text(json.dumps(value), encoding="utf-8")
        with self.assertRaises(ValueError): quota.reconciled_before(self.context, plan, plan["actions"][0], attempt)

    def test_fixture_uses_current_http_dto_instead_of_application_internal_shape(self):
        backend = next(parent for parent in Path(__file__).resolve().parents if (parent / "openapi/openapi.json").is_file())
        schema = json.loads((backend / "openapi/openapi.json").read_text(encoding="utf-8"))["components"]["schemas"]
        observed = quota.observe(self.bridge, model.targets(self.plan)[0])
        self.assertEqual(set(observed["tenant"]["data"]), set(schema["TenantCapacityVo"]["properties"]))
        self.assertEqual(model.FIELDS | {"tenant_id", "status"}, set(schema["TenantVo"]["properties"]))

    def test_failed_outer_cannot_use_old_success_or_confirmed_inner_receipts(self):
        self.plan_actions()
        self.run_mode("quotas-apply", fail_outer=True)
        with self.assertRaises(ValueError): model.capacity_evidence(self.backend, self.directory, self.request)
        self.run_mode("quotas-apply")
        model.capacity_evidence(self.backend, self.directory, self.request)
        self.stage("seed-runtime", "quotas-reconcile", error=ValueError("最新失败"))
        with self.assertRaises(ValueError): model.capacity_evidence(self.backend, self.directory, self.request)

    def test_identity_or_new_runtime_history_closes_all_quota_actions(self):
        model.before_identities(self.backend, self.directory, self.request)
        root = Path(self.request["identity_state"])
        root.mkdir(parents=True)
        with self.assertRaises(ValueError): model.before_identities(self.backend, self.directory, self.request)
        root.rmdir()
        self.stage("seed-runtime", "identities-apply", error=ValueError("容量门禁提前拒绝，无生产者"))
        model.before_identities(self.backend, self.directory, self.request)
        number = begin(self.directory, "seed-runtime", "identities-apply", {})
        output = self.directory / "seed-runtime" / f"attempt-{number:04d}"
        output.mkdir(parents=True)
        (output / "session-launch.json").write_text('{}', encoding="utf-8")
        finish(self.directory, number, error=ValueError("未知身份生产者"))
        with self.assertRaises(ValueError): model.before_identities(self.backend, self.directory, self.request)

    def test_identity_verify_department_gate_runs_before_any_guard_or_producer(self):
        with patch.object(runtime, "current_guard") as guard, patch.object(runtime, "Producer") as producer:
            with self.assertRaises(ValueError):
                runtime.identity_run(self.context, self.request, "verify")
        guard.assert_not_called()
        producer.assert_not_called()

    def test_shutdown_failure_does_not_mask_unknown_write(self):
        self.context.output = self.directory / "seed-runtime/attempt-0004"
        original = TimeoutError("PUT 未知")
        bridge = Mock()
        bridge.call.side_effect = ValueError("注销失败")
        bridge.close.side_effect = ValueError("关闭失败")
        with patch.object(quota, "Bridge", return_value=bridge), patch.object(quota, "execute_body", side_effect=original), patch.object(quota, "cleanup_failure"):
            with self.assertRaises(TimeoutError) as caught: quota.execute_quotas(self.context, self.request, "quotas-apply")
        self.assertIs(caught.exception, original)


if __name__ == "__main__":
    unittest.main()
