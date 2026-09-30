"""只读采集明确隔离控制库中精确关联任务的全部尝试，不提供任意 SQL 入口。"""
from __future__ import annotations

import json
import os
import re
import sys

from devex_job_model import EvidenceError, associated_job, evidence, selection
from full_stack_migration_mysql import MysqlSession, identifier, ownership


def validate_binding(value: dict, scope: str) -> dict:
    if not isinstance(value, dict) or set(value) != {"control", "server_uuid", "mysql_client"}:
        raise EvidenceError("invalid_database_binding")
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", scope):
        raise EvidenceError("invalid_scope")
    control = value["control"]
    if not isinstance(control, dict) or set(control) != {"host", "port", "database", "username", "password_env", "tls_mode"}:
        raise EvidenceError("invalid_control_binding")
    identifier(control["database"])
    if (control["host"] not in {"127.0.0.1", "localhost"}
            or not isinstance(control["port"], int) or isinstance(control["port"], bool)
            or not 1 <= control["port"] <= 65535
            or control["tls_mode"] not in {"required", "preferred", "disabled"}
            or not isinstance(control["username"], str) or not control["username"]
            or not re.fullmatch(r"[A-Z][A-Z0-9_]*", control["password_env"])):
        raise EvidenceError("invalid_control_binding")
    if not re.fullmatch(r"[a-f0-9-]{36}", value["server_uuid"]):
        raise EvidenceError("invalid_server_identity")
    if control["password_env"] not in os.environ:
        raise EvidenceError("missing_database_password_environment")
    return control


def job_sql(schema: str, expected: dict) -> str:
    identifier(schema)
    selection(expected)
    kind, business_id = expected["kind"], expected["id"]
    if kind in {"export", "import"}:
        table = "sys_export_job" if kind == "export" else "sys_user_import_job"
        source = f"`{schema}`.`{table}` b JOIN `{schema}`.sys_background_job j ON j.id=b.background_job_id"
        state = "b.status" if kind == "export" else "IF(b.cancel_requested,'cancel_requested',b.status)"
    elif kind == "schedule":
        source = (f"`{schema}`.sys_job_schedule_execution b JOIN `{schema}`.sys_background_job j "
                  "ON j.id=b.background_job_id AND j.schedule_id=b.schedule_id "
                  "AND JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.schedule_id'))=CAST(b.schedule_id AS CHAR)")
        state = "b.outcome"
    else:
        source = (f"`{schema}`.sys_message b JOIN `{schema}`.sys_outbox_event o "
                  "ON o.aggregate_type='message' AND o.aggregate_id=CAST(b.id AS CHAR) "
                  "AND o.tenant_id=b.tenant_id AND o.event_type='system.message.published' "
                  f"JOIN `{schema}`.sys_background_job j ON j.dedupe_key=o.dedupe_key "
                  "AND j.dedupe_key=CONCAT('message:',b.id) "
                  "AND JSON_UNQUOTE(JSON_EXTRACT(j.payload,'$.message_id'))=CAST(b.id AS CHAR)")
        state = "o.status"
    return ("SELECT JSON_OBJECT('business_id',CAST(b.id AS CHAR),'business_tenant',b.tenant_id,"
            f"'business_status',{state},'job_id',CAST(j.id AS CHAR),'job_tenant',j.tenant_id,"
            "'job_type',j.job_type,'status',j.status,'claim_sequence',j.claim_sequence) "
            f"FROM {source} WHERE b.id={business_id} AND b.tenant_id='{expected['tenant_id']}' LIMIT 2")


def attempts_sql(schema: str, job_id: str) -> str:
    from devex_job_model import identifier as job_identifier
    identifier(schema)
    job_identifier(job_id)
    times = ",".join(f"'{name}',DATE_FORMAT(`{name}`,'%Y-%m-%dT%H:%i:%s.%fZ')" for name in
                     ("available_at", "started_at", "finished_at", "closed_at"))
    return ("SELECT JSON_OBJECT('job_id',CAST(job_id AS CHAR),'sequence',`sequence`,"
            f"'outcome',outcome,{times}) FROM `{schema}`.sys_background_job_attempt "
            f"WHERE job_id={job_id} ORDER BY `sequence` LIMIT 10001")


def collect(binding: dict, scope: str, expected: dict) -> dict:
    control = validate_binding(binding, scope)
    selection(expected)
    previous = os.environ.get("RYFRAME_E2E_MYSQL_CLIENT")
    os.environ["RYFRAME_E2E_MYSQL_CLIENT"] = binding["mysql_client"]
    try:
        with MysqlSession(control) as session:
            session.execute("SET SESSION time_zone='+00:00'")
            session.execute("START TRANSACTION READ ONLY")
            try:
                server = ownership(session, control["database"], scope, "control")
            except ValueError as error:
                raise EvidenceError("wrong_database_ownership") from error
            if server != binding["server_uuid"]:
                raise EvidenceError("wrong_server_identity")
            row = associated_job(session.execute(job_sql(control["database"], expected)), expected)
            attempts = session.execute(attempts_sql(control["database"], row["job_id"]))
            if len(attempts) > 10000:
                raise EvidenceError("attempt_history_limit")
            result = evidence(row, attempts)
            result.update(scope_id=scope, kind=expected["kind"], business_id=expected["id"], format_version=1)
            return result
    finally:
        if previous is None:
            os.environ.pop("RYFRAME_E2E_MYSQL_CLIENT", None)
        else:
            os.environ["RYFRAME_E2E_MYSQL_CLIENT"] = previous


def main() -> int:
    try:
        source = sys.stdin.read(32769)
        if len(source) > 32768:
            raise EvidenceError("collector_input_limit")
        request = json.loads(source)
        if set(request) != {"binding", "scope_id", "selection"}:
            raise EvidenceError("invalid_collector_request")
        result = collect(request["binding"], request["scope_id"], request["selection"])
    except EvidenceError as error:
        result = {"format_version": 1, "valid": False, "reason": str(error)}
    except Exception:
        # 不转发 MySQL stderr、SQL 或绑定值，避免凭据及业务数据泄漏。
        result = {"format_version": 1, "valid": False, "reason": "database_collection_failed"}
    print(json.dumps(result, ensure_ascii=False))
    return 0 if result["valid"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
