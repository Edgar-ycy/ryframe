import datetime as dt
from pathlib import Path

from workspace_directory import WorkspaceDirectory

from restore_build import file_digest
from restore_reference import artifact, now, write_json
from restore_reference_io import DATA_HEADER
from restore_reference_plan import BUCKETS


def environment(test):
    local = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"
    local.mkdir(parents=True, exist_ok=True)
    temporary = WorkspaceDirectory(dir=local)
    test.addCleanup(temporary.cleanup)
    backend = Path(temporary.name).resolve()
    config = backend / ".local-tests/client.cnf"
    config.parent.mkdir()
    config.write_text("[client]\nhost=127.0.0.1\nport=3306\nuser=test\npassword=fixture\n")
    executable = backend / "tool.exe"
    executable.write_bytes(b"unit-test-tool")
    plan = {"format_version": 1, "id": "restore-case", "target_side": "base",
            "work_dir": str(config.parent / "reference"),
            "tools": {name: {"path": str(executable), "sha256": file_digest(executable)["sha256"]}
                      for name in ("mysql", "mysqldump", "aws", "node")}}
    for side in ("source", "target"):
        plan[side] = {"scope_id": side, "runtime_dir": str(config.parent / side),
                      "api_url": "http://127.0.0.1:3000", "frontend_url": "http://127.0.0.1:4174",
                      "s3": {"endpoint": "http://127.0.0.1:9000", "region": "us-east-1",
                             "access_key_env": "TEST_ACCESS", "secret_key_env": "TEST_SECRET"},
                      "databases": [{"key": key, "kind": kind, "mode": mode,
                                     "database": f"{side}_{key}", "server_uuid": "uuid-1", "defaults_file": str(config),
                                     "defaults_sha256": file_digest(config)["sha256"]}
                                    for key, kind, mode in (("control", "combined", "shared"), ("dedicated", "tenant", "dedicated"))]}
    return backend, plan


def inventory(plan):
    observed = now()
    return {"id": plan["id"], "scope_id": "source", "source_sha": "a" * 40,
            "captured_at": observed, "quiesced_at": observed,
            "databases": [{**{key: db[key] for key in ("key", "kind", "database", "server_uuid")},
                           "shared": db["mode"] == "shared",
                           "tables": [{"table": "sys_post", "rows": 1, "sha256": "b" * 64}]}
                          for db in plan["source"]["databases"]],
            "objects": [{"bucket": bucket, "prefix": "source/", "entries": []} for bucket in sorted(BUCKETS)]}


def stored_backup(plan, work):
    manifest = inventory(plan)
    root = work / "backup"
    root.mkdir()
    artifacts = []
    for db in manifest["databases"]:
        path = root / "databases" / f"{db['key']}.sql"
        path.parent.mkdir(exist_ok=True)
        path.write_text(DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1,'旧数据');\n", encoding="utf-8")
        artifacts.append(artifact(root, path, f"db:{db['key']}"))
    for objects in manifest["objects"]:
        directory = root / "objects" / objects["bucket"]
        directory.mkdir(parents=True)
        entries = []
        if objects["bucket"] == "uploads":
            path = directory / "one.bin"
            path.write_bytes(b"restored-file")
            entry = {"key": "source/tenant/file.txt", **file_digest(path)}
            objects["entries"].append(entry)
            entries.append({**entry, "file": path.relative_to(root).as_posix(), "content_type": "text/plain"})
            artifacts.append(artifact(root, path, "objects:uploads"))
        index = directory / "index.json"
        write_json(index, {"entries": entries})
        artifacts.append(artifact(root, index, f"objects:{objects['bucket']}"))
    manifest["artifacts"] = artifacts
    write_json(root / "manifest.json", manifest)
    return root, manifest


def restore_record(plan, captured_at=None):
    from restore_reference_execution import product_plan_hash

    started = now()
    product = {"id": "restore-run", "backup_id": plan["id"], "scope_id": "target",
                     "fault_at": started, "frontend_sha": "f" * 40,
                     "api_ready_url": plan["target"]["api_url"] + "/readyz", "worker_ready_url": "http://127.0.0.1:19200/readyz",
                     "object_endpoint": plan["target"]["s3"]["endpoint"], "object_prefix": "target/",
                     "databases": [{"source_key": db["key"], "target_key": db["key"],
                                    "server_uuid": db["server_uuid"], "database": db["database"]}
                                   for db in plan["target"]["databases"]]}
    return {"status": "running", "started_at": started, "plan": product, "plan_hash": product_plan_hash(product),
            "recovered_at": captured_at or started, "completed_at": None, "data_verified_at": None, "failure": None}
