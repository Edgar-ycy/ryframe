"""调度三个动作的部分成功、未知写入、显式核对及故障窗口；无真实服务。"""
import copy
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_post_actions as actions
from devex_clone_capture import read_json
from devex_clone_schedule import schedule_row_sha256
from devex_clone_run_state import binding

COLUMNS = dict(id="BIGINT", tenant_id="VARCHAR", name="VARCHAR", handler_key="VARCHAR", cron_expression="VARCHAR",
               timezone="VARCHAR", enabled="TINYINT", misfire_policy="VARCHAR", concurrency_policy="VARCHAR",
               max_runtime_seconds="INT", next_run_at="DATETIME", last_run_at="DATETIME", version="BIGINT",
               del_flag="CHAR", created_at="DATETIME", updated_at="DATETIME")


class FakeBridge:
    def __init__(self, context):
        self.context, self.writes, self.calls = context, [], []
        self.failure = None

    def login(self):
        return {**self.context.request['source_admin'], 'is_super_admin': True, 'permissions': []}

    def call(self, operation, **arguments):
        self.calls.append(operation)
        if operation == 'close':
            return {'closed': True}
        key = arguments['schedule_id']
        row = self.context.rows[key]
        if operation == 'disable':
            self.writes.append(key)
            if self.failure == ('before', key):
                raise TimeoutError('离线模拟：服务未提交')
            if row['version'] != arguments['version']:
                raise ValueError('离线模拟：乐观锁冲突')
            row.update(enabled=0, version=row['version'] + 1, next_run_at=None, updated_at='2026-09-04 07:00:01')
            if self.failure == ('after', key):
                raise TimeoutError('离线模拟：服务已提交，响应丢失')
        return dict(id=key, handler_key=row['handler_key'], enabled=bool(row['enabled']), version=row['version'])


class ActionsTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        base = self.backend / '.local-tests/run/post-copy'
        base.mkdir(parents=True)
        self.context = SimpleNamespace(backend=self.backend, base=base,
            request={'source_admin': {'subject_id': '9', 'tenant_id': 'system', 'username': 'admin'}},
            request_binding={'path': 'fixture', 'bytes': 1, 'sha256': 'a' * 64}, rows={},
            guard=lambda **_: None, administrator=lambda: None)
        handlers = ('system.export_result_cleanup', 'system.message_retention_cleanup', 'system.data_retention_cleanup')
        pending = []
        for index, handler in enumerate(handlers, 1):
            row = dict(id=index, tenant_id='system', name='清理', handler_key=handler,
                cron_expression='0 30 3 * * * *', timezone='UTC', enabled=1, misfire_policy='fire_once',
                concurrency_policy='forbid', max_runtime_seconds=900, next_run_at='2026-09-05 03:30:00',
                last_run_at='2026-09-04 03:30:00', version=1, del_flag='0',
                created_at='2026-09-01 00:00:00', updated_at='2026-09-04 03:30:00')
            self.context.rows[str(index)] = row
            pending.append(dict(action='disable_schedule_via_api', tenant_id='system', schedule_id=str(index),
                handler_key=handler, expected_version=1, source_row_sha256=schedule_row_sha256(row)))
        self.context.copy = {'plan': {'pending_target_actions': pending}}
        self.context.row = lambda action: {'row': copy.deepcopy(self.context.rows[action['schedule_id']]),
            'database_now': '2026-09-04 07:00:02' if not self.context.rows[action['schedule_id']]['enabled'] else '2026-09-04 07:00:00'}
        self.addCleanup(patch.stopall)
        patch('devex_clone_post_model.schema_catalog', return_value=({'sys_job_schedule': COLUMNS}, {})).start()
        patch('devex_clone_post_model.schema_catalog', return_value=({'sys_job_schedule': COLUMNS}, {})).start()
        self.bridge = FakeBridge(self.context)
        self.number = 0

    def execute(self, mode='schedules'):
        self.number += 1
        self.context.output = self.context.base / f'attempt-{self.number:04d}'
        self.context.output.mkdir()
        return actions.execute_actions(self.context, self.bridge, mode)

    def test_three_actions_confirmed_and_repeat_only_reads(self):
        first = self.execute()
        self.assertEqual(first['status'], 'target_schedule_actions_verified')
        saved = [binding(Path(item['path'])) for item in first['confirmed']]
        self.execute()
        self.assertEqual(self.bridge.writes, ['1', '2', '3'])
        self.assertEqual([binding(Path(item['path'])) for item in saved], saved)

    def test_unknown_write_keeps_original_exception_when_logout_and_guard_fail(self):
        self.bridge.failure = ('after', '2')
        call = self.bridge.call
        def failed_close(operation, **values):
            if operation == 'close':
                raise ValueError('注销失败')
            return call(operation, **values)
        self.bridge.call = failed_close
        with self.assertRaises(TimeoutError): self.execute()
        self.assertTrue((self.context.output / 'session-logout-error.json').exists())
        self.assertTrue((self.context.base / 'schedule-2/0001/intent.json').exists())
        self.assertFalse((self.context.base / 'schedule-2/0001/confirmed.json').exists())

    def test_unknown_committed_second_action_requires_reconcile_then_resumes_third(self):
        self.bridge.failure = ('after', '2')
        with self.assertRaises(TimeoutError): self.execute()
        with self.assertRaises(ValueError): self.execute()
        self.assertEqual(self.bridge.writes, ['1', '2'])
        result = self.execute('reconcile')
        self.assertEqual(result['status'], 'target_schedule_actions_reconciled')
        proof = read_json(self.context.base / 'schedule-2/0001/confirmed.json')
        self.assertFalse(proof['api_execution_proven'])
        self.bridge.failure = None
        self.execute()
        self.assertEqual(self.bridge.writes, ['1', '2', '3'])

    def test_unknown_unchanged_requires_explicit_reconcile_and_new_versioned_attempt(self):
        self.bridge.failure = ('before', '1')
        with self.assertRaises(TimeoutError): self.execute()
        with self.assertRaises(ValueError): self.execute()
        self.execute('reconcile')
        self.assertFalse((self.context.base / 'schedule-1/0001/confirmed.json').exists())
        original = binding(self.context.base / 'schedule-1/0001/intent.json')
        self.bridge.failure = None
        self.execute()
        self.assertEqual(binding(Path(original['path'])), original)
        self.assertTrue((self.context.base / 'schedule-1/0002/confirmed.json').exists())

    def test_unrelated_row_change_stays_unresolved_and_blocks_future_writes(self):
        self.bridge.failure = ('after', '1')
        with self.assertRaises(TimeoutError): self.execute()
        self.context.rows['1']['name'] = '未知修改'
        result = self.execute('reconcile')
        self.assertEqual(result['status'], 'needs_reconciliation')
        with self.assertRaises(ValueError): self.execute()
        self.assertEqual(self.bridge.writes, ['1'])

    def test_confirmed_full_row_drift_blocks_without_rewrite(self):
        self.execute()
        self.context.rows['2']['version'] += 1
        with self.assertRaises(ValueError): self.execute()
        self.assertEqual(self.bridge.writes, ['1', '2', '3'])

    def test_changed_before_evidence_blocks_reconciliation(self):
        self.bridge.failure = ('after', '1')
        with self.assertRaises(TimeoutError): self.execute()
        path = self.context.base / 'schedule-1/0001/before.json'
        value = read_json(path)
        value['row']['name'] = '未知改动'
        path.write_text(json.dumps(value), encoding='utf-8')
        with self.assertRaises(ValueError): self.execute('reconcile')

    def test_failed_confirmation_publication_recovers_committed_row(self):
        original = actions.write_json
        def fail(path, value):
            if path.name == 'confirmed.json': raise OSError('离线磁盘故障')
            return original(path, value)
        with patch.object(actions, 'write_json', side_effect=fail), self.assertRaises(OSError): self.execute()
        with self.assertRaises(ValueError): self.execute()
        self.execute('reconcile')
        self.execute()
        self.assertEqual(self.bridge.writes, ['1', '2', '3'])

    def test_failed_intent_publication_never_calls_put_and_preserves_partial(self):
        original = actions.write_json
        def fail(path, value):
            if path.name == 'intent.json': raise OSError('离线磁盘故障')
            return original(path, value)
        with patch.object(actions, 'write_json', side_effect=fail), self.assertRaises(OSError): self.execute()
        self.assertEqual(self.bridge.writes, [])
        self.execute()
        self.assertTrue((self.context.base / 'schedule-1/0001/before.json').exists())
        self.assertTrue((self.context.base / 'schedule-1/0002/confirmed.json').exists())

    def test_wrong_login_identity_never_calls_mutation(self):
        with patch.object(self.bridge, 'login', return_value={'subject_id': '10'}), self.assertRaises(ValueError):
            self.execute()
        self.assertEqual(self.bridge.writes, [])

    def test_worker_guard_failure_precedes_any_login(self):
        def reject(**_): raise ValueError('离线模拟 Worker 端口仍开放')
        self.context.guard = reject
        with self.assertRaises(ValueError): self.execute()
        self.assertEqual(self.bridge.calls, [])

    def test_confirmation_requires_original_evidence_set_and_exact_embedded_observation(self):
        self.execute()
        path = self.context.base / 'schedule-1/0001/confirmed.json'
        original = read_json(path)
        variants = [{**original, 'evidence': {}}, {**original, 'api_execution_proven': False},
                    {**original, 'observed': {**original['observed'], 'api': {}}}]
        for value in variants:
            path.write_text(json.dumps(value), encoding='utf-8')
            with self.assertRaises(ValueError): actions.verify_confirmations(self.context)
        path.write_text(json.dumps(original), encoding='utf-8')
        self.assertEqual(len(actions.verify_confirmations(self.context)), 3)

    def test_reconciliation_cannot_be_relabelled_as_successful_api_response(self):
        self.bridge.failure = ('after', '1')
        with self.assertRaises(TimeoutError): self.execute()
        self.execute('reconcile')
        path = self.context.base / 'schedule-1/0001/confirmed.json'
        value = read_json(path)
        value['api_execution_proven'] = True
        path.write_text(json.dumps(value), encoding='utf-8')
        with self.assertRaises(ValueError): self.execute()
        self.assertEqual(self.bridge.writes, ['1'])


if __name__ == '__main__':
    unittest.main()
