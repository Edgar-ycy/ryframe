import io
import json
import os
from pathlib import Path
import queue
import sys
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_job_wait as waiter
from test_devex_job_timings import BINDING, JOB, SCOPE, SELECTION, Session


class JobWaitTests(unittest.TestCase):
    def setUp(self):
        environment = mock.patch.dict(os.environ, {"DEVEX_TEST_PASSWORD": "fixture-only"})
        environment.start()
        self.addCleanup(environment.stop)

    def test_completion_waits_for_worker_even_after_business_cancel(self):
        self.assertIsNone(waiter.completion([{**JOB, "status": "running"}], SELECTION))
        self.assertIsNone(waiter.completion([{**JOB, "status": "pending", "business_status": "cancelled"}], SELECTION))
        self.assertEqual(waiter.completion([{**JOB, "business_status": "cancelled"}], SELECTION)["reason"], "job_cancelled")
        self.assertEqual(waiter.completion([{**JOB, "status": "dead"}], SELECTION)["reason"], "job_failed")
        self.assertTrue(waiter.completion([JOB], SELECTION)["valid"])

    def test_each_poll_uses_fresh_read_only_snapshot_and_checks_owner(self):
        session, pending, last, results = Session(job={**JOB, "status": "running"}), {}, [0], []
        waiter.accept({"ticket": 1, "selection": SELECTION, "timeout_ms": 1000}, pending, last, now=lambda: 0)
        waiter.poll(session, BINDING, SCOPE, pending, results.append, now=lambda: .1)
        self.assertEqual(results, [])
        session.job = JOB
        waiter.poll(session, BINDING, SCOPE, pending, results.append, now=lambda: .2)
        self.assertEqual(len(results), 1)
        self.assertTrue(results[0]["valid"])
        self.assertEqual(pending, {})
        self.assertEqual(session.queries.count("START TRANSACTION READ ONLY"), 2)
        self.assertEqual(session.queries.count("ROLLBACK"), 2)
        self.assertEqual(sum("ryframe_resource_ownership" in sql for sql in session.queries), 2)
        self.assertFalse(any("sys_background_job_attempt" in sql for sql in session.queries))

    def test_wrong_scope_is_rejected_before_task_read_and_rollback_always_runs(self):
        session, pending = Session(scope="other-scope"), {}
        waiter.accept({"ticket": 1, "selection": SELECTION, "timeout_ms": 1000}, pending, [0])
        with self.assertRaisesRegex(waiter.EvidenceError, "ownership"):
            waiter.poll(session, BINDING, SCOPE, pending, lambda _value: None)
        self.assertEqual(session.queries[-1], "ROLLBACK")
        self.assertFalse(any("sys_background_job j" in sql for sql in session.queries))

    def test_timeout_retains_exact_identity_as_failure(self):
        session, pending, results = Session(job={**JOB, "status": "running"}), {}, []
        waiter.accept({"ticket": 1, "selection": SELECTION, "timeout_ms": 1000}, pending, [0], now=lambda: 0)
        waiter.poll(session, BINDING, SCOPE, pending, results.append, now=lambda: 1)
        self.assertEqual(results[0]["selection"], SELECTION)
        self.assertEqual(results[0]["reason"], "job_completion_timeout")
        self.assertFalse(results[0]["valid"])

    def test_success_returned_after_deadline_is_still_a_failed_period(self):
        session, pending, results = Session(), {}, []
        waiter.accept({"ticket": 1, "selection": SELECTION, "timeout_ms": 1000}, pending, [0], now=lambda: 0)
        moments = iter([.5, 1.01])
        waiter.poll(session, BINDING, SCOPE, pending, results.append, now=lambda: next(moments))
        self.assertEqual(results[0]["reason"], "job_completion_timeout")
        self.assertFalse(results[0]["valid"])

    def test_requests_are_bounded_monotonic_and_strictly_validated(self):
        pending, last = {}, [0]
        request = {"ticket": 1, "selection": SELECTION, "timeout_ms": 1000}
        waiter.accept(request, pending, last)
        for value in [request, {**request, "ticket": 2, "timeout_ms": 999},
                      {**request, "ticket": 2, "selection": {**SELECTION, "id": "1 OR 1=1"}}]:
            with self.assertRaises(waiter.EvidenceError):
                waiter.accept(value, pending, last)
        for ticket in range(2, 101):
            waiter.accept({**request, "ticket": ticket}, pending, last)
        with self.assertRaises(waiter.EvidenceError):
            waiter.accept({**request, "ticket": 101}, pending, last)

    def test_concurrent_ids_share_one_connection_until_normal_close(self):
        messages, results, session = queue.Queue(), [], Session()

        class Stream:
            def readline(self, _limit):
                return messages.get(timeout=5)

        def emit(value):
            results.append(value)
            if value.get("ready"):
                for ticket in (1, 2):
                    messages.put(json.dumps({"ticket": ticket, "selection": SELECTION, "timeout_ms": 1000}) + "\n")
            elif len([row for row in results if "ticket" in row]) == 2:
                messages.put("")

        with mock.patch.object(waiter, "MysqlSession", return_value=session) as connect:
            waiter.serve(BINDING, SCOPE, Stream(), emit)
        connect.assert_called_once()
        self.assertTrue(session.closed)
        self.assertEqual(len(results), 3)
        self.assertTrue(all(row["valid"] for row in results if "ticket" in row))

    def test_protocol_and_query_failure_are_sanitized(self):
        for source in ["not-json\n", json.dumps({"binding": BINDING, "scope_id": SCOPE}) + "\n"]:
            output = io.StringIO()
            with mock.patch.object(waiter.sys, "stdin", io.StringIO(source)), \
                    mock.patch.object(waiter.sys, "stdout", output), \
                    mock.patch.object(waiter, "MysqlSession", return_value=Session(error=True)):
                self.assertEqual(waiter.main(), 1)
            self.assertFalse(json.loads(output.getvalue())["valid"])
            self.assertNotIn("secret", output.getvalue())


if __name__ == "__main__":
    unittest.main()
