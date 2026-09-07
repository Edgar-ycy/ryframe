"""固定 MySQL 查询使用真实子进程输出句柄，失败保留原始字节与各层一致证据。"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

from artifact_digests import filesystem_path
from devex_clone_factory import CloneSession
from devex_clone_inventory import _Capture
from devex_clone_post_context import Context
from restore_reference import work_directory
from restore_reference_fixture import environment
from restore_reference_io import DatabaseVerificationError, ExternalTools, VerificationStdout, verification_stdout


class VerificationStdoutTests(unittest.TestCase):
    def setUp(self):
        self.backend, self.plan = environment(self)
        self.work = work_directory(self.plan)
        self.database = self.plan['source']['databases'][0]
        self.stub = self.work / 'query-stub.py'
        self.stub.write_text("import os,sys,time\nos.write(1,bytes.fromhex(os.environ['QUERY_OUTPUT']))\n"
                             "if os.environ.get('QUERY_SLEEP'): time.sleep(30)\n"
                             "sys.exit(int(os.environ.get('QUERY_EXIT','0')))\n", encoding='utf-8')
        self.environment = dict(os.environ)
        self.environment['QUERY_OUTPUT'] = b'uuid-1\tsource_control\n'.hex()
        self.calls = []
        self.timeout = None
        self.tools = ExternalTools(self.plan, self.work, self.native)
        self.tools._mysql_command = Mock(side_effect=lambda _db: ([sys.executable, str(self.stub)], self.environment))

    def native(self, command, **options):
        self.calls.append(command)
        self.assertIsInstance(options['stdout'], VerificationStdout)
        self.assertEqual(options['stdin'], subprocess.DEVNULL)
        self.assertIsNone(options['input'])
        if self.timeout is not None:
            options['timeout'] = self.timeout
        return subprocess.run(command, **options)

    def output(self, raw):
        self.environment['QUERY_OUTPUT'] = raw.hex()

    def receipt(self):
        records = list(self.work.glob('mysql-*.json'))
        self.assertEqual(len(records), 1)
        receipt = json.loads(records[0].read_text(encoding='utf-8'))
        artifact = receipt['stdout_file']
        raw = Path(artifact['path']).read_bytes()
        self.assertEqual(artifact['bytes'], len(raw))
        self.assertEqual(artifact['sha256'], hashlib.sha256(raw).hexdigest())
        return receipt, raw

    def test_native_stdout_file_success_has_exact_raw_bytes_and_one_call(self):
        self.assertEqual(self.tools.database_verification_response(self.database, 'identity'), 'uuid-1\tsource_control')
        receipt, raw = self.receipt()
        self.assertEqual(raw, b'uuid-1\tsource_control\n')
        self.assertEqual(receipt['returncode'], 0)
        self.assertIsNone(receipt['error_type'])
        self.assertEqual(len(self.calls), 1)

    def test_empty_identity_retry_preserves_both_native_outputs_and_relation(self):
        answers = iter((b'', b'uuid-1\tsource_control\n'))
        def native(command, **options):
            self.calls.append(command)
            environment = dict(options['env'])
            environment['QUERY_OUTPUT'] = next(answers).hex()
            options['env'] = environment
            return subprocess.run(command, **options)
        self.tools.run = native
        self.assertEqual(self.tools.database_verification_response(self.database, 'identity'),
                         'uuid-1\tsource_control')
        records = [json.loads(path.read_text(encoding='utf-8')) for path in self.work.glob('mysql-*.json')]
        outputs = [value for value in records if value['kind'] == 'mysql-verification-output']
        relation = [value for value in records if value['kind'] == 'mysql-verification-retry']
        self.assertEqual((len(self.calls), len(outputs), len(relation)), (2, 2, 1))
        self.assertEqual(set(relation[0]), {'format_version', 'kind', 'check', 'target_key',
                                           'reason', 'first', 'second'})
        self.assertEqual(relation[0]['reason'], 'first_returncode_zero_stdout_stderr_exactly_empty')
        expected_outputs = (b'', b'uuid-1\tsource_control\n')
        for index, name in enumerate(('first', 'second')):
            descriptor = relation[0][name]
            raw = Path(descriptor['path']).read_bytes()
            self.assertEqual((descriptor['bytes'], descriptor['sha256']),
                             (len(raw), hashlib.sha256(raw).hexdigest()))
            receipt = json.loads(raw)
            self.assertIn(receipt, outputs)
            self.assertEqual(Path(receipt['stdout_file']['path']).read_bytes(), expected_outputs[index])
            self.assertEqual((receipt['returncode'], receipt['stderr_bytes'], receipt['stderr_sha256']),
                             (0, 0, hashlib.sha256(b'').hexdigest()))
        self.assertNotIn('SELECT', json.dumps(relation[0]))

    def test_native_nonzero_after_write_records_partial_output_without_retry(self):
        self.output(b'partial-secret\n')
        self.environment['QUERY_EXIT'] = '7'
        with self.assertRaises(DatabaseVerificationError) as caught:
            self.tools.database_verification_response(self.database, 'identity')
        self.assertEqual(caught.exception.returncode, 7)
        self.assertEqual(caught.exception.response_bytes, 15)
        self.assertNotIn('partial-secret', str(caught.exception))
        receipt, raw = self.receipt()
        self.assertEqual((receipt['returncode'], receipt['error_type'], raw), (7, 'CalledProcessError', b'partial-secret\n'))
        self.assertEqual(len(self.calls), 1)

    def test_native_timeout_preserves_original_type_and_written_file(self):
        self.timeout = 0.4
        self.output(b'timeout-partial\n')
        self.environment['QUERY_SLEEP'] = '1'
        with self.assertRaises(subprocess.TimeoutExpired):
            self.tools.database_verification_response(self.database, 'identity')
        receipt, raw = self.receipt()
        self.assertEqual(receipt['error_type'], 'TimeoutExpired')
        self.assertEqual(raw, b'timeout-partial\n')
        self.assertEqual(len(self.calls), 1)

    def test_native_invalid_utf8_preserves_decoder_failure_and_raw_file(self):
        self.output(b'\xff\n')
        with self.assertRaises(UnicodeDecodeError):
            self.tools.database_verification_response(self.database, 'identity')
        receipt, raw = self.receipt()
        self.assertEqual((receipt['error_type'], raw), ('UnicodeDecodeError', b'\xff\n'))
        self.assertEqual(len(self.calls), 1)

    def test_empty_truncated_extra_identity_and_owner_rows_fail_once(self):
        for label, raw, check in [('empty', b'', 'identity'), ('short', b'uuid-1\tsource_contr', 'identity'),
                                  ('extra', b'uuid-1\tsource_control\nextra\trow\n', 'identity'),
                                  ('owner-short', b'control\tsource\twrong\n', 'ownership'),
                                  ('owner-extra', b'control\tsource\twrong\nextra\trow\tmarker\n', 'ownership')]:
            with self.subTest(label=label):
                local = self.work / label
                local.mkdir()
                def run(command, **options):
                    value = (self.database['server_uuid'] + '\t' + self.database['database']).encode() if check == 'ownership' and command[-1].startswith('SELECT @@') else raw
                    options['stdout'].write(value)
                    return subprocess.CompletedProcess(command, 0, None, b'')
                runner = Mock(side_effect=run)
                tools = ExternalTools(self.plan, local, runner)
                with self.assertRaises(DatabaseVerificationError):
                    tools.verify_databases('source')
                expected_calls = 2 if check == 'ownership' or label == 'empty' else 1
                self.assertEqual(runner.call_count, expected_calls)

    def nested(self):
        session = object.__new__(CloneSession)
        session.output = self.work / 'session'
        session.output.mkdir()
        session.copy_owner, session.secret_redaction, session.runner = None, {}, self.native
        capture = object.__new__(_Capture)
        capture.output, capture.environment = self.work, self.environment
        capture.redaction, capture.original = {}, SimpleNamespace(run=session.command)
        self.tools.run = capture.run
        return session

    def test_nested_inventory_and_session_receipts_bind_same_actual_output(self):
        session = self.nested()
        self.tools.database_verification_response(self.database, 'identity')
        actual, _ = self.receipt()
        for root in (self.work, session.output):
            receipt = json.loads(next(root.glob('command-*.json')).read_text(encoding='utf-8'))
            self.assertEqual(receipt['stdout_file'], actual['stdout_file'])
            self.assertEqual(receipt['stdout'], 'uuid-1\tsource_control\n')

    def test_nested_nonzero_diagnostics_do_not_use_none_as_empty_stdout(self):
        session = self.nested()
        self.environment['QUERY_EXIT'] = '9'
        self.output(b'partial\n')
        with self.assertRaises(DatabaseVerificationError):
            self.tools.database_verification_response(self.database, 'identity')
        actual, _ = self.receipt()
        for root in (self.work, session.output):
            receipt = json.loads(next(root.glob('command-*.json')).read_text(encoding='utf-8'))
            self.assertEqual(receipt['stdout_file'], actual['stdout_file'])
            self.assertEqual((receipt['stdout'], receipt['returncode'], receipt['error_type']), ('partial\n', 9, 'CalledProcessError'))

    def test_post_context_records_same_file_without_changing_other_output(self):
        context = object.__new__(Context)
        context.output, context.redaction, context.runner = self.work, {}, self.native
        self.tools.run = context.run
        self.tools.database_verification_response(self.database, 'identity')
        actual, _ = self.receipt()
        receipt = json.loads(next(self.work.glob('command-*.json')).read_text(encoding='utf-8'))
        self.assertEqual(receipt['stdout_file'], actual['stdout_file'])

    def test_snapshot_error_is_sticky_and_not_empty_success(self):
        self.nested()
        error = OSError('snapshot unavailable')
        with patch.object(VerificationStdout, '_state', side_effect=[(1, 2, 0, 0, 0), error]):
            with self.assertRaises(OSError) as caught:
                self.tools.database_verification_response(self.database, 'identity')
        self.assertIs(caught.exception, error)
        for root in (self.work, self.work / 'session'):
            receipt = json.loads(next(root.glob('command-*.json')).read_text(encoding='utf-8'))
            self.assertIsNone(receipt['stdout'])
            self.assertEqual(receipt['stdout_capture_error'], 'OSError')
            self.assertNotIn('bytes', receipt['stdout_file'])

    def test_snapshot_failure_does_not_replace_original_subprocess_error(self):
        original = subprocess.CalledProcessError(8, ['fixed-command'], stderr=b'private')
        self.tools.run = Mock(side_effect=original)
        with patch.object(VerificationStdout, 'snapshot', side_effect=ValueError('capture error')):
            with self.assertRaises(subprocess.CalledProcessError) as caught:
                self.tools.database_verification_response(self.database, 'identity')
        self.assertIs(caught.exception, original)
        self.assertEqual(self.tools.run.call_count, 1)

    def test_exclusive_name_collision_cannot_overwrite_prior_file(self):
        with patch('restore_reference_io.uuid.uuid4', return_value=SimpleNamespace(hex='fixed')):
            path = self.work / 'mysql-identity-fixed.stdout'
            path.write_bytes(b'previous')
            with self.assertRaises(FileExistsError):
                VerificationStdout(self.work, 'identity', 'shared-control')
            self.assertEqual(path.read_bytes(), b'previous')

    def test_long_windows_output_path_preserves_stdout_and_receipt(self):
        work = self.work
        while len(str(work / 'mysql-ownership-shared-control-0123456789abcdef0123456789abcdef.stdout')) <= 270:
            work /= 'evidence-segment'
        os.makedirs(filesystem_path(work))
        self.addCleanup(shutil.rmtree, filesystem_path(work))
        with VerificationStdout(work, 'ownership', 'shared-control') as output:
            output.write(b'control\tscope\tmarker\n')
            raw, artifact = output.snapshot()
            receipt = output.receipt(0, stderr=b'')
        self.assertEqual(raw, b'control\tscope\tmarker\n')
        with open(filesystem_path(artifact['path']), 'rb') as stream:
            self.assertEqual(stream.read(), raw)
        self.assertTrue(os.path.isfile(filesystem_path(receipt['path'])))

    def test_late_same_size_change_cannot_return_earlier_success(self):
        original = VerificationStdout.snapshot
        calls = 0
        def snapshot(output):
            nonlocal calls
            calls += 1
            if calls == 2:
                output.stream.seek(0)
                output.write(b'uuid-2\tsource_control\n')
            return original(output)
        with patch.object(VerificationStdout, 'snapshot', snapshot), self.assertRaises(ValueError):
            self.tools.database_verification_response(self.database, 'identity')
        self.assertEqual(len(self.calls), 1)
        receipt = json.loads(next(self.work.glob('mysql-*.json')).read_text(encoding='utf-8'))
        self.assertEqual(receipt['stdout_capture_error'], 'ValueError')

    def test_late_path_replacement_is_rejected_or_refused_by_os(self):
        original = VerificationStdout.snapshot
        calls = 0
        def snapshot(output):
            nonlocal calls
            calls += 1
            if calls == 2:
                replacement = self.work / 'replacement-after-first'
                replacement.write_bytes(b'uuid-2\tsource_control\n')
                replacement.replace(output.path)
            return original(output)
        with patch.object(VerificationStdout, 'snapshot', snapshot), self.assertRaises((ValueError, PermissionError)):
            self.tools.database_verification_response(self.database, 'identity')
        self.assertEqual(len(self.calls), 1)

    def test_linked_path_rejected_before_creation(self):
        original = os.path.islink
        with patch('restore_reference_io.os.path.islink', lambda path: path == filesystem_path(self.work) or original(path)):
            with self.assertRaisesRegex(ValueError, '链接'):
                VerificationStdout(self.work, 'identity', 'shared-control')
        self.assertEqual(list(self.work.glob('*.stdout')), [])

    def test_relative_or_missing_work_is_not_normalized_into_permission(self):
        with self.assertRaises(ValueError):
            VerificationStdout(Path('relative-work'), 'identity', 'shared-control')
        with self.assertRaises(FileNotFoundError):
            VerificationStdout(self.work / 'absent-parent', 'identity', 'shared-control')

    def test_constructor_cleanup_failure_does_not_replace_identity_error(self):
        original = ValueError('file identity unavailable')
        stream = Mock()
        stream.close.side_effect = OSError('close unavailable')
        with patch('builtins.open', return_value=stream), patch.object(VerificationStdout, '_state', side_effect=original):
            with self.assertRaises(ValueError) as caught:
                VerificationStdout(self.work, 'identity', 'shared-control')
        self.assertIs(caught.exception, original)
        stream.close.assert_called_once_with()
        self.assertEqual(original.__notes__, ['MySQL 输出构造收尾失败：OSError'])

    def test_replaced_path_never_reads_replacement_and_changed_snapshot_is_rejected(self):
        with VerificationStdout(self.work, 'identity', 'shared-control') as output:
            output.write(b'original')
            output.snapshot()
            replacement = self.work / 'replacement'
            replacement.write_bytes(b'replaced')
            try:
                replacement.replace(output.path)
            except PermissionError:
                # Windows 打开的原句柄可能直接拒绝删除替换；仍必须验证后续原句柄字节漂移。
                output.stream.seek(0)
                output.write(b'modified')
            with self.assertRaises(ValueError):
                output.snapshot()

    def test_same_size_content_change_during_snapshot_fails_and_remains_failed(self):
        with VerificationStdout(self.work, 'identity', 'shared-control') as output:
            output.write(b'original')
            real_state = output._state
            calls = 0
            def state():
                nonlocal calls
                calls += 1
                if calls == 2:
                    output.stream.seek(0)
                    output.write(b'modified')
                return real_state()
            with patch.object(output, '_state', side_effect=state), self.assertRaises(ValueError):
                output.snapshot()
            raw, observed = verification_stdout(output, None)
            self.assertIsNone(raw)
            self.assertEqual(observed['stdout_capture_error'], 'ValueError')


if __name__ == '__main__':
    unittest.main()
