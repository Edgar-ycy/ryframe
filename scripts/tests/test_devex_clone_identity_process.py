"""身份生产者与原阶段、明确计划、账本及进程身份绑定；不连接服务。"""
import copy
import json
import os
from pathlib import Path
from types import SimpleNamespace
import unittest
from tests.workspace_directory import WorkspaceDirectory
from unittest.mock import patch

import devex_clone_post_process as process
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import begin, binding, bind_controller_attempt, finish, initialize_state, load_state, run_lock
from full_stack_process import write_receipt
from restore_build import file_digest


class IdentityProducerTests(unittest.TestCase):
    def setUp(self):
        self.backend = next(path for path in Path(__file__).resolve().parents if (path / 'Cargo.toml').is_file())
        temporary = WorkspaceDirectory(dir=self.backend / '.local-tests/tmp', prefix='identity-producer-')
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name).resolve()
        write_json(self.directory / 'manifest.json', {'kind': 'isolated-producer-fixture'})
        initialize_state(self.directory)
        self.node = Path('D:/Program Files/nodejs/node.exe') if os.name == 'nt' else Path('/usr/bin/node')
        self.plan_sha256 = 'a' * 64
        write_json(self.directory / 'identity-plan.json', {
            'kind': 'fixture-plan', 'plan_sha256': self.plan_sha256})
        self.request = {'node': {'path': str(self.node), 'sha256': file_digest(self.node)['sha256']},
                        'identity_plan': binding(self.directory / 'identity-plan.json'),
                        'identity_state': str(self.directory / 'seed-runtime/identities')}
        write_json(self.directory / 'seed-runtime.json', self.request)
        self.number = begin(self.directory, 'seed-runtime', 'identities-apply', {})
        with run_lock(self.directory) as owner:
            bind_controller_attempt(self.directory, self.number, owner)
        self.output = self.directory / 'seed-runtime/attempt-0001'
        self.output.mkdir(parents=True)
        self.controller = read_json(self.directory / 'controller-0001.json')
        self.controller['owner']['identity'] = {**self.controller['owner']['identity'], 'pid': 2147482901}
        write_receipt(self.directory / 'controller-0001.json', self.controller)
        self.identity = {'pid': 2147482902, 'started': 'fixture-node', 'executable': str(self.node)}
        self.lock_owner = '10000000-0000-4000-8000-000000000001'
        self.addCleanup(patch.stopall)
        patch('devex_clone_seed.registered', return_value=self.request).start()

    def record(self):
        self.command = process.producer_command(self.backend, self.request, self.directory, 1, 'identity-apply', self.output)
        self.intent = {'format_version': 1, 'kind': 'devex-post-producer-launch',
            'run_manifest': binding(self.directory / 'manifest.json'), 'registration': binding(self.directory / 'seed-runtime.json'),
            'controller': binding(self.directory / 'controller-0001.json'), 'attempt': 1, 'producer_kind': 'identity-apply',
            'command': self.command, 'node': self.request['node'], 'script': binding(Path(self.command[1]))}
        write_json(self.output / 'session-launch.json', self.intent)
        self.value = {'format_version': 1, 'kind': 'devex-post-producer',
                      'launch': binding(self.output / 'session-launch.json'), 'identity': self.identity,
                      'identity_lock_owner': self.lock_owner}
        write_json(self.output / 'session-process.json', self.value)
        finish(self.directory, 1, error=RuntimeError('fixture interrupted controller'))
        return binding(self.output / 'session-process.json')

    def context(self):
        return SimpleNamespace(backend=self.backend, directory_root=self.directory, output=self.output,
            request=self.request, request_binding=binding(self.directory / 'seed-runtime.json'), private={'target_api': {}})

    def identity_lock(self, **updates):
        root = Path(self.request['identity_state'])
        root.mkdir(parents=True, exist_ok=True)
        value = {'format_version': 1, 'kind': 'devex-identity-run-lock', 'pid': self.identity['pid'],
                 'plan_sha256': self.plan_sha256, 'owner_token': self.lock_owner}
        value.update(updates)
        path = root / 'lock'
        path.write_bytes((json.dumps(value, ensure_ascii=False, indent=2) + '\n').encode())
        return path

    def test_commands_bind_both_modes_plan_and_state_without_private_values(self):
        for mode in ('apply', 'verify'):
            command = process.producer_command(self.backend, self.request, self.directory, 1, 'identity-' + mode, self.output)
            self.assertEqual(command[2:], ['--run-dir', str(self.directory), '--attempt', '1',
                '--identity-plan', self.request['identity_plan']['path'], '--identity-state', self.request['identity_state'],
                '--identity-mode', mode])
        with self.assertRaises(ValueError):
            process.producer_command(self.backend, self.request, self.directory, 1, 'unknown', self.output)

    def test_mode_stage_output_and_registration_rejected_before_spawn(self):
        with patch.object(process.subprocess, 'Popen') as spawn:
            for kind in ('identity-verify', 'existing', 'session', 'unknown'):
                with self.subTest(kind=kind), self.assertRaises(ValueError):
                    process.Producer(self.context(), kind)
            context = self.context()
            context.output = self.directory / 'post-copy/attempt-0001'
            with self.assertRaises(ValueError):
                process.Producer(context, 'identity-apply')
            context = self.context()
            context.request_binding = {**context.request_binding, 'sha256': '0' * 64}
            with self.assertRaises(ValueError):
                process.Producer(context, 'identity-apply')
            context = self.context()
            context.request = {**self.request, 'identity_state': str(self.directory / 'other-state')}
            with self.assertRaises(ValueError):
                process.Producer(context, 'identity-apply')
            spawn.assert_not_called()

    def test_old_seed_producer_blocks_new_post_copy_action(self):
        self.record()
        number = begin(self.directory, 'post-copy', 'verify', {})
        with patch.object(process, 'process_identity', return_value=self.identity), \
                patch.object(process, 'actual_argv', return_value=self.command), self.assertRaisesRegex(ValueError, '旧 Node'):
            process.require_quiet(self.backend, self.directory, number)

    def test_stopped_identity_receipt_before_lock_owner_recording_remains_observable(self):
        self.record()
        historical = copy.deepcopy(self.value)
        historical.pop('identity_lock_owner')
        write_receipt(self.output / 'session-process.json', historical)
        attempt = load_state(self.directory)['attempts'][0]
        with patch.object(process, 'process_identity', return_value=None):
            observed = process.inspect_producer(self.backend, self.directory, attempt)
        self.assertFalse(observed['alive'])
        self.assertNotIn('identity_lock_owner', observed['value'])

    def test_identity_receipt_without_owner_never_cleans_a_remaining_lock(self):
        self.record()
        historical = copy.deepcopy(self.value)
        historical.pop('identity_lock_owner')
        write_receipt(self.output / 'session-process.json', historical)
        descriptor = binding(self.output / 'session-process.json')
        lock = self.identity_lock()
        before = lock.read_bytes()
        actual = process.process_identity
        with patch.object(process, 'process_identity',
                          side_effect=lambda pid: actual(pid) if pid == os.getpid() else None), \
                self.assertRaisesRegex(ValueError, '未记录锁 owner'):
            self.recovery(descriptor)
        self.assertEqual(lock.read_bytes(), before)

    def test_actual_argv_other_plan_state_mode_run_and_attempt_fail(self):
        self.record()
        attempt = load_state(self.directory)['attempts'][0]
        for flag in ('--identity-plan', '--identity-state', '--identity-mode', '--run-dir', '--attempt'):
            changed = self.command.copy()
            changed[changed.index(flag) + 1] = 'other'
            with self.subTest(flag=flag), patch.object(process, 'process_identity', return_value=self.identity), \
                    patch.object(process, 'actual_argv', return_value=changed), self.assertRaisesRegex(ValueError, '实际参数'):
                process.inspect_producer(self.backend, self.directory, attempt)

    def test_pid_reuse_and_registration_substitution_fail(self):
        self.record()
        attempt = load_state(self.directory)['attempts'][0]
        with patch.object(process, 'process_identity', return_value={**self.identity, 'started': 'other'}), self.assertRaises(ValueError):
            process.inspect_producer(self.backend, self.directory, attempt)
        modified = copy.deepcopy(self.intent)
        modified['registration']['sha256'] = '0' * 64
        write_receipt(self.output / 'session-launch.json', modified)
        write_receipt(self.output / 'session-process.json', {**self.value, 'launch': binding(self.output / 'session-launch.json')})
        with self.assertRaises(ValueError):
            process.inspect_producer(self.backend, self.directory, attempt)

    def recovery(self, descriptor, stage='seed-runtime'):
        with run_lock(self.directory) as owner:
            number = begin(self.directory, stage, 'recover-session', {}, verify_results=False)
            bind_controller_attempt(self.directory, number, owner, verify_results=False)
            return process.recover_session(self.backend, self.directory, number, descriptor)

    def test_exact_seed_recovery_keeps_unknown_identity_ledger(self):
        descriptor = self.record()
        ledger = Path(self.request['identity_state'])
        ledger.mkdir()
        write_json(ledger / 'ledger.json', {'status': 'applying', 'entries': [{'phase': 'intent'}]})
        lock = self.identity_lock()
        before = binding(ledger / 'ledger.json')
        alive = [True]
        actual = process.process_identity
        def identity(pid):
            if pid == os.getpid():
                return actual(pid)
            return self.identity if pid == self.identity['pid'] and alive[0] else None
        with patch.object(process, 'process_identity', side_effect=identity), \
                patch.object(process, 'actual_argv', return_value=self.command), \
                patch.object(process, 'terminate_owned_process', side_effect=lambda _: alive.__setitem__(0, False)) as stop:
            result = self.recovery(descriptor)
        self.assertEqual(result['status'], 'seed_session_recovered')
        self.assertTrue(result['copy_requires_reconciliation'])
        self.assertEqual(binding(ledger / 'ledger.json'), before)
        self.assertFalse(lock.exists())
        releases = list((ledger / 'lock-releases').iterdir())
        self.assertEqual(len([item for item in releases if item.suffix == '.lock']), 1)
        self.assertEqual(len([item for item in releases if item.name.endswith('.released.json')]), 1)
        stop.assert_called_once_with(self.identity)

    def test_identity_recovery_preserves_pid_and_plan_mismatch_locks(self):
        for update in ({'pid': self.identity['pid'] + 1}, {'plan_sha256': 'b' * 64}):
            with self.subTest(update=update):
                lock = self.identity_lock(**update)
                before = lock.read_bytes()
                with self.assertRaises(ValueError):
                    process.recover_identity_lock(self.backend, self.request, self.identity, self.lock_owner)
                self.assertEqual(lock.read_bytes(), before)
                lock.unlink()

    def test_recover_session_preserves_lock_with_other_owner_token(self):
        descriptor = self.record()
        lock = self.identity_lock(owner_token='20000000-0000-4000-8000-000000000002')
        before = lock.read_bytes()
        actual = process.process_identity
        with patch.object(process, 'process_identity',
                          side_effect=lambda pid: actual(pid) if pid == os.getpid() else None), \
                self.assertRaises(ValueError):
            self.recovery(descriptor)
        self.assertEqual(lock.read_bytes(), before)

    def test_identity_recovery_preserves_link_and_owner_token_replacement(self):
        lock = self.identity_lock()
        actual_linked = process.linked
        with patch.object(process, 'linked', side_effect=lambda path: path == lock or actual_linked(path)), \
                self.assertRaises(ValueError):
            process.recover_identity_lock(self.backend, self.request, self.identity, self.lock_owner)
        self.assertTrue(lock.exists())
        original = process._identity_lock_snapshot
        calls = [0]
        def replace(path):
            calls[0] += 1
            if calls[0] == 2:
                self.identity_lock(owner_token='20000000-0000-4000-8000-000000000002')
            return original(path)
        with patch.object(process, '_identity_lock_snapshot', side_effect=replace), self.assertRaises(ValueError):
            process.recover_identity_lock(self.backend, self.request, self.identity, self.lock_owner)
        self.assertIn(b'20000000-0000-4000-8000-000000000002', lock.read_bytes())

    def test_identity_recovery_publishes_permanent_release_and_is_idempotent(self):
        lock = self.identity_lock()
        self.assertTrue(process.recover_identity_lock(
            self.backend, self.request, self.identity, self.lock_owner))
        self.assertFalse(lock.exists())
        release = lock.parent / 'lock-releases'
        before = {item.name: item.read_bytes() for item in release.iterdir()}
        self.assertEqual(len([name for name in before if name.endswith('.lock')]), 1)
        self.assertEqual(len([name for name in before if name.endswith('.released.json')]), 1)
        self.assertFalse(process.recover_identity_lock(
            self.backend, self.request, self.identity, self.lock_owner))
        self.assertEqual({item.name: item.read_bytes() for item in release.iterdir()}, before)

    def test_identity_recovery_preserves_replacement_across_both_claim_boundaries(self):
        lock = self.identity_lock()
        replacement = b'{"replacement":"before-claim"}\n'
        def before(stage, _paths):
            if stage == 'before-claim':
                lock.unlink()
                lock.write_bytes(replacement)
        with patch.object(process, '_identity_lock_release_checkpoint', side_effect=before), \
                self.assertRaisesRegex(ValueError, 'claim.*替换'):
            process.recover_identity_lock(self.backend, self.request, self.identity, self.lock_owner)
        releases = list((lock.parent / 'lock-releases').iterdir())
        self.assertEqual(len(releases), 1)
        self.assertEqual(releases[0].read_bytes(), replacement)
        self.assertFalse(lock.exists())

        other = self.directory / 'second-identities'
        self.request['identity_state'] = str(other)
        lock = self.identity_lock()
        replacement = b'{"replacement":"after-claim"}\n'
        def claimed(stage, _paths):
            if stage == 'claimed':
                lock.write_bytes(replacement)
        with patch.object(process, '_identity_lock_release_checkpoint', side_effect=claimed):
            process.recover_identity_lock(self.backend, self.request, self.identity, self.lock_owner)
        self.assertEqual(lock.read_bytes(), replacement)
        releases = list((other / 'lock-releases').iterdir())
        self.assertEqual(len([item for item in releases if item.suffix == '.lock']), 1)
        self.assertEqual(len([item for item in releases if item.name.endswith('.released.json')]), 1)
        self.assertFalse(process.recover_identity_lock(
            self.backend, self.request, self.identity, self.lock_owner))
        self.assertEqual(lock.read_bytes(), replacement)

    def test_identity_recovery_resumes_baseexception_at_all_release_boundaries(self):
        for boundary in ('before-claim', 'claimed', 'released'):
            with self.subTest(boundary=boundary):
                root = self.directory / f'{boundary}-identities'
                self.request['identity_state'] = str(root)
                lock = self.identity_lock()
                def crash(stage, _paths):
                    if stage == boundary:
                        raise KeyboardInterrupt(f'fixture {boundary}')
                with patch.object(process, '_identity_lock_release_checkpoint', side_effect=crash), \
                        self.assertRaises(KeyboardInterrupt):
                    process.recover_identity_lock(self.backend, self.request, self.identity, self.lock_owner)
                self.assertEqual(lock.exists(), boundary == 'before-claim')
                process.recover_identity_lock(self.backend, self.request, self.identity, self.lock_owner)
                files = list((root / 'lock-releases').iterdir())
                self.assertEqual(len([item for item in files if item.suffix == '.lock']), 1)
                self.assertEqual(len([item for item in files if item.name.endswith('.released.json')]), 1)
                self.assertFalse(any(item.name.endswith('.pending') for item in files))

    def test_post_recovery_cannot_select_seed_producer(self):
        descriptor = self.record()
        with patch.object(process, 'terminate_owned_process') as stop, self.assertRaises(ValueError):
            self.recovery(descriptor, 'post-copy')
        stop.assert_not_called()

    def test_missing_identity_receipt_never_guesses_pid(self):
        self.record()
        (self.output / 'session-process.json').unlink()
        with patch.object(process, 'process_identity') as identity, self.assertRaisesRegex(ValueError, '缺少内核身份'):
            process.require_quiet(self.backend, self.directory)
        identity.assert_not_called()


if __name__ == '__main__':
    unittest.main()
