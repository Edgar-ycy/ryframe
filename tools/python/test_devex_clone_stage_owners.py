"""首次复制的对象归属拒绝与环境漂移回归。"""
import json
import os
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_factory as factory
import test_devex_clone_factory as fixtures


class StageOwnersTests(unittest.TestCase):
    def setUp(self):
        self.case = fixtures.FactoryTests()
        self.case.setUp()
        self.addCleanup(self.case.doCleanups)
        self.backend, self.output = self.case.backend, self.case.output

    def execute(self, runner=None):
        return factory.copy_to_fresh_target(self.backend, self.case.export_binding, self.case.initialized,
            self.case.environments['source'], self.case.environments['target'], self.output,
            copy_id='factory-case', stage='source_to_seed', run=runner or self.case.case.runner,
            source_storage_run=self.case.run_dir)

    def bad_owner(self, command, **kwargs):
        result = self.case.case.runner(command, **kwargs)
        if '__s3-read' in command:
            request = json.loads(kwargs['input'])
            if (request['operation'] == 'get-object' and request['bucket'] == 'imports'
                    and Path(request['destination']).parent.name.startswith('target-owners-')):
                Path(request['destination']).write_bytes(b'other-isolated-owner')
        return result

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

    def test_environment_drift_during_stage_owner_reads_cannot_publish_or_begin_copy(self):
        original = dict(os.environ)
        def drift(command, **kwargs):
            response = self.case.case.runner(command, **kwargs)
            if '__s3-read' in command:
                request = json.loads(kwargs['input'])
                if Path(request['destination']).parent.name.startswith('target-owners-'):
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
