"""复制与接续入口的完整五桶 owner 门禁；复用真实本地计划/账本和离线协议替身。"""
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_factory as factory
import devex_clone_resume as resume
import test_devex_clone_factory as fixtures
from restore_build import file_digest
from restore_reference_plan import BUCKETS


class StageOwnersTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.FactoryTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.backend, self.output = self.case.backend, self.case.output

    def execute(self, runner=None):
        return factory.copy_to_fresh_target(self.backend, self.case.export_binding, self.case.initialized,
            self.case.environments['source'], self.case.environments['target'], self.output,
            copy_id='factory-case', stage='source_to_seed', run=runner or self.case.case.runner)

    def continue_copy(self, mode, runner=None):
        return resume.continue_copy(self.backend, self.output, self.case.environments['source'],
            self.case.environments['target'], mode=mode, run=runner or self.case.case.runner)

    def owner_commands(self, directory):
        return [command for command, _ in self.case.case.runner.calls
                if 's3api' in command and command[command.index('s3api') + 1] == 'get-object'
                and Path(command[-1]).parent == directory]

    def check_success_evidence(self, directory):
        commands = self.owner_commands(directory)
        self.assertEqual(len(commands), 5)
        self.assertEqual([command[command.index('--bucket') + 1] for command in commands], sorted(BUCKETS))
        self.assertTrue(all(command[command.index('--key') + 1] == 'clone-target/.ryframe-owner' for command in commands))
        value = json.loads((directory / 'verified.json').read_text(encoding='utf-8'))
        self.assertEqual(value['status'], 'target_owners_verified')
        self.assertEqual(value['scope_id'], 'clone-target')
        self.assertEqual(len(value['owners']), 5)
        for item in value['owners']:
            self.assertEqual(file_digest(directory / item['file']), {key: item[key] for key in ('bytes', 'sha256')})
        diagnostics = [json.loads(path.read_text(encoding='utf-8')) for path in self.output.glob('command-*.json')]
        self.assertEqual(sum(record['command'] in commands for record in diagnostics), 5)

    def bad_owner(self, command, **kwargs):
        result = self.case.case.runner(command, **kwargs)
        if ('s3api' in command and command[command.index('s3api') + 1] == 'get-object'
                and command[command.index('--bucket') + 1] == 'imports'
                and Path(command[-1]).parent.name.startswith('target-owners-')):
            Path(command[-1]).write_bytes(b'other-isolated-owner')
        return result

    def test_first_copy_checks_all_buckets_once_after_prepare_and_before_any_intent(self):
        events = []
        original_prepare = factory.CloneSession.prepare
        original_owners = factory.CloneSession.verify_target_owners
        def prepare(session, *arguments):
            original_prepare(session, *arguments)
            events.append('prepare')
        def owners(session):
            self.assertTrue((session.target_directory / 'initialize.lock').exists())
            self.assertFalse((session.output / 'ledger').exists())
            self.assertEqual(self.case.case.writes(), [])
            events.append('owners')
            original_owners(session)
        with patch.object(factory.CloneSession, 'prepare', prepare), \
                patch.object(factory.CloneSession, 'verify_target_owners', owners):
            result = self.execute()
        self.assertEqual(events, ['prepare', 'owners'])
        self.assertEqual(result['status'], 'data_steps_verified')
        directories = list(self.output.glob('target-owners-*'))
        self.assertEqual(len(directories), 1)
        self.check_success_evidence(directories[0])

    def test_bad_non_object_bucket_rejects_first_copy_before_ledger_or_intent(self):
        with patch.object(factory, 'TransferSteps') as steps, self.assertRaisesRegex(ValueError, 'ownership'):
            self.execute(self.bad_owner)
        steps.assert_not_called()
        self.assertFalse((self.output / 'ledger').exists())
        self.assertEqual(self.case.case.writes(), [])
        directory, = self.output.glob('target-owners-*')
        self.assertTrue((directory / 'failure.json').exists())
        self.assertFalse((directory / 'verified.json').exists())
        self.assertTrue(any(path.read_bytes() == b'other-isolated-owner' for path in directory.glob('owner-*.txt')))

    def test_reconcile_and_resume_each_check_once_after_hydrate(self):
        self.execute()
        old_hydrate = resume.hydrate
        old_owners = factory.CloneSession.verify_target_owners
        for mode in ('reconcile', 'resume'):
            with self.subTest(mode=mode):
                events = []
                before = set(self.output.glob('target-owners-*'))
                writes = len(self.case.case.writes())
                def hydrate(session, saved):
                    old_hydrate(session, saved)
                    events.append('hydrate')
                def owners(session):
                    self.assertTrue((session.target_directory / 'initialize.lock').exists())
                    self.assertEqual(len(self.case.case.writes()), writes)
                    events.append('owners')
                    old_owners(session)
                with patch.object(resume, 'hydrate', hydrate), patch.object(factory.CloneSession, 'verify_target_owners', owners):
                    result = self.continue_copy(mode)
                self.assertEqual(events, ['hydrate', 'owners'])
                self.assertEqual(result['status'], 'copy_reconciled' if mode == 'reconcile' else 'data_steps_verified')
                directory, = set(self.output.glob('target-owners-*')) - before
                self.check_success_evidence(directory)
                self.assertEqual(len(self.case.case.writes()), writes)

    def test_bad_other_bucket_on_continuation_preserves_original_ledger_and_no_new_intent(self):
        self.execute()
        head = self.output / 'ledger/head.json'
        original, writes = head.read_bytes(), len(self.case.case.writes())
        before = set(self.output.glob('target-owners-*'))
        with patch.object(resume, 'TransferSteps') as steps, self.assertRaisesRegex(ValueError, 'ownership'):
            self.continue_copy('reconcile', self.bad_owner)
        steps.assert_not_called()
        self.assertEqual(head.read_bytes(), original)
        self.assertEqual(len(self.case.case.writes()), writes)
        directory, = set(self.output.glob('target-owners-*')) - before
        self.assertTrue((directory / 'failure.json').exists())
        self.assertFalse((directory / 'verified.json').exists())

    def test_environment_drift_during_stage_owner_reads_cannot_publish_or_begin_copy(self):
        original = dict(os.environ)
        def drift(command, **kwargs):
            response = self.case.case.runner(command, **kwargs)
            if 's3api' in command and Path(command[-1]).parent.name.startswith('target-owners-'):
                os.environ['CLONE_SIDE'] = 'changed-during-owner-read'
            return response
        with patch.object(factory, 'TransferSteps') as steps, self.assertRaisesRegex(ValueError, '环境'):
            self.execute(drift)
        self.assertEqual(dict(os.environ), original)
        steps.assert_not_called()
        self.assertEqual(self.case.case.writes(), [])
        directory, = self.output.glob('target-owners-*')
        self.assertTrue((directory / 'failure.json').exists())
        self.assertFalse((directory / 'verified.json').exists())


if __name__ == '__main__':
    unittest.main()
