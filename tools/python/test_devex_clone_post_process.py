"""Node 残留生产者的明确收据、内核身份和故障窗口，不连接外部服务。"""
import copy
import io
import os
from pathlib import Path
import sys
import threading
import time
from types import SimpleNamespace
import unittest
from workspace_directory import WorkspaceDirectory
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_post_process as process
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import (begin, binding, bind_controller_attempt, finish, initialize_state, run_lock)
from full_stack_process import write_receipt
from restore_build import file_digest


class ProducerTests(unittest.TestCase):
    def setUp(self):
        self.backend = next(path for path in Path(__file__).resolve().parents if (path / 'Cargo.toml').is_file())
        temporary = WorkspaceDirectory(dir=self.backend / '.local-tests/tmp', prefix='post-producer-')
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        write_json(self.directory / 'manifest.json', {'id': 'fixture-only'})
        initialize_state(self.directory)
        node = Path('D:/Program Files/nodejs/node.exe') if os.name == 'nt' else Path(os.path.realpath('/usr/bin/node'))
        self.request = {'node': {'path': str(node), 'sha256': file_digest(node)['sha256']},
                        'reference_plan': {'path': str(self.directory / 'plan.json')},
                        'dataset': {'path': str(self.directory / 'dataset.json')}}
        write_json(self.directory / 'post-copy.json', self.request)
        self.number = begin(self.directory, 'post-copy', 'schedules', {})
        with run_lock(self.directory) as owner:
            bind_controller_attempt(self.directory, self.number, owner)
        self.output = self.directory / 'post-copy/attempt-0001'
        self.output.mkdir(parents=True)
        self.controller = read_json(self.directory / 'controller-0001.json')
        self.controller['owner']['identity'] = {**self.controller['owner']['identity'], 'pid': 2147482999}
        write_receipt(self.directory / 'controller-0001.json', self.controller)
        self.identity = {'pid': 2147483000, 'started': 'fixture-child', 'executable': str(node)}
        self.addCleanup(patch.stopall)
        patch('devex_clone_post.registered', return_value=self.request).start()
        expected_registration = binding(self.directory / 'post-copy.json')
        def registration(_backend, _directory, *, descriptor=None, **_):
            if descriptor is not None and descriptor != expected_registration:
                raise ValueError('fixture registration differs')
            return SimpleNamespace(request=self.request, descriptor=expected_registration)
        patch('devex_clone_post.registration', side_effect=registration).start()

    def record(self, kind='session'):
        command = process.producer_command(self.backend, self.request, self.directory, 1, kind, self.output)
        self.intent = {'format_version': 1, 'kind': 'devex-post-producer-launch', 'run_manifest': binding(self.directory / 'manifest.json'),
            'registration': binding(self.directory / 'post-copy.json'), 'controller': binding(self.directory / 'controller-0001.json'),
            'attempt': 1, 'producer_kind': kind, 'command': command, 'node': self.request['node'], 'script': binding(Path(command[1]))}
        write_json(self.output / 'session-launch.json', self.intent)
        self.value = {'format_version': 1, 'kind': 'devex-post-producer', 'launch': binding(self.output / 'session-launch.json'), 'identity': self.identity}
        write_json(self.output / 'session-process.json', self.value)
        finish(self.directory, 1, error=RuntimeError('fixture parent interrupted'))
        return binding(self.output / 'session-process.json')

    def test_dead_receipt_retained_and_latest_failed_attempt_is_inspected(self):
        self.record()
        with patch.object(process, 'process_identity', return_value=None):
            process.require_quiet(self.backend, self.directory)
        self.assertTrue((self.output / 'session-process.json').exists())

    def test_historical_dead_producer_survives_current_tool_change_or_removal(self):
        self.record()
        with patch.object(process, 'external_file', side_effect=FileNotFoundError('old tool removed')) as tool, \
                patch.object(process, 'process_identity', return_value=None):
            process.require_quiet(self.backend, self.directory)
        tool.assert_not_called()

    def test_live_orphan_recovery_uses_historical_identity_even_when_tool_bytes_changed(self):
        descriptor = self.record()
        live = [True]
        def identity(pid):
            return self.identity if pid == self.identity['pid'] and live[0] else None
        with patch.object(process, 'external_file', side_effect=ValueError('current tool changed')) as tool, \
                patch.object(process, 'actual_argv', return_value=self.intent['command']), \
                patch.object(process, 'terminate_owned_process', side_effect=lambda _: live.__setitem__(0, False)):
            result = self.recovery(descriptor, identity)
        self.assertFalse(result['already_stopped'])
        tool.assert_not_called()

    def test_live_identity_reused_pid_and_other_run_argv_are_all_rejected(self):
        self.record()
        for actual, argv in [(self.identity, self.intent['command']),
                             ({**self.identity, 'started': 'other'}, self.intent['command']),
                             (self.identity, [*self.intent['command'][:-3], 'other-run', '--attempt', '1'])]:
            with self.subTest(actual=actual['started'], argv_matches=argv == self.intent['command']), \
                    patch.object(process, 'process_identity', return_value=actual), \
                    patch.object(process, 'actual_argv', return_value=argv), self.assertRaises(ValueError):
                process.require_quiet(self.backend, self.directory)

    def test_newer_numeric_pid_generation_is_not_treated_as_old_live_producer(self):
        self.identity["started"] = "1000"
        self.record()
        current = {**self.identity, "started": "2000"}
        with patch.object(process, "process_identity", return_value=current), \
                patch.object(process, "actual_argv") as argv:
            process.require_quiet(self.backend, self.directory)
        argv.assert_not_called()

    def test_missing_kernel_receipt_never_scans_or_guesses(self):
        self.record()
        (self.output / 'session-process.json').unlink()
        with patch.object(process, 'process_identity') as identity, self.assertRaisesRegex(ValueError, '缺少内核身份'):
            process.require_quiet(self.backend, self.directory)
        identity.assert_not_called()

    def test_other_attempt_registration_and_controller_replacements_fail(self):
        self.record()
        original = copy.deepcopy(self.intent)
        for field, value in [('attempt', 2), ('registration', {'sha256': 'f' * 64}),
                              ('command', [*self.intent['command'][:-1], '99'])]:
            with self.subTest(field=field):
                write_receipt(self.output / 'session-launch.json', {**original, field: value})
                write_receipt(self.output / 'session-process.json', {**self.value, 'launch': binding(self.output / 'session-launch.json')})
                with self.assertRaises(ValueError):
                    process.require_quiet(self.backend, self.directory)

    def recovery(self, descriptor, identity):
        with run_lock(self.directory) as owner:
            number = begin(self.directory, 'post-copy', 'recover-session', {}, verify_results=False)
            bind_controller_attempt(self.directory, number, owner, verify_results=False)
            original = process.process_identity
            with patch.object(process, 'process_identity', side_effect=lambda pid: original(pid) if pid == os.getpid() else identity(pid)):
                result = process.recover_session(self.backend, self.directory, number, descriptor)
            finish(self.directory, number, result=result, verify_results=False)
            return result

    def test_exact_live_recovery_and_duplicate_dead_recovery_keep_old_intents(self):
        descriptor = self.record()
        write_json(self.output / 'unknown-intent.json', {'status': 'unknown'})
        alive = [True]
        def identity(pid):
            return self.identity if pid == self.identity['pid'] and alive[0] else None
        def terminate(value):
            self.assertEqual(value, self.identity)
            alive[0] = False
        with patch.object(process, 'actual_argv', return_value=self.intent['command']), \
                patch.object(process, 'terminate_owned_process', side_effect=terminate) as stop:
            first = self.recovery(descriptor, identity)
            second = self.recovery(descriptor, identity)
        self.assertFalse(first['already_stopped'])
        self.assertTrue(second['already_stopped'])
        self.assertTrue(second['copy_requires_reconciliation'])
        self.assertTrue((self.output / 'unknown-intent.json').exists())
        stop.assert_called_once()

    def test_changed_explicit_binding_cannot_recover_other_process(self):
        descriptor = self.record()
        descriptor['sha256'] = '0' * 64
        with patch.object(process, 'terminate_owned_process') as stop, self.assertRaises(ValueError):
            self.recovery(descriptor, lambda _: None)
        stop.assert_not_called()

    def test_controller_still_alive_blocks_recovery(self):
        descriptor = self.record()
        with self.assertRaisesRegex(ValueError, '原控制进程仍存在'):
            self.recovery(descriptor, lambda pid: self.controller['owner']['identity'] if pid == self.controller['owner']['identity']['pid'] else None)

    def test_node_launch_without_current_controller_lock_is_rejected(self):
        from types import SimpleNamespace

        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.output,
                                  request=self.request, request_binding=binding(self.directory / 'post-copy.json'))
        with self.assertRaises((FileNotFoundError, ValueError)):
            process.Producer(context, 'session')

    def test_initialization_cleanup_failure_preserves_original_identity_error(self):
        from types import SimpleNamespace

        context = SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.output,
            request=self.request, request_binding=binding(self.directory / 'post-copy.json'), private={'target_api': {}})
        def failed_cleanup(instance):
            instance.log.close()
            raise RuntimeError('cleanup')
        with run_lock(self.directory) as owner:
            write_receipt(self.directory / 'controller-0001.json', {**self.controller, 'owner': owner})
            with patch.object(process.subprocess, 'Popen') as popen, patch.object(process.Producer, 'close', autospec=True, side_effect=failed_cleanup):
                child_pid = popen.return_value.pid = 2147483001
                original = process.process_identity
                with patch.object(process, 'process_identity', side_effect=lambda pid: None if pid == child_pid else original(pid)):
                    with self.assertRaisesRegex(ValueError, '内核工具身份不符'):
                        process.Producer(context, 'session')
        self.assertEqual(read_json(self.output / 'session-cleanup-error.json')['error_type'], 'RuntimeError')

    def test_failed_identity_cleanup_does_not_block_on_reader_stream_lock(self):
        from types import SimpleNamespace

        read_fd, write_fd = os.pipe()
        reader = os.fdopen(read_fd, 'rb')
        entered = threading.Event()
        def wait_response():
            entered.set()
            reader.readline()
        thread = threading.Thread(target=wait_response, daemon=True)
        thread.start()
        entered.wait(timeout=1)
        producer = process.Producer.__new__(process.Producer)
        producer.identity, producer.log = self.identity, io.BytesIO()
        producer.child = SimpleNamespace(poll=lambda: None, stdin=io.BytesIO(), stdout=reader)
        started = time.monotonic()
        try:
            with patch.object(process, 'terminate_owned_process', side_effect=ValueError('身份不确定')):
                with self.assertRaisesRegex(ValueError, '身份不确定'):
                    producer.close()
            self.assertLess(time.monotonic() - started, 1)
            self.assertFalse(reader.closed)
            self.assertFalse(producer.child.stdin.closed)
            self.assertTrue(producer.log.closed)
        finally:
            os.write(write_fd, b'\n')
            os.close(write_fd)
            thread.join(timeout=1)
            reader.close()
            producer.child.stdin.close()

    def test_diagnostic_disk_error_cannot_replace_original_unknown_write(self):
        from types import SimpleNamespace

        original = TimeoutError('original unknown write')
        with patch.object(process, 'write_json', side_effect=OSError('disk full')):
            process.cleanup_failure(SimpleNamespace(output=self.output), 'cleanup', ValueError('close failed'), original)
        self.assertEqual(str(original), 'original unknown write')
        self.assertIn('OSError', original.__notes__[0])


if __name__ == '__main__':
    unittest.main()
