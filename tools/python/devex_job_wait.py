"""复用一条只读连接观察精确任务终态；不把已入队当作已完成。"""
from __future__ import annotations

import json
import os
import queue
import sys
import threading
import time

from devex_job_model import EvidenceError, associated_job, selection
from devex_job_timings import job_sql, validate_binding
from full_stack_migration_mysql import MysqlSession, ownership


PENDING = {"missing_job", "message_not_dispatched", "job_unfinished"}


def completion(rows: list[dict], expected: dict) -> dict | None:
    try:
        row = associated_job(rows, expected)
        if row.get("status") not in {"succeeded", "dead"}:
            return None
        if row.get("business_status") in {"cancelled", "cancel_requested"}:
            raise EvidenceError("job_cancelled")
        if row.get("status") == "dead" or row.get("business_status") in {"failed", "partial", "dead", "expired"}:
            raise EvidenceError("job_failed")
        if row.get("business_status") not in {"succeeded", "published", "enqueued"}:
            return None
        return {"valid": True, "job_id": row["job_id"], "status": row["status"]}
    except EvidenceError as error:
        if str(error) in PENDING:
            return None
        return {"valid": False, "reason": str(error)}


def check_owner(session, binding, scope):
    try:
        server = ownership(session, binding["control"]["database"], scope, "control")
    except ValueError as error:
        raise EvidenceError("wrong_database_ownership") from error
    if server != binding["server_uuid"]:
        raise EvidenceError("wrong_server_identity")


def poll(session, binding: dict, scope: str, pending: dict, emit, now=time.monotonic):
    """每轮重新建立只读快照，不能在同一 REPEATABLE READ 快照等待状态变化。"""
    session.execute("START TRANSACTION READ ONLY")
    try:
        check_owner(session, binding, scope)
        for ticket, item in list(pending.items()):
            expired = now() >= item["deadline"]
            result = None if expired else completion(
                session.execute(job_sql(binding["control"]["database"], item["selection"])), item["selection"])
            if now() >= item["deadline"]:
                result = {"valid": False, "reason": "job_completion_timeout"}
            if result is not None:
                emit({"ticket": ticket, "scope_id": scope, "selection": item["selection"], **result})
                del pending[ticket]
    finally:
        session.execute("ROLLBACK")


def read_requests(stream, messages):
    try:
        while True:
            line = stream.readline(32769)
            if not line:
                break
            if len(line) > 32768 or not line.endswith("\n"):
                raise EvidenceError("wait_input_limit")
            messages.put(json.loads(line))
    except Exception:
        messages.put({"invalid": True})
    finally:
        messages.put(None)


def accept(value, pending, last_ticket, now=time.monotonic):
    if (not isinstance(value, dict) or set(value) != {"ticket", "selection", "timeout_ms"}
            or not isinstance(value["ticket"], int) or isinstance(value["ticket"], bool)
            or not last_ticket[0] < value["ticket"] <= 4_000_000
            or not isinstance(value["timeout_ms"], int) or isinstance(value["timeout_ms"], bool)
            or not 1000 <= value["timeout_ms"] <= 300000 or len(pending) >= 100):
        raise EvidenceError("invalid_wait_request")
    selection(value["selection"])
    last_ticket[0] = value["ticket"]
    pending[value["ticket"]] = {"selection": value["selection"], "deadline": now() + value["timeout_ms"] / 1000}


def serve(binding, scope, stream, emit):
    control = validate_binding(binding, scope)
    os.environ["RYFRAME_E2E_MYSQL_CLIENT"] = binding["mysql_client"]
    messages, pending, last_ticket = queue.Queue(maxsize=101), {}, [0]
    threading.Thread(target=read_requests, args=(stream, messages), daemon=True).start()
    with MysqlSession(control) as session:
        session.execute("SET SESSION time_zone='+00:00'")
        check_owner(session, binding, scope)
        emit({"ready": True, "scope_id": scope})
        closed, next_poll = False, 0
        while not closed or pending:
            try:
                value = messages.get(timeout=max(0.001, next_poll - time.monotonic()) if pending else .25)
                if value is None:
                    closed = True
                else:
                    accept(value, pending, last_ticket)
                while not messages.empty():
                    value = messages.get_nowait()
                    if value is None:
                        closed = True
                    else:
                        accept(value, pending, last_ticket)
            except queue.Empty:
                pass
            if closed:
                for ticket, item in pending.items():
                    emit({"ticket": ticket, "scope_id": scope, "selection": item["selection"],
                          "valid": False, "reason": "job_observer_input_closed"})
                pending.clear()
            if pending and time.monotonic() >= next_poll:
                poll(session, binding, scope, pending, emit)
                next_poll = time.monotonic() + .25
        check_owner(session, binding, scope)


def main():
    def emit(value):
        print(json.dumps({"format_version": 1, **value}, ensure_ascii=False), flush=True)
    try:
        header = sys.stdin.readline(32769)
        if len(header) > 32768 or not header.endswith("\n"):
            raise EvidenceError("wait_input_limit")
        request = json.loads(header)
        if set(request) != {"binding", "scope_id"}:
            raise EvidenceError("invalid_wait_binding")
        serve(request["binding"], request["scope_id"], sys.stdin, emit)
        return 0
    except EvidenceError as error:
        emit({"valid": False, "reason": str(error)})
    except Exception:
        emit({"valid": False, "reason": "database_completion_failed"})
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
