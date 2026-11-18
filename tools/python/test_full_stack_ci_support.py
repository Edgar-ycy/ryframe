"""验证历史配置复用的私有文件传输。"""
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch
from contextlib import nullcontext

sys.path.insert(0,str(Path(__file__).resolve().parent))
import full_stack_ci_support as support



class SupportTests(unittest.TestCase):
    def test_file_transport_keeps_plaintext_in_stdin_only_and_checks_exit_status(self):
        directory = Path.cwd() / 'private'
        private_config = "password='private-password'"
        directory_response = Mock(returncode=0, stdout=b'{"ok":true,"result":{}}')
        response=Mock(returncode=0,stdout=b'{"ok":true,"result":{"text":"rendered"}}')
        with patch('rate_limit_tool.tool_binding',return_value={'path':'/native-tool'}), \
                patch('rate_limit_tool.verified_tool',return_value=nullcontext('/native-tool')), \
                patch('subprocess.run', side_effect=[directory_response, response]) as run:
            support.configuration_directory(directory)
            self.assertEqual(support.instance_text(private_config, 2), 'rendered')
        calls = [
            ('private-directory', {'path': str(directory)}),
            ('instance-text', {'text': private_config, 'worker_id': 2}),
        ]
        for call, (operation, request) in zip(run.call_args_list, calls, strict=True):
            self.assertEqual(call.args[0], ['/native-tool', '__ci-databases', operation])
            self.assertEqual(json.loads(call.kwargs['input']), request)
            self.assertNotIn('private-password', str(call.args))
            self.assertNotIn('private-password', str(call.kwargs['env']))
        for changed in [Mock(returncode=1,stdout=response.stdout),
                        Mock(returncode=0,stdout=b'{"ok":false,"error":"private-password"}'),
                        Mock(returncode=0,stdout=b'{"ok":true,"ok":true,"result":{}}'),
                        Mock(returncode=0,stdout=b'invalid')]:
            with patch('rate_limit_tool.tool_binding',return_value={'path':'/native-tool'}), \
                    patch('rate_limit_tool.verified_tool',return_value=nullcontext('/native-tool')), \
                    patch('subprocess.run',return_value=changed),self.assertRaises(RuntimeError) as raised:
                support.instance_text(private_config, 2)
            self.assertNotIn('private-password',str(raised.exception))

    def test_missing_file_tool_binding_never_starts_a_runner(self):
        for operation, request in (
            ('private-directory', {'path': str(Path.cwd() / 'private')}),
            ('instance-text', {'text': 'private-password', 'worker_id': 2}),
        ):
            with self.subTest(operation=operation), \
                    patch('rate_limit_tool.tool_binding', return_value=None), \
                    patch('subprocess.run') as run, \
                    self.assertRaisesRegex(ValueError, '正式 xtask 或 run_checks.py') as raised:
                support._configuration(operation, request)
            run.assert_not_called()
            self.assertNotIn('private-password', str(raised.exception))
