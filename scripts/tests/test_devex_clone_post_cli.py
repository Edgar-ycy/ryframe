"""统一 post-copy CLI 的显式登记与固定阶段输入，无外部服务。"""
import argparse
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import devex_clone_run_cli as cli


class CliTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.backend = Path(temporary.name).resolve()
        self.directory = self.backend / '.local-tests/run'
        self.directory.mkdir(parents=True)
        self.parser = argparse.ArgumentParser()
        cli.add_commands(self.parser.add_subparsers(dest='command', required=True))

    def args(self, mode, *extra):
        return self.parser.parse_args(['post-copy', '--backend-dir', str(self.backend),
            '--run-dir', str(self.directory), '--operation', mode, '--write', *extra])

    def test_fixed_stage_reuses_same_run_and_never_accepts_new_request(self):
        expected = {'status': 'stage_finished', 'stage': 'post-copy', 'mode': 'prepare', 'attempt': 3, 'restore_qualified': False}
        with patch.object(cli, 'execute', return_value=expected) as execute:
            self.assertEqual(cli.dispatch(self.args('prepare'), self.backend), expected)
        execute.assert_called_once_with(self.backend, self.directory, 'post-copy', 'prepare', post_copy_request=None, producer_binding=None)
        with self.assertRaises(ValueError):
            cli.dispatch(self.args('prepare', '--request', str(self.directory / 'other.json')), self.backend)

    def test_registration_requires_exact_request_and_explicit_write(self):
        for operation in ('register', 'amend'):
            with self.subTest(operation=operation), self.assertRaises(ValueError):
                cli.dispatch(self.args(operation), self.backend)
        with self.assertRaises(SystemExit):
            self.parser.parse_args(['post-copy', '--backend-dir', str(self.backend), '--run-dir', str(self.directory),
                                    '--operation', 'register', '--request', str(self.directory / 'request.json')])

    def test_amend_uses_same_stage_and_complete_request_argument(self):
        request = self.directory / 'corrected-request.json'
        expected = {'status': 'stage_finished', 'stage': 'post-copy', 'mode': 'amend',
                    'attempt': 24, 'restore_qualified': False}
        with patch.object(cli, 'execute', return_value=expected) as execute:
            self.assertEqual(cli.dispatch(self.args('amend', '--request', str(request)), self.backend), expected)
        execute.assert_called_once_with(self.backend, self.directory, 'post-copy', 'amend',
                                        post_copy_request=request, producer_binding=None)

    def test_session_recovery_requires_explicit_producer_binding(self):
        with self.assertRaises(ValueError): cli.dispatch(self.args('recover-session'), self.backend)
        with self.assertRaises(ValueError):
            cli.dispatch(self.args('verify', '--producer-binding', str(self.directory / 'owner.json')), self.backend)


if __name__ == '__main__':
    unittest.main()
