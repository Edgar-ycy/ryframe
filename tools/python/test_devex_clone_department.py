"""固定部门阶段的空前像、幂等写入、核对和身份门禁。"""
import base64
import copy
import hashlib
import io
import json
from pathlib import Path
import shutil
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch
import zipfile

import devex_clone_department as department
import devex_clone_department_model as model
import devex_clone_seed_runtime as runtime
import devex_clone_post_process as process
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import begin, binding, finish, load_state
from restore_reference_plan import plan_hash
import test_devex_clone_seed as fixtures


class FakeBridge:
    def __init__(self, targets):
        self.targets = {item["tenant_id"]: item for item in targets}
        self.rows = {tenant: [] for tenant in self.targets}
        self.subjects = {tenant: str(9000 + index) for index, tenant in enumerate(self.targets, 1)}
        self.calls = []
        self.failure = None
        self.next_id = 5000
        self.template_extra = {tenant: [] for tenant in self.targets}
        self.template_identities = {}
        self.template_subjects = {}

    def login(self, **kwargs):
        identities = kwargs["template_identities"]
        self.template_identities = {item["tenant_id"]: copy.deepcopy(item) for item in identities}
        for item in identities:
            self.template_subjects.setdefault(item["tenant_id"], item["subject_id"])
        first = next(iter(self.targets.values()))
        return {"scope_id": "seed-scope", "tenant_ids": list(self.targets),
                "department_name": first["body"]["name"],
                "template_principals": [model.principal(item) for item in identities]}

    def authorization(self, tenant, *, write=False):
        target = self.targets[tenant]
        permissions = ["system:dept:list"] + (["system:dept:add"] if write else [])
        return {"subject_id": self.subjects[tenant], "tenant_id": tenant,
                "username": target["admin"]["username"], "is_super_admin": False,
                "permissions": permissions}

    def template_authorization(self, tenant):
        identity = self.template_identities[tenant]
        return {"subject_id": self.template_subjects[tenant], "tenant_id": tenant,
                "username": identity["username"], "is_super_admin": False,
                "permissions": [model.TEMPLATE_PERMISSION]}

    @staticmethod
    def page(rows):
        return {"code": 200, "data": {"items": copy.deepcopy(rows), "page": 1, "page_size": 100,
                "total": len(rows), "total_pages": 1 if rows else 0, "max_page_size": 100}}

    def template(self, tenant):
        rows = self.rows[tenant]
        paths = sorted([item["name"] for item in rows
                        if item["parent_id"] is None and item["ancestors"] == "0"
                        and item["status"] == "1"] + self.template_extra[tenant])
        cells = "".join(
            f'<row r="{index}"><c r="A{index}" t="inlineStr"><is><t>{value}</t></is></c></row>'
            for index, value in enumerate(paths, 2))
        sheet = (f'<?xml version="1.0" encoding="UTF-8"?>'
                 f'<worksheet xmlns="{model.SHEET_NS}"><sheetData>{cells}</sheetData></worksheet>').encode()
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("xl/worksheets/sheet2.xml", sheet)
        content = output.getvalue()
        return {"bytes": len(content),
                "media_type": "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
                "sha256": hashlib.sha256(content).hexdigest(),
                "base64": base64.b64encode(content).decode()}

    def call(self, operation, **values):
        if operation == "close":
            return {"closed": True}
        tenant = values["tenant_id"]
        self.calls.append((operation, tenant, copy.deepcopy(values)))
        if operation == "list":
            return {"response": self.page(self.rows[tenant]),
                    "authorization": self.authorization(tenant)}
        if operation == "detail":
            row = next(item for item in self.rows[tenant] if item["id"] == values["department_id"])
            return {"response": {"code": 200, "data": copy.deepcopy(row)},
                    "authorization": self.authorization(tenant)}
        if operation == "template":
            return {"response": self.template(tenant),
                    "authorization": self.template_authorization(tenant)}
        if operation != "create":
            raise AssertionError(operation)
        if self.failure == "before":
            raise TimeoutError("POST 结果未知")
        self.next_id += 1
        body = values["body"]
        row = {"id": str(self.next_id), "name": body["name"], "parent_id": None,
               "ancestors": "0", "sort": 0, "status": "1", "remark": None,
               "created_at": "2026-09-05T00:00:00Z"}
        self.rows[tenant].append(row)
        if self.failure == "after":
            raise TimeoutError("POST 后响应未知")
        return {"response": {"code": 200, "data": copy.deepcopy(row)},
                "authorization": self.authorization(tenant, write=True)}


class DepartmentGuardTests(unittest.TestCase):
    def test_quick_guard_reuses_current_full_stage_confirmation(self):
        context = SimpleNamespace(backend=Path("backend"), directory_root=Path("run"))
        request, prerequisite = {"seed": True}, {"department": True}
        with patch("devex_clone_seed_runtime.current_guard", return_value=["schedule-proof"]) as full, \
                patch("devex_clone_seed_runtime.current_mutation_guard") as mutation, \
                patch.object(department, "require_prerequisite"):
            department.guard(context, request, prerequisite)
            department.guard(context, request, prerequisite, quick=True)
        full.assert_called_once_with(context, request, old_api=True)
        mutation.assert_called_once_with(context, request, prerequisite, ["schedule-proof"])


class DepartmentTests(unittest.TestCase):
    file = fixtures.SeedTests.file
    stage = fixtures.SeedTests.stage
    register = fixtures.SeedTests.register

    def setUp(self):
        fixtures.SeedTests.setUp(self)
        tenants = []
        for index, group in enumerate(self.plan["groups"]):
            slot = group["slot"]
            admin = {"tenant_id": group["tenant_id"], "username": "admin",
                     "password_env": "RYFRAME_ADMIN", "client_address": f"198.18.70.{index + 1}"}
            permissions = (["monitor:schedule:add"] if index == 0 else
                           ["system:post:list", model.TEMPLATE_PERMISSION])
            group.update(kind="system" if index == 0 else "tenant", admin=admin,
                         permissions=permissions)
            if index:
                tenants.append({"slot": slot, "tenant_id": group["tenant_id"],
                                "target_key": "shared", "admin": admin})
        self.plan["environment"].update(plan_id="fixture", system_admin=self.plan["groups"][0]["admin"],
                                        tenants=tenants)
        self.plan["plan_sha256"] = plan_hash(
            {key: value for key, value in self.plan.items() if key != "plan_sha256"})
        plan_path = Path(self.request["identity_plan"]["path"])
        plan_path.write_text(json.dumps(self.plan), encoding="utf-8")
        self.request["identity_plan"] = binding(plan_path)
        self.register()
        self.context = SimpleNamespace(backend=self.backend, directory_root=self.directory,
            request=self.post, administrator=Mock(), output=None)
        self.targets = model.targets(self.plan)
        self.bridge = FakeBridge(self.targets)
        self.prepared_identity()
        self.guard = patch.object(department, "guard")
        self.guard.start()
        self.addCleanup(self.guard.stop)

    def prepared_identity(self):
        root = Path(self.request["identity_state"])
        root.mkdir(parents=True, exist_ok=True)
        if (root / "ledger.json").exists():
            return model.identity_projection(self.backend, self.request, allow_absent=False)
        roles = {group["slot"]: str(10000 + index) for index, group in enumerate(self.plan["groups"])}
        user_slots = [user["user_slot"] for group in self.plan["groups"] for user in group["users"]]
        users = {slot: str(20000 + index) for index, slot in enumerate(user_slots)}
        entries = []
        for index, value in enumerate(model.expected_entries(self.plan)):
            identifier = roles.get(value["slot"], users.get(value["slot"]))
            entry = {**value, "id": identifier}
            if value["operation"] == "post_system_users_by_id_password_reset_requests":
                entry["request_id"] = str(30000 + index)
            entries.append(entry)
        write_json(root / "ledger.json", {"format_version": 1, "plan_sha256": self.plan["plan_sha256"],
            "status": "prepared", "entries": entries, "roles": roles, "users": users})
        return model.identity_projection(self.backend, self.request, allow_absent=False)

    def run_mode(self, mode, *, fail_outer=False):
        number = begin(self.directory, "seed-runtime", mode, {})
        self.context.output = self.directory / "seed-runtime" / f"attempt-{number:04d}"
        self.context.output.mkdir(parents=True)
        try:
            result = department.execute_body(self.context, self.request, self.bridge, mode)
        except BaseException as error:
            finish(self.directory, number, error=error)
            raise
        finish(self.directory, number, result=result,
               error=ValueError("外层失败") if fail_outer else None)
        return result

    @staticmethod
    def execution_sources(*, product="1", tools="2", support="3"):
        return {"snapshot": {"head": "fixture", "patch_sha256": "4" * 64,
                             "files": [], "clean": False},
                "worktree_fingerprint": "sha256:" + "5" * 64,
                "fingerprints": {
                    "product": {"sha256": product * 64, "files": 10},
                    "test_tools": {"sha256": tools * 64, "files": 20},
                    "support": {"sha256": support * 64, "files": 5}}}

    def write_controller(self, number, identity):
        attempt = load_state(self.directory)["attempts"][number - 1]
        original = {**attempt, "status": "running", "finished_at": None,
                    "result": None, "error_type": None}
        owner = {"format_version": 1, "identity": identity, "directory": str(self.directory),
                 "manifest_sha256": binding(self.directory / "manifest.json")["sha256"]}
        value = {"format_version": 1, "kind": "devex-stage-controller", "owner": owner,
                 "attempt": number, "attempt_sha256": plan_hash(original)}
        write_json(self.directory / f"controller-{number:04d}.json", value)
        return value

    def write_failure(self, number):
        attempt = load_state(self.directory)["attempts"][number - 1]
        write_json(self.directory / f"failure-{number:04d}.json", {
            "format_version": 1, "kind": "devex-stage-failure", "attempt": number,
            "stage": "seed-runtime", "mode": "departments-plan", "error_type": attempt["error_type"],
            "frames": [{"file": "tools/python/devex_clone_run.py", "function": "execute", "line": 1}],
            "controller": binding(self.directory / f"controller-{number:04d}.json")})

    def write_guard_failure(self, number):
        attempt = load_state(self.directory)["attempts"][number - 1]
        write_json(self.directory / f"failure-{number:04d}.json", {
            "format_version": 1, "kind": "devex-stage-failure", "attempt": number,
            "stage": "seed-runtime", "mode": "departments-plan", "error_type": attempt["error_type"],
            "frames": [
                {"file": "tools/python/devex_clone_run.py", "function": "execute", "line": 1},
                {"file": "tools/python/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 2},
                {"file": "tools/python/devex_clone_department.py", "function": "execute_departments", "line": 3},
                {"file": "tools/python/devex_clone_department.py", "function": "adopt_failed_plan", "line": 4},
                {"file": "tools/python/devex_clone_department.py", "function": "_adoption_evidence", "line": 5},
                {"file": "tools/python/devex_clone_department.py", "function": "_empty_adoption_output", "line": 6},
            ],
            "controller": binding(self.directory / f"controller-{number:04d}.json")})

    def finish_guard_failure(self, output, number, *, unknown=None):
        write_json(output / ("command-" + "a" * 32 + ".json"), {
            "command": ["cargo", "-V"], "returncode": 0, "error_type": None,
            "stdout": "cargo fixture", "stderr": ""})
        if unknown is not None:
            write_json(output / unknown, {"fixture": True})
        finish(self.directory, number, error=ValueError("guard inventory"))
        self.write_guard_failure(number)
        self.remove_run_lock()

    def start_plan_retry(self, sources, identity):
        number = begin(self.directory, "seed-runtime", "departments-plan", sources)
        self.context.output = self.directory / "seed-runtime" / f"attempt-{number:04d}"
        self.context.output.mkdir(parents=True)
        controller = self.write_controller(number, identity)
        lock = self.directory / "run.lock"
        lock.mkdir()
        write_json(lock / "owner.json", controller["owner"])
        return number

    def remove_run_lock(self):
        (self.directory / "run.lock/owner.json").unlink()
        (self.directory / "run.lock").rmdir()

    def failed_plan_retry(self, *, current_product="1"):
        candidate = begin(self.directory, "seed-runtime", "departments-plan",
                          self.execution_sources())
        self.context.output = self.directory / "seed-runtime" / f"attempt-{candidate:04d}"
        self.context.output.mkdir(parents=True)
        department.execute_body(self.context, self.request, self.bridge, "departments-plan")
        finish(self.directory, candidate, error=TimeoutError("close"))
        controller = self.write_controller(
            candidate, {"pid": 991, "started": "222", "executable": "python"})
        self.write_failure(candidate)
        producer_path = self.context.output / "session-process.json"
        write_json(producer_path, {"fixture": True})
        observed = {"alive": False, "producer_kind": "department-plan", "request": self.request,
                    "controller": controller, "receipt": binding(producer_path)}
        current_identity = {"pid": 992, "started": "333", "executable": "python"}
        self.start_plan_retry(self.execution_sources(product=current_product, tools="6", support="7"),
                              current_identity)
        return candidate, observed, current_identity

    def complete(self):
        self.run_mode("departments-plan")
        self.run_mode("departments-apply")
        return self.run_mode("departments-verify")

    def test_c34_prepared_622_entries_get_ten_departments_without_replaying_identities(self):
        before = self.prepared_identity()
        result = self.complete()
        self.assertEqual(result["status"], "seed_departments_verified")
        self.assertEqual(len(result["confirmed"]), 10)
        self.assertEqual(len([item for item in self.bridge.calls if item[0] == "create"]), 10)
        self.assertEqual(model.identity_projection(self.backend, self.request, allow_absent=False), before)
        actions = model.validate_plan(self.backend, self.directory, self.request)["actions"]
        templates = model.template_identities(self.backend, self.request)
        self.assertEqual([item["template_principal"] for item in actions],
                         [model.principal(item) for item in templates])
        proof = model.authorize_identity(self.backend, self.directory, self.request, "verify")
        self.assertEqual(proof, model.department_evidence(self.backend, self.directory, self.request))

    def test_apply_repeats_only_mutation_guard_between_full_stage_guards(self):
        self.run_mode("departments-plan")
        guard_mock = department.guard
        guard_mock.reset_mock()
        self.run_mode("departments-apply")
        calls = guard_mock.call_args_list
        self.assertEqual([call for call in calls if not call.kwargs.get("quick")],
                         [unittest.mock.call(self.context, self.request,
                                             model.read_prerequisite(self.backend, self.directory))])
        self.assertTrue(any(call.kwargs.get("quick") for call in calls))

    def test_department_plan_requires_confirmed_identity_ledger_before_http(self):
        shutil.rmtree(self.request["identity_state"])
        with self.assertRaises(ValueError):
            self.run_mode("departments-plan")
        self.assertEqual(self.bridge.calls, [])

    def test_unknown_before_reconciles_then_reuses_same_fixed_idempotency_key(self):
        self.run_mode("departments-plan")
        self.bridge.failure = "before"
        with self.assertRaises(TimeoutError):
            self.run_mode("departments-apply")
        first_action = model.validate_plan(self.backend, self.directory, self.request)["actions"][0]
        first_attempt = model.directories(self.directory, first_action)[1][0]
        first_intent = read_json(first_attempt / "intent.json")
        with self.assertRaises(ValueError):
            self.run_mode("departments-apply")
        self.bridge.failure = None
        self.run_mode("departments-reconcile")
        self.run_mode("departments-apply")
        attempts = model.directories(self.directory, first_action)[1]
        self.assertEqual(len(attempts), 2)
        self.assertEqual(read_json(attempts[1] / "intent.json")["idempotency_key"],
                         first_intent["idempotency_key"])
        keys = [values["idempotency_key"] for name, tenant, values in self.bridge.calls
                if name == "create" and tenant == first_action["target"]["tenant_id"]]
        self.assertEqual(keys, [first_intent["idempotency_key"], first_intent["idempotency_key"]])

    def test_subject_change_closes_unknown_result_and_idempotency_reuse(self):
        self.run_mode("departments-plan")
        tenant = self.targets[0]["tenant_id"]
        self.bridge.failure = "before"
        with self.assertRaises(TimeoutError):
            self.run_mode("departments-apply")
        self.bridge.failure = None
        self.bridge.subjects[tenant] = "999999"
        with self.assertRaises(ValueError):
            self.run_mode("departments-reconcile")
        with self.assertRaises(ValueError):
            self.run_mode("departments-apply")
        self.assertEqual(len([item for item in self.bridge.calls
                              if item[0] == "create" and item[1] == tenant]), 1)

    def test_template_subject_change_fails_before_department_write(self):
        self.run_mode("departments-plan")
        tenant = self.targets[0]["tenant_id"]
        self.bridge.template_subjects[tenant] = "999999"
        with self.assertRaises(ValueError):
            self.run_mode("departments-apply")
        self.assertFalse(any(item[0] == "create" for item in self.bridge.calls))

    def test_unknown_after_is_only_confirmed_by_reconcile_and_never_replayed(self):
        self.run_mode("departments-plan")
        self.bridge.failure = "after"
        with self.assertRaises(TimeoutError):
            self.run_mode("departments-apply")
        self.bridge.failure = None
        self.run_mode("departments-reconcile")
        plan = model.validate_plan(self.backend, self.directory, self.request)
        attempt = model.directories(self.directory, plan["actions"][0])[1][0]
        confirmed = model.confirmation(self.backend, self.directory, plan, plan["actions"][0],
                                       self.targets[0], attempt)
        self.assertFalse(confirmed["api_response_proven"])
        self.assertEqual(set(confirmed["evidence"]), {"intent", "before", "observed", "reconcile"})
        count = len([item for item in self.bridge.calls if item[0] == "create"])
        self.run_mode("departments-apply")
        self.assertEqual(len([item for item in self.bridge.calls if item[0] == "create"]), count + 9)
        self.assertEqual(len(model.directories(self.directory, plan["actions"][0])[1]), 1)

    def test_saved_success_response_is_consumed_by_reconcile_and_never_orphaned(self):
        self.run_mode("departments-plan")
        original, calls = department.observe, 0

        def fail_after_response(*args):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise RuntimeError("确认观察中断")
            return original(*args)

        with patch.object(department, "observe", side_effect=fail_after_response):
            with self.assertRaises(RuntimeError):
                self.run_mode("departments-apply")
        plan = model.validate_plan(self.backend, self.directory, self.request)
        attempt = model.directories(self.directory, plan["actions"][0])[1][0]
        self.assertTrue((attempt / "api-response.json").exists())
        with self.assertRaises(ValueError):
            self.run_mode("departments-apply")
        self.run_mode("departments-reconcile")
        confirmed = model.confirmation(self.backend, self.directory, plan, plan["actions"][0],
                                       self.targets[0], attempt)
        self.assertTrue(confirmed["api_response_proven"])
        self.assertFalse(any(attempt.glob("reconcile-*.json")))

    def test_template_path_drift_and_inconsistent_page_metadata_fail_before_write(self):
        self.run_mode("departments-plan")
        tenant = self.targets[0]["tenant_id"]
        self.bridge.template_extra[tenant] = ["意外可用路径"]
        with self.assertRaises(ValueError):
            self.run_mode("departments-apply")
        self.assertFalse(any(item[0] == "create" for item in self.bridge.calls))
        page = self.bridge.page([])
        page["data"]["total_pages"] = 1
        with self.assertRaises(ValueError):
            model.page_data(page)

    def test_department_template_reuses_restricted_xlsx_reader(self):
        sheet = (
            f'<worksheet xmlns="{model.SHEET_NS}"><sheetData><row r="2">'
            '<c r="A2" t="s"><v>1</v></c></row></sheetData></worksheet>'
        )
        strings = f'<sst xmlns="{model.SHEET_NS}"><si><t>唯一字符串</t></si></sst>'
        invalid_index = io.BytesIO()
        with zipfile.ZipFile(invalid_index, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("xl/worksheets/sheet2.xml", sheet)
            archive.writestr("xl/sharedStrings.xml", strings)
        with self.assertRaisesRegex(ValueError, "受限且可解析"):
            model._workbook_paths(invalid_index.getvalue())

        traversal = io.BytesIO()
        with zipfile.ZipFile(traversal, "w", zipfile.ZIP_DEFLATED) as archive:
            archive.writestr("xl/worksheets/sheet2.xml", sheet)
            archive.writestr("../outside", b"escape")
        with self.assertRaisesRegex(ValueError, "受限且可解析"):
            model._workbook_paths(traversal.getvalue())

    def test_existing_unrelated_departments_are_preserved_and_fixed_root_is_added(self):
        tenant = self.targets[0]["tenant_id"]
        self.bridge.rows[tenant] = [
            {"id": "6998", "name": "既有根", "parent_id": None, "ancestors": "", "sort": 0,
             "status": "1", "remark": None, "created_at": "2026-09-05T00:00:00Z"},
            {"id": "6999", "name": "既有子项", "parent_id": "6998", "ancestors": "6998", "sort": 1,
             "status": "1", "remark": None, "created_at": "2026-09-05T00:00:00Z"},
        ]
        self.complete()
        names = {item["name"] for item in self.bridge.rows[tenant]}
        self.assertEqual(names, {"既有根", "既有子项", self.targets[0]["body"]["name"]})
        action = model.validate_plan(self.backend, self.directory, self.request)["actions"][0]
        self.assertEqual(action["before"]["total"], 2)

    def test_mismatched_or_duplicate_fixed_departments_never_get_written(self):
        row = {"id": "7000", "name": self.targets[0]["body"]["name"], "parent_id": None,
               "ancestors": "0", "sort": 0, "status": "0", "remark": None,
               "created_at": "2026-09-05T00:00:00Z"}
        self.bridge.rows[self.targets[0]["tenant_id"]] = [row]
        with self.assertRaises(ValueError):
            self.run_mode("departments-plan")
        self.assertFalse(any(item[0] == "create" for item in self.bridge.calls))

    def test_tampered_confirmation_and_latest_failed_verify_close_identity_gate(self):
        self.prepared_identity()
        self.complete()
        plan = model.validate_plan(self.backend, self.directory, self.request)
        attempt = model.directories(self.directory, plan["actions"][0])[1][0]
        original = read_json(attempt / "confirmed.json")
        (attempt / "confirmed.json").write_text(
            json.dumps({**original, "api_response_proven": False}), encoding="utf-8")
        with self.assertRaises(ValueError):
            model.authorize_identity(self.backend, self.directory, self.request, "verify")
        (attempt / "confirmed.json").write_text(json.dumps(original), encoding="utf-8")
        self.stage("seed-runtime", "departments-verify", error=ValueError("新核验失败"))
        with self.assertRaises(ValueError):
            model.department_evidence(self.backend, self.directory, self.request)

    def test_permissions_intent_and_prepared_entry_tampering_fail_closed(self):
        target = self.targets[0]
        wildcard = {"subject_id": "9", "tenant_id": target["tenant_id"],
                    "username": target["admin"]["username"], "is_super_admin": False,
                    "permissions": ["*"]}
        with self.assertRaises(ValueError):
            model.authorization(target, wildcard, write=True)
        wildcard.update(is_super_admin=True, permissions=["*", "system:dept:list", "system:dept:add"])
        with self.assertRaises(ValueError):
            model.authorization(target, wildcard, write=True)
        template_identity = model.template_identities(self.backend, self.request)[0]
        expected = model.principal(template_identity)
        template_auth = {**expected, "is_super_admin": False,
                         "permissions": [model.TEMPLATE_PERMISSION]}
        self.assertEqual(model.template_authorization(target, template_auth, expected), expected)
        for permissions in ([], ["*", model.TEMPLATE_PERMISSION]):
            with self.assertRaises(ValueError):
                model.template_authorization(target, {**template_auth, "permissions": permissions}, expected)
        self.prepared_identity()
        root = Path(self.request["identity_state"])
        ledger = read_json(root / "ledger.json")
        ledger["entries"][3]["id"] = ledger["entries"][0]["id"]
        (root / "ledger.json").write_text(json.dumps(ledger), encoding="utf-8")
        with self.assertRaises(ValueError):
            model.identity_projection(self.backend, self.request, allow_absent=False)

    def test_final_evidence_validates_every_attempt_and_one_idempotency_key(self):
        self.run_mode("departments-plan")
        self.bridge.failure = "before"
        with self.assertRaises(TimeoutError):
            self.run_mode("departments-apply")
        self.bridge.failure = None
        self.run_mode("departments-reconcile")
        self.run_mode("departments-apply")
        self.run_mode("departments-verify")
        plan = model.validate_plan(self.backend, self.directory, self.request)
        attempts = model.directories(self.directory, plan["actions"][0])[1]
        reconcile = next(attempts[0].glob("reconcile-*.json"))
        original = reconcile.read_text(encoding="utf-8")
        reconcile.unlink()
        with self.assertRaises(ValueError):
            model.department_evidence(self.backend, self.directory, self.request)
        reconcile.write_text(original, encoding="utf-8")
        intent_path = attempts[1] / "intent.json"
        retry = read_json(intent_path)
        intent_path.write_text(json.dumps({**retry,
            "idempotency_key": "87654321-4321-4321-8321-cba987654321"}), encoding="utf-8")
        with self.assertRaises(ValueError):
            model.department_evidence(self.backend, self.directory, self.request)
        intent_path.write_text(json.dumps(retry), encoding="utf-8")
        extra = attempts[1].parent / "0003"
        extra.mkdir()
        with self.assertRaises(ValueError):
            model.department_evidence(self.backend, self.directory, self.request)

    def test_department_producer_commands_bind_all_four_modes_without_secrets(self):
        output = self.directory / "seed-runtime/attempt-0042"
        for mode in ("plan", "apply", "reconcile", "verify"):
            command = process.producer_command(self.backend, self.request, self.directory, 42,
                                               "department-" + mode, output)
            self.assertEqual(command[2:], ["--run-dir", str(self.directory), "--attempt", "42",
                "--department-mode", mode, "--identity-plan", self.request["identity_plan"]["path"],
                "--identity-plan-sha256", self.request["identity_plan"]["sha256"]])
            self.assertNotIn("RYFRAME_ADMIN", command)

    def test_department_producer_uses_only_bound_identity_environment_for_fixed_users(self):
        identity_environment = {"APP_SCOPE_ID": "seed-scope", "RYFRAME_USERS": "private-users"}
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.directory,
                                  request=self.post, private={"target_api": {"APP_SCOPE_ID": "seed-scope"}})
        bridge = Mock()
        bridge.call.return_value = {"closed": True}
        with patch.object(department, "identity_inputs", return_value=(self.plan, identity_environment)), \
                patch.object(department, "Bridge", return_value=bridge) as bridge_type, \
                patch.object(department, "execute_body", return_value={"status": "fixture"}), \
                patch.object(department, "read_prerequisite", return_value={}):
            self.assertEqual(department.execute_departments(context, self.request, "departments-plan"),
                             {"status": "fixture"})
        producer_context = bridge_type.call_args.args[0]
        self.assertEqual(producer_context.private["target_api"], identity_environment)
        self.assertEqual(context.private["target_api"], {"APP_SCOPE_ID": "seed-scope"})
        self.assertEqual(bridge.call.call_args.args, ("close",))
        self.assertGreater(bridge.call.call_args.kwargs["timeout"], 209)
        self.assertLessEqual(bridge.call.call_args.kwargs["timeout"], 210)
        bridge.close.assert_called_once_with()

    def test_failed_complete_plan_is_adopted_without_bridge_or_http_reread(self):
        candidate, observed, current_identity = self.failed_plan_retry()
        calls = copy.deepcopy(self.bridge.calls)
        with patch.object(department, "inspect_producer", return_value=observed), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                patch.object(department, "Bridge") as bridge_type:
            result = department.execute_departments(self.context, self.request, "departments-plan")
        bridge_type.assert_not_called()
        self.assertEqual(self.bridge.calls, calls)
        self.assertEqual(result["status"], "seed_departments_planned")
        self.assertEqual(result["adopted"]["attempt"], candidate)
        self.assertEqual(len(result["adopted"]["before"]), 10)
        self.assertEqual(result["adopted"]["original_fingerprints"]["product"],
                         result["adopted"]["current_fingerprints"]["product"])
        self.assertNotEqual(result["adopted"]["original_fingerprints"]["test_tools"],
                            result["adopted"]["current_fingerprints"]["test_tools"])
        self.assertNotEqual(result["adopted"]["original_fingerprints"]["support"],
                            result["adopted"]["current_fingerprints"]["support"])

    def test_failed_adoption_outer_failure_retries_original_without_bridge_or_http(self):
        candidate, observed, current_identity = self.failed_plan_retry()
        original_calls = copy.deepcopy(self.bridge.calls)
        def producer(_backend, _directory, attempt):
            return observed if attempt["number"] == candidate else None
        with patch.object(department, "inspect_producer", side_effect=producer), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                patch.object(department, "Bridge") as bridge_type:
            adopted = department.execute_departments(self.context, self.request, "departments-plan")
        bridge_type.assert_not_called()
        failed_number = load_state(self.directory)["attempts"][-1]["number"]
        finish(self.directory, failed_number, result=adopted, error=ValueError("外层源码终检失败"))
        self.write_failure(failed_number)
        self.remove_run_lock()
        retry_identity = {"pid": 993, "started": "444", "executable": "python"}
        self.start_plan_retry(self.execution_sources(product="1", tools="8", support="9"), retry_identity)
        with patch.object(department, "inspect_producer", side_effect=producer), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: retry_identity if pid == retry_identity["pid"] else None), \
                patch.object(department, "Bridge") as bridge_type:
            retried = department.execute_departments(self.context, self.request, "departments-plan")
        bridge_type.assert_not_called()
        self.assertEqual(self.bridge.calls, original_calls)
        self.assertEqual(retried["adopted"]["attempt"], candidate)
        self.assertEqual(retried["adopted"]["original_fingerprints"],
                         adopted["adopted"]["original_fingerprints"])
        self.assertNotEqual(retried["adopted"]["current_controller"],
                            adopted["adopted"]["current_controller"])

    def test_quiet_gate_failure_before_dispatch_is_skipped_without_remote_reread(self):
        candidate, observed, _ = self.failed_plan_retry()
        original_calls = copy.deepcopy(self.bridge.calls)
        preflight = load_state(self.directory)["attempts"][-1]
        self.context.output.rmdir()
        finish(self.directory, preflight["number"], error=ValueError("quiet gate"))
        write_json(self.directory / f"failure-{preflight['number']:04d}.json", {
            "format_version": 1, "kind": "devex-stage-failure", "attempt": preflight["number"],
            "stage": "seed-runtime", "mode": "departments-plan", "error_type": "ValueError",
            "frames": [
                {"file": "tools/python/devex_clone_run.py", "function": "execute", "line": 1},
                {"file": "tools/python/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 2},
                {"file": "tools/python/devex_clone_post_process.py", "function": "require_quiet", "line": 3},
                {"file": "tools/python/devex_clone_post_process.py", "function": "inspect_producer", "line": 4},
            ],
            "controller": binding(self.directory / f"controller-{preflight['number']:04d}.json")})
        self.remove_run_lock()
        current_identity = {"pid": 994, "started": "555", "executable": "python"}
        self.start_plan_retry(self.execution_sources(product="1", tools="8", support="9"),
                              current_identity)

        def producer(_backend, _directory, attempt):
            return observed if attempt["number"] == candidate else None

        with patch.object(department, "inspect_producer", side_effect=producer), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                patch.object(department, "Bridge") as bridge_type:
            result = department.execute_departments(self.context, self.request, "departments-plan")
        bridge_type.assert_not_called()
        self.assertEqual(self.bridge.calls, original_calls)
        self.assertEqual(result["adopted"]["attempt"], candidate)

    def test_quiet_then_read_only_guard_failures_adopt_original_without_http(self):
        candidate, observed, _ = self.failed_plan_retry()
        original_calls = copy.deepcopy(self.bridge.calls)
        quiet = load_state(self.directory)["attempts"][-1]
        self.context.output.rmdir()
        finish(self.directory, quiet["number"], error=ValueError("quiet gate"))
        write_json(self.directory / f"failure-{quiet['number']:04d}.json", {
            "format_version": 1, "kind": "devex-stage-failure", "attempt": quiet["number"],
            "stage": "seed-runtime", "mode": "departments-plan", "error_type": "ValueError",
            "frames": [
                {"file": "tools/python/devex_clone_run.py", "function": "execute", "line": 1},
                {"file": "tools/python/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 2},
                {"file": "tools/python/devex_clone_post_process.py", "function": "require_quiet", "line": 3},
            ],
            "controller": binding(self.directory / f"controller-{quiet['number']:04d}.json")})
        self.remove_run_lock()
        guard_identity = {"pid": 994, "started": "555", "executable": "python"}
        guard_number = self.start_plan_retry(self.execution_sources(product="1", tools="8", support="9"),
                                             guard_identity)
        guard_output = self.context.output
        self.finish_guard_failure(guard_output, guard_number)
        current_identity = {"pid": 995, "started": "666", "executable": "python"}
        self.start_plan_retry(self.execution_sources(product="1", tools="a", support="b"),
                              current_identity)

        def producer(_backend, _directory, attempt):
            return observed if attempt["number"] == candidate else None

        with patch.object(department, "inspect_producer", side_effect=producer), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                patch.object(department, "Bridge") as bridge_type:
            result = department.execute_departments(self.context, self.request, "departments-plan")
        bridge_type.assert_not_called()
        self.assertEqual(self.bridge.calls, original_calls)
        self.assertEqual(result["adopted"]["attempt"], candidate)
        self.assertIsNotNone(result["adopted"]["guard"])

    def test_guard_failure_rejects_node_or_unknown_evidence(self):
        candidate, observed, _ = self.failed_plan_retry()
        failed = load_state(self.directory)["attempts"][-1]
        output = self.context.output
        self.finish_guard_failure(output, failed["number"], unknown="session-process.json")
        current_identity = {"pid": 995, "started": "666", "executable": "python"}
        self.start_plan_retry(self.execution_sources(), current_identity)

        def producer(_backend, _directory, attempt):
            return observed if attempt["number"] == candidate else None

        with patch.object(department, "inspect_producer", side_effect=producer), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                self.assertRaises(ValueError):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_failed_adoption_chain_rejects_unknown_intermediate_output(self):
        candidate, observed, current_identity = self.failed_plan_retry()
        def producer(_backend, _directory, attempt):
            return observed if attempt["number"] == candidate else None
        with patch.object(department, "inspect_producer", side_effect=producer), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None):
            adopted = department.execute_departments(self.context, self.request, "departments-plan")
        failed_output = self.context.output
        failed_number = load_state(self.directory)["attempts"][-1]["number"]
        finish(self.directory, failed_number, result=adopted, error=OSError("外层发布失败"))
        self.write_failure(failed_number)
        write_json(failed_output / "unknown.json", {"remote_writes": 0})
        self.remove_run_lock()
        retry_identity = {"pid": 993, "started": "444", "executable": "python"}
        self.start_plan_retry(self.execution_sources(), retry_identity)
        with patch.object(department, "inspect_producer", side_effect=producer), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: retry_identity if pid == retry_identity["pid"] else None), \
                self.assertRaises(ValueError):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_failed_plan_adoption_rejects_missing_or_tampered_before_evidence(self):
        _, observed, current_identity = self.failed_plan_retry()
        plan = read_json(model.plan_path(self.directory))
        evidence = Path(plan["actions"][0]["before_evidence"]["path"])
        evidence.unlink()
        with patch.object(department, "inspect_producer", return_value=observed), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                self.assertRaises((FileNotFoundError, ValueError)):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_failed_plan_adoption_rejects_changed_before_evidence(self):
        _, observed, current_identity = self.failed_plan_retry()
        plan = read_json(model.plan_path(self.directory))
        evidence = Path(plan["actions"][0]["before_evidence"]["path"])
        value = read_json(evidence)
        value["list"]["authorization"]["subject_id"] = "999999"
        evidence.write_text(json.dumps(value), encoding="utf-8")
        with patch.object(department, "inspect_producer", return_value=observed), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                self.assertRaises(ValueError):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_failed_plan_adoption_rejects_cross_attempt_evidence(self):
        _, observed, current_identity = self.failed_plan_retry()
        plan_path = model.plan_path(self.directory)
        plan = read_json(plan_path)
        source = Path(plan["actions"][0]["before_evidence"]["path"])
        crossed = self.context.output / source.name
        shutil.copyfile(source, crossed)
        plan["actions"][0]["before_evidence"] = binding(crossed)
        plan["plan_sha256"] = plan_hash({key: value for key, value in plan.items()
                                         if key != "plan_sha256"})
        plan_path.write_text(json.dumps(plan), encoding="utf-8")
        with patch.object(department, "inspect_producer", return_value=observed), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                self.assertRaises(ValueError):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_failed_plan_adoption_rejects_action_evidence(self):
        _, observed, current_identity = self.failed_plan_retry()
        action_root = model.prerequisite_path(self.directory).parent / "tenant-01"
        action_root.mkdir()
        with patch.object(department, "inspect_producer", return_value=observed), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                self.assertRaises(ValueError):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_failed_plan_adoption_rejects_product_change(self):
        _, observed, current_identity = self.failed_plan_retry(current_product="8")
        with patch.object(department, "inspect_producer", return_value=observed), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                self.assertRaises(ValueError):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_failed_plan_adoption_rejects_live_original_producer(self):
        _, observed, current_identity = self.failed_plan_retry()
        observed = {**observed, "alive": True}
        with patch.object(department, "inspect_producer", return_value=observed), \
                patch.object(department, "process_identity",
                             side_effect=lambda pid: current_identity if pid == current_identity["pid"] else None), \
                self.assertRaises(ValueError):
            department.execute_departments(self.context, self.request, "departments-plan")

    def test_close_timeout_does_not_send_a_second_out_of_order_close(self):
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.directory,
                                  request=self.post, private={"target_api": {}})
        bridge = Mock()
        bridge.call.side_effect = TimeoutError("close")
        with patch.object(department, "identity_inputs", return_value=(self.plan, {})), \
                patch.object(department, "Bridge", return_value=bridge), \
                patch.object(department, "execute_body", return_value={"status": "fixture"}), \
                self.assertRaises(TimeoutError):
            department.execute_departments(context, self.request, "departments-plan")
        self.assertEqual(bridge.call.call_args.args, ("close",))
        self.assertGreater(bridge.call.call_args.kwargs["timeout"], 209)
        self.assertLessEqual(bridge.call.call_args.kwargs["timeout"], 210)
        bridge.close.assert_called_once_with()

    def test_close_deadline_reserves_margin_after_elapsed_cleanup_time(self):
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.directory,
                                  request=self.post, private={"target_api": {}})
        bridge = Mock()
        bridge.call.return_value = {"closed": True}
        with patch.object(department, "identity_inputs", return_value=(self.plan, {})), \
                patch.object(department, "Bridge", return_value=bridge), \
                patch.object(department, "execute_body", return_value={"status": "fixture"}), \
                patch.object(department, "read_prerequisite", return_value={}), \
                patch.object(department.time, "monotonic", side_effect=[100.0, 105.0]):
            department.execute_departments(context, self.request, "departments-apply")
        bridge.call.assert_called_once_with("close", timeout=205.0)
        bridge.close.assert_called_once_with()

    def test_base_exception_before_close_gets_one_bounded_cleanup_and_exact_close(self):
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.directory,
                                  request=self.post, private={"target_api": {}})
        bridge = Mock()
        bridge.call.return_value = {"closed": True}
        with patch.object(department, "identity_inputs", return_value=(self.plan, {})), \
                patch.object(department, "Bridge", return_value=bridge), \
                patch.object(department, "execute_body", side_effect=KeyboardInterrupt()), \
                self.assertRaises(KeyboardInterrupt):
            department.execute_departments(context, self.request, "departments-plan")
        self.assertEqual(bridge.call.call_args.args, ("close",))
        self.assertGreater(bridge.call.call_args.kwargs["timeout"], 209)
        self.assertLessEqual(bridge.call.call_args.kwargs["timeout"], 210)
        bridge.close.assert_called_once_with()

    def test_identity_producer_is_not_started_before_department_gate(self):
        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.directory,
                                  request=self.post)
        with patch.object(runtime, "capacity_evidence", return_value={}), \
                patch.object(runtime, "current_guard"), \
                patch("devex_clone_department_model.authorize_identity", side_effect=ValueError("departments")), \
                patch.object(runtime, "Producer") as producer, self.assertRaises(ValueError):
            runtime.identity_run(context, self.request, "verify")
        producer.assert_not_called()


if __name__ == "__main__":
    unittest.main()
