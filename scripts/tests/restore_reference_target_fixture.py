"""严格目标计划的离线文件图；真实来源核验由已有模块单独覆盖。"""

import copy
import hashlib
from pathlib import Path
from unittest.mock import patch

import restore_reference as reference
import restore_reference_target as target
from devex_clone_inventory import KEYS, expected_owners
from restore_reference_fixture import environment, stored_backup


def setup(test):
    test.backend, test.plan = environment(test)
    for side in ("source", "target"):
        original = test.plan[side]["databases"]
        test.plan[side]["databases"] = [
            {**original[0 if index == 0 else 1], "key": key, "database": f"{side}_{index}",
             "mode": "shared" if index < 2 else "dedicated"}
            for index, key in enumerate(KEYS)]
    test.work = reference.work_directory(test.plan)
    root, manifest = stored_backup(test.plan, test.work)
    manifest.update(completed_at=reference.now(), retention_until=reference.now())
    test.write("backup/manifest.json", manifest)
    test.source_inputs = {
        "source_export": {"result": test.write_binding("export-result.json", {"published": True}),
                          "export": test.write_binding("export.json", {"exported": True})},
        "source_runtime": test.write_binding("source-runtime.json", {"runtime": True}),
        "source_quiescence": test.write_binding("source-quiescence.json", {"quiesced": True})}
    test.backup_value = {"format_version": 1, "kind": "restore-reference-backup", "reference_plan": copy.deepcopy(test.plan),
                         "manifest": test.descriptor(root / "manifest.json"), "backup_root": str(root),
                         "artifacts": len(manifest["artifacts"]), **test.source_inputs}
    test.write("backup.json", {"command": "backup", "status": "completed", "plan_sha256": reference.plan_hash(test.plan),
                               "started_at": reference.now(), "completed_at": reference.now(), "result": test.backup_value})
    test.comparison = {"source_export": copy.deepcopy(test.source_inputs["source_export"]), "arms": {}}
    for name, head in (("b0", "b"), ("b1", "c")):
        inventory = {"source": {"snapshot": {"head": head * 40}}}
        test.comparison["arms"][name] = {
            "roots": {key: str(test.backend / name / key) for key in ("source_backend", "execution_backend", "frontend")},
            "sources": {"backend": inventory, "frontend": inventory},
            "execution_sources": {"backend": {"source": {"snapshot": {"head": ("a" if name == "b0" else head) * 40}}}},
            "builds": {role: test.write_binding(f"{name}/{role}-build.json", {"role": role, "arm": name})
                       for role in ("backend", "frontend")},
            "adapter": {"contract": "legacy-stable-readiness-b0-v1"} if name == "b0" else None}
    test.write("comparison.json", test.comparison)
    test.selected = {**{key: test.plan["target"][key] for key in ("runtime_dir", "api_url", "frontend_url")},
                     "worker_ready_url": "http://127.0.0.1:19200/readyz", "backend_dir": str(test.backend)}
    test.maintenance_binding = {"kind": "current-backend", "path": str(test.backend)}
    test.request = {"side": "base", "target": {key: test.plan["target"][key] for key in ("scope_id", "s3", "databases")},
                    "maintenance_build": test.write_binding("maintenance-build.json", {"kind": "devex-clone-tool-build"})}
    _fresh(test)
    test.product = {"id": "restore-run-base", "backup_id": test.plan["id"], "scope_id": test.plan["target"]["scope_id"],
                    "fault_at": "2026-09-05T00:00:00Z", "databases": [{"source_key": db["key"], "target_key": db["key"],
                    "server_uuid": db["server_uuid"], "database": db["database"]} for db in test.plan["target"]["databases"]],
                    "object_endpoint": test.plan["target"]["s3"]["endpoint"], "object_prefix": "target/",
                    "api_ready_url": test.selected["api_url"] + "/readyz", "worker_ready_url": test.selected["worker_ready_url"],
                    "frontend_sha": "b" * 40}
    test.write("product.json", test.product)
    test.paths = {"backup_receipt": test.work / "backup.json", "comparison_sources": test.work / "comparison.json",
                  "arm_input": test.work / "arm-input.json", "fresh_target_verify": test.work / "observe/verify.json",
                  "product_plan": test.work / "product.json"}
    patches = [patch.object(target, "backup_source", side_effect=lambda *_: copy.deepcopy(test.source_inputs)),
               patch.object(target, "verify_comparison_sources", side_effect=lambda _root, value, **_kwargs: value),
               patch.object(target, "verify_published_arm_input", side_effect=lambda *_: copy.deepcopy(test.arm)),
               patch.object(target, "request_binding", side_effect=lambda *_: ({}, copy.deepcopy(test.selected))),
               patch.object(target, "execution_backend", side_effect=lambda *_: (test.backend, copy.deepcopy(test.maintenance_binding)))]
    for item in patches:
        item.start()
        test.addCleanup(item.stop)


def _fresh(test):
    observations = {db["database"]: {"resource": {"kind": "database", "scope_id": "target",
                     "server_uuid": db["server_uuid"], "database": db["database"]},
                     "ownership": list(expected_owners("target", db["kind"] == "combined"))}
                    for db in test.plan["target"]["databases"]}
    objects = {}
    for bucket in target.BUCKETS:
        owner = f"ryframe-owner:v1:target:object-storage:{bucket}".encode()
        objects[bucket] = {"keys": ["target/.ryframe-owner"], "owner": {"bytes": len(owner), "sha256": hashlib.sha256(owner).hexdigest()}}
    initialized = {"inventory": {"observations": observations}, "objects": objects, "redis": {"owner": "fixture"}}
    inventories = {}
    for phase in ("before", "after"):
        for key in KEYS:
            path = test.write(f"observe/inventory/{phase}-target-{key}.json", {"phase": phase})
            inventories[path.name] = reference.file_digest(path)
        test.write(f"observe/inventory/binding-{phase}.json", {"scope_id": "target"})
    inventory = test.write_binding("observe/inventory/inventory.json", {
        "status": "side_inventory_captured", "format_version": 1, "side": "target", "scope_id": "target",
        "keys": list(KEYS), "inventories": inventories, "binding_sha256": reference.plan_hash({"scope_id": "target"}),
        "remote_writes": 0, "producer_stopped_proven": False, "fresh_target_proven": False,
        "target_ready": False, "clone_verified": False, "restore_qualified": False,
        "observations": {db["key"]: observations[db["database"]] for db in test.plan["target"]["databases"]}})
    result = {"initialized": test.write_binding("fresh/target/initialized.json", initialized),
              "target_registration": test.write_binding("fresh/registration.json", {"registered": True}),
              "target_initialized_files": test.write_binding("fresh/initialized-files.json", {"files": True}),
              "target_environment": test.write_binding("fresh/environment.json", {"environment": {}}),
              "source_export_result": test.source_inputs["source_export"]["result"],
              "source_export": test.source_inputs["source_export"]["export"]}
    arm_binding = test.write_binding("arm-input.json", result)
    test.arm = {"binding": arm_binding, "result": result, "target": test.request,
                "initialized": initialized, "target_side": "base"}
    test.fresh = {"status": "fresh_target_reverified", "initialized": result["initialized"],
                  "inventory": {"receipt": inventory, "observations": observations}, "remote_writes": 0,
                  "target_ready": False, "clone_verified": False, "restore_qualified": False}
    test.write("observe/verify.json", test.fresh)
