import copy
import io
import json
import os
from pathlib import Path
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_job_model as model
import devex_job_timings as collector


SCOPE = "devex-test-scope"
SERVER = "12345678-1234-1234-1234-123456789abc"
BINDING = {"control": {"host": "127.0.0.1", "port": 3306, "database": "devex_control",
                       "username": "test", "password_env": "DEVEX_TEST_PASSWORD", "tls_mode": "required"},
           "server_uuid": SERVER, "mysql_client": "fixture-client"}
SELECTION = {"kind": "export", "id": "101", "tenant_id": "tenant-a", "job_type": "system.export.execute"}
JOB = {"business_id": "101", "business_tenant": "tenant-a", "business_status": "succeeded",
       "job_id": "201", "job_tenant": "tenant-a", "job_type": "system.export.execute",
       "status": "succeeded", "claim_sequence": 1}
ATTEMPT = {"job_id": "201", "sequence": 1, "outcome": "succeeded",
           "available_at": "2026-09-04T00:00:00.000000Z", "started_at": "2026-09-04T00:00:01.000000Z",
           "finished_at": "2026-09-04T00:00:03.000000Z", "closed_at": "2026-09-04T00:00:03.000000Z"}


class Session:
    def __init__(self, *, scope=SCOPE, server=SERVER, job=None, attempts=None, error=False):
        self.scope, self.server = scope, server
        self.job = JOB.copy() if job is None else job
        self.attempts = [ATTEMPT.copy()] if attempts is None else attempts
        self.error, self.queries, self.closed = error, [], False

    def execute(self, sql):
        self.queries.append(sql)
        if self.error:
            raise RuntimeError("fixture secret and connection must not escape")
        if "ryframe_resource_ownership" in sql:
            return [{"scope": self.scope, "marker": f"ryframe-owner:v1:{self.scope}:control", "server": self.server}]
        if "sys_background_job_attempt" in sql:
            return self.attempts
        if sql.startswith("SELECT JSON_OBJECT"):
            return [self.job]
        return []

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.closed = True


class JobTimingTests(unittest.TestCase):
    def setUp(self):
        self.environment = mock.patch.dict(os.environ, {"DEVEX_TEST_PASSWORD": "fixture-only"})
        self.environment.start()
        self.addCleanup(self.environment.stop)

    def collect(self, session):
        with mock.patch.object(collector, "MysqlSession", return_value=session):
            return collector.collect(BINDING, SCOPE, SELECTION)

    def test_success_uses_exact_owner_job_and_persisted_times(self):
        session = Session()
        result = self.collect(session)
        self.assertTrue(result["valid"])
        self.assertEqual((result["queue_ms"], result["execution_ms"]), (1000, 2000))
        self.assertTrue(session.closed)
        self.assertIn("START TRANSACTION READ ONLY", session.queries)
        self.assertTrue(all("UPDATE" not in sql and "DELETE" not in sql for sql in session.queries))
        self.assertIn("b.id=101 AND b.tenant_id='tenant-a'", session.queries[-2])
        self.assertIn("WHERE job_id=201 ORDER BY `sequence`", session.queries[-1])
        self.assertNotIn("updated_at", " ".join(session.queries))

    def test_wrong_scope_server_tenant_type_and_job_are_rejected(self):
        for session, reason in [(Session(scope="other-scope"), "ownership"),
                                (Session(server="other-server"), "server_identity")]:
            with self.subTest(reason=reason), self.assertRaisesRegex(model.EvidenceError, reason):
                self.collect(session)
            self.assertEqual(len(session.queries), 3)
        for key, value in [("business_id", "102"), ("business_tenant", "tenant-b"),
                           ("job_tenant", "tenant-b"), ("job_type", "system.user.import"), ("job_id", "0")]:
            with self.subTest(key=key), self.assertRaises(model.EvidenceError):
                self.collect(Session(job={**JOB, key: value}))

    def test_failed_cancelled_unfinished_and_missing_start_keep_evidence(self):
        for job, attempts, reason in [({**JOB, "status": "running"}, [ATTEMPT], "job_unfinished"),
                                     ({**JOB, "business_status": "cancelled"}, [ATTEMPT], "job_cancelled"),
                                     ({**JOB, "status": "dead"}, [ATTEMPT], "job_failed"),
                                     (JOB, [{**ATTEMPT, "started_at": None}], "missing_attempt_timestamp"),
                                     (JOB, [{**ATTEMPT, "outcome": "lease_expired", "finished_at": None}], "attempt_result_unknown")]:
            with self.subTest(reason=reason):
                result = self.collect(Session(job=job, attempts=attempts))
                self.assertFalse(result["valid"])
                self.assertEqual(result["reason"], reason)
                self.assertEqual(result["attempts"], attempts)

    def test_every_attempt_is_counted_without_retry_wait_as_execution(self):
        failed = {**ATTEMPT, "outcome": "failed"}
        succeeded = {**ATTEMPT, "sequence": 2, "available_at": "2026-09-04T00:00:10Z",
                     "started_at": "2026-09-04T00:00:12Z", "finished_at": "2026-09-04T00:00:15Z",
                     "closed_at": "2026-09-04T00:00:15Z"}
        result = model.evidence({**JOB, "claim_sequence": 2}, [failed, succeeded])
        self.assertTrue(result["valid"])
        self.assertEqual((result["queue_ms"], result["execution_ms"]), (3000, 5000))
        self.assertEqual(result["unsuccessful_attempts"], 1)
        self.assertFalse(model.evidence({**JOB, "claim_sequence": 2}, [succeeded])["valid"])

    def test_exact_message_and_schedule_links_do_not_guess_latest_job(self):
        message = collector.job_sql("devex_control", {**SELECTION, "kind": "message", "job_type": "system.message.dispatch"})
        self.assertIn("o.aggregate_id=CAST(b.id AS CHAR)", message)
        self.assertIn("j.dedupe_key=o.dedupe_key", message)
        self.assertIn("o.event_type='system.message.published'", message)
        self.assertIn("'$.message_id'", message)
        schedule = {**SELECTION, "kind": "schedule", "tenant_id": "system", "job_type": "system.message.retention"}
        self.assertIn("j.id=b.background_job_id AND j.schedule_id=b.schedule_id", collector.job_sql("devex_control", schedule))
        platform = {**JOB, "business_tenant": "system", "business_status": "enqueued", "job_tenant": None, "job_type": schedule["job_type"]}
        self.assertEqual(model.associated_job([platform], schedule), platform)
        with self.assertRaises(model.EvidenceError):
            model.associated_job([{**platform, "business_status": "skipped_concurrency"}], schedule)

    def test_input_injection_is_rejected_before_database_connection(self):
        for change in [{"id": "1 OR 1=1"}, {"tenant_id": "x';--"}, {"id": str(2**63)}, {"job_type": "other"}]:
            with self.subTest(change=change), mock.patch.object(collector, "MysqlSession") as connect:
                with self.assertRaises(model.EvidenceError):
                    collector.collect(BINDING, SCOPE, {**SELECTION, **change})
                connect.assert_not_called()
        binding = copy.deepcopy(BINDING)
        binding["control"]["host"] = "shared.example.invalid"
        with self.assertRaises(model.EvidenceError):
            collector.validate_binding(binding, SCOPE)

    def test_query_failure_is_preserved_without_sensitive_diagnostics(self):
        request = json.dumps({"binding": BINDING, "scope_id": SCOPE, "selection": SELECTION})
        output = io.StringIO()
        with mock.patch.object(collector.sys, "stdin", io.StringIO(request)), \
                mock.patch.object(collector.sys, "stdout", output), \
                mock.patch.object(collector, "MysqlSession", return_value=Session(error=True)):
            self.assertEqual(collector.main(), 1)
        self.assertEqual(json.loads(output.getvalue())["reason"], "database_collection_failed")
        self.assertNotIn("secret", output.getvalue())


if __name__ == "__main__":
    unittest.main()
