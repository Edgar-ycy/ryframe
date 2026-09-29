"""身份准备的受限只读环境核验和当前 XLSX 模板检查。"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import sys

from user_import_fixture import read_model, template_archive
from full_stack_migration_mysql import MysqlSession, identifier, ownership
from full_stack_process import process_identity, read_process
from full_stack_runtime import verify_runtime
from full_stack_rate_limit_config import load_app_table
from process_sockets import verify_listener


def file_hash(path: Path) -> str:
    if path.is_symlink() or not path.is_file():
        raise ValueError("invalid_file")
    return hashlib.sha256(path.read_bytes()).hexdigest()


def quota_sql(schema: str, tenant: str) -> str:
    identifier(schema)
    if not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", tenant):
        raise ValueError("invalid_tenant")
    return ("SELECT JSON_OBJECT('tenant_id',t.tenant_id,'status',t.status,'max_users',t.max_users,"
            "'max_roles',t.max_roles,'users',(SELECT COUNT(*) FROM "
            f"`{schema}`.sys_user u WHERE u.tenant_id=t.tenant_id AND u.del_flag='0'),"
            f"'roles',(SELECT COUNT(*) FROM `{schema}`.sys_role r WHERE r.tenant_id=t.tenant_id AND r.del_flag='0'),"
            "'target_key',p.current_target_key,'placement_state',p.state) "
            f"FROM `{schema}`.sys_tenant t LEFT JOIN `{schema}`.sys_tenant_data_placement p "
            f"ON p.tenant_id=t.tenant_id WHERE t.tenant_id='{tenant}'")


def validate_quota(row: dict, group: dict, environment: dict, phase: str) -> None:
    quota = environment["quota"]
    expected = quota[f"{group['kind']}_max_users"]
    required = group["count"] if phase == "apply" else 0
    if group["kind"] == "tenant":
        required += quota["import_headroom_per_tenant"]
    if (row.get("tenant_id") != group["tenant_id"] or row.get("status") != "enabled"
            or row.get("max_users") != expected or type(row.get("users")) is not int
            or expected - row["users"] < required or type(row.get("roles")) is not int
            or type(row.get("max_roles")) is not int
            or (phase == "apply" and row["max_roles"] > 0 and row["roles"] >= row["max_roles"])):
        raise ValueError("quota_or_tenant_mismatch")
    if group["kind"] == "tenant":
        tenant = next(item for item in environment["tenants"] if item["slot"] == group["slot"])
        if row.get("target_key") != tenant["target_key"] or row.get("placement_state") != "active":
            raise ValueError("tenant_placement_mismatch")


def validate_target_bindings(backend: Path, environment: dict, variables: dict) -> None:
    # 运行收据绑定 APP 值与 TOML 字节，不绑定外部 JSON 文件内容；不能据其证明启动时目标。
    if "APP_TENANT_DATA_TARGETS_FILE" in variables:
        raise ValueError("target_file_not_bound_by_runtime_receipt")
    table = load_app_table(backend, variables)
    targets = table.get("tenant_data", {}).get("targets", [])
    if "APP_TENANT_DATA_TARGETS" in variables:
        targets = json.loads(variables["APP_TENANT_DATA_TARGETS"])
    if (not isinstance(targets, list) or any(not isinstance(item, dict) or not isinstance(item.get("key"), str)
            for item in targets) or len({item["key"] for item in targets}) != len(targets)):
        raise ValueError("invalid_runtime_targets")
    configured = {item["key"]: item for item in targets}
    for declared in environment["database"]["targets"]:
        if declared["key"] == "shared-control":
            validate_control_target(declared, configured.get("shared-control"), environment, variables)
            continue
        actual = configured.get(declared["key"], {})
        if actual.get("kind") != "mysql" or actual.get("mode") != declared["mode"]:
            raise ValueError("api_target_kind_or_mode_mismatch")
        connection = declared["connection"]
        for field in ("host", "database", "username", "password_env"):
            if actual.get(field) != connection[field]:
                raise ValueError("api_target_connection_mismatch")
        if actual.get("port", 3306) != connection["port"] or actual.get("tls_mode", "required") != connection["tls_mode"]:
            raise ValueError("api_target_connection_mismatch")
        if (not connection["password_env"].startswith("APP_") or not variables.get(connection["password_env"])
                or any(actual.get(field) for field in ("tls_ca", "tls_client_cert", "tls_client_key"))):
            raise ValueError("api_target_secret_or_tls_binding_mismatch")


def validate_control_target(declared: dict, configured: dict | None, environment: dict, variables: dict) -> None:
    # 产品规范化会补充内置控制库目标；物理连接仍须精确等于本次 API 的主控制库。
    control = environment["database"]["control"]
    if (declared["mode"] != "shared" or declared["connection"] != control
            or not control["password_env"].startswith("APP_") or not variables.get(control["password_env"])):
        raise ValueError("api_control_target_connection_mismatch")
    if configured is None:
        return
    forbidden = ("host", "port", "database", "username", "password_env", "max_connections",
                 "tls_mode", "tls_ca", "tls_client_cert", "tls_client_key")
    if (configured.get("kind") != "control" or configured.get("mode") != "shared"
            or any(configured.get(field) is not None for field in forbidden)):
        raise ValueError("api_control_target_configuration_mismatch")


def inspect(environment: dict, groups: list[dict], phase: str) -> dict:
    if phase not in ("apply", "verify") or os.environ.get("APP_SCOPE_ID") != environment["scope_id"]:
        raise ValueError("invalid_scope_or_phase")
    backend = Path(environment["backend_dir"]).resolve()
    directory = Path(environment["runtime"]["directory"]).resolve()
    if not directory.is_relative_to(backend / ".local-tests"):
        raise ValueError("runtime_outside_workspace")
    runtime = environment["runtime"]
    if (file_hash(directory / "runtime.json") != runtime["receipt_sha256"]
            or file_hash(directory / "api.json") != runtime["api_process_sha256"]):
        raise ValueError("runtime_receipt_changed")
    verified = verify_runtime(backend, directory)
    process = read_process(directory, "api", environment["scope_id"])
    if (process_identity(process["pid"]) != process
            or Path(process["executable"]).resolve() != Path(verified["artifacts"]["api"]["path"]).resolve()):
        raise ValueError("api_process_changed")
    verify_listener(process["pid"], environment["api_url"])
    database = environment["database"]
    control = database["control"]
    for field, suffix in (("host", "HOST"), ("port", "PORT"), ("database", "NAME"), ("username", "USERNAME"), ("tls_mode", "TLS_MODE")):
        if str(control[field]) != os.environ.get(f"APP_DATABASE_{suffix}"):
            raise ValueError("api_control_binding_mismatch")
    if os.environ.get(control["password_env"]) != os.environ.get("APP_DATABASE_PASSWORD"):
        raise ValueError("api_control_secret_binding_mismatch")
    validate_target_bindings(backend, environment, os.environ)
    os.environ["RYFRAME_E2E_MYSQL_CLIENT"] = database["mysql_client"]
    result = []
    with MysqlSession(control) as session:
        session.execute("START TRANSACTION READ ONLY")
        if ownership(session, control["database"], environment["scope_id"], "control") != database["server_uuid"]:
            raise ValueError("wrong_control_server")
        for group in groups:
            rows = session.execute(quota_sql(control["database"], group["tenant_id"]))
            if len(rows) != 1:
                raise ValueError("missing_or_duplicate_tenant")
            validate_quota(rows[0], group, environment, phase)
            result.append({"slot": group["slot"], **rows[0]})
    for target in database["targets"]:
        with MysqlSession(target["connection"]) as session:
            if ownership(session, target["connection"]["database"], environment["scope_id"], "tenant-data") != database["server_uuid"]:
                raise ValueError("wrong_target_server")
    return {"scope_id": environment["scope_id"], "server_uuid": database["server_uuid"],
            "api_process": process, "tenants": result}


def template(path: str, sha256: str) -> dict:
    with template_archive(Path(path), sha256) as archive:
        _, department = read_model(archive)
    return {"template_sha256": sha256, "department_path": department,
            "department_sha256": hashlib.sha256(department.encode()).hexdigest()}


def main() -> int:
    try:
        data = sys.stdin.buffer.read(1024 * 1024 + 1)
        if len(data) > 1024 * 1024:
            raise ValueError("input_limit")
        request = json.loads(data)
        if set(request) == {"operation", "environment", "groups", "phase"} and request["operation"] == "inspect":
            result = inspect(request["environment"], request["groups"], request["phase"])
        elif set(request) == {"operation", "path", "sha256"} and request["operation"] == "template":
            result = template(request["path"], request["sha256"])
        else:
            raise ValueError("invalid_operation")
        print(json.dumps({"ok": True, "receipt": result}, ensure_ascii=False))
        return 0
    except Exception:
        # 不输出连接串、SQL、进程环境、响应正文或原始异常。
        print(json.dumps({"ok": False, "stage": "identity_environment_failed"}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
