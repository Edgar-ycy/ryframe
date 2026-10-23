"""仅验证底层端口，不启动产品或基础设施服务。"""
import io
import json
from pathlib import Path
import sys
import unittest
from unittest.mock import Mock, patch
from contextlib import nullcontext

sys.path.insert(0,str(Path(__file__).resolve().parent))
import full_stack_ci_support as support



class SupportTests(unittest.TestCase):
    def test_versions_fields_unknown_operations_and_relative_paths_fail_closed(self):
        value={'format_version':1,'operation':'unknown','backend':str(Path.cwd()),'directory':str(Path.cwd()),'tool':{},'arguments':{}}
        with patch('rate_limit_tool.install_binding'):
            for changed in [{**value,'format_version':True},{**value,'extra':'private-password'},{**value,'backend':'relative'},value]:
                with self.subTest(changed=changed),self.assertRaises(ValueError):support.execute(changed)

    def test_duplicate_protocol_keys_are_rejected(self):
        with self.assertRaises(ValueError):json.loads('{"operation":"a","operation":"b"}',object_pairs_hook=support.unique)

    def test_main_never_exposes_private_exception_and_rejects_argv(self):
        class Input:
            buffer=io.BytesIO(b'{}')
        with patch.object(sys,'argv',['support']),patch.object(sys,'stdin',Input()),patch.object(support,'execute',side_effect=ValueError('private-password')),patch('builtins.print') as output:
            self.assertEqual(support.main(),1)
        self.assertNotIn('private-password',str(output.call_args))
        self.assertIn('ValueError',str(output.call_args))
        with patch.object(sys,'argv',['support','untrusted']),patch('builtins.print'):
            self.assertEqual(support.main(),2)

    def test_file_helpers_only_forward_private_protocol(self):
        with patch.object(support, '_configuration', return_value={'text':'rendered'}) as native:
            self.assertEqual(support.instance_text('secret config', 2), 'rendered')
            native.assert_called_once_with('instance-text', {'text':'secret config','worker_id':2})
        with patch.object(support, '_configuration') as native:
            support.configuration_directory(Path.cwd() / 'private')
            native.assert_called_once_with('private-directory', {'path':str(Path.cwd() / 'private')})

    def test_file_transport_keeps_plaintext_in_stdin_only_and_checks_exit_status(self):
        response=Mock(returncode=0,stdout=b'{"ok":true,"result":{"text":"rendered"}}')
        with patch('rate_limit_tool.tool_binding',return_value={'path':'/native-tool'}), \
                patch('rate_limit_tool.verified_tool',return_value=nullcontext('/native-tool')), \
                patch('subprocess.run',return_value=response) as run:
            self.assertEqual(support.instance_text("password='private-password'",2),'rendered')
        args=run.call_args
        self.assertEqual(args.args[0],['/native-tool','__ci-databases','instance-text'])
        self.assertIn(b'private-password',args.kwargs['input'])
        self.assertNotIn('private-password',str(args.args))
        self.assertNotIn('private-password',str(args.kwargs['env']))
        for changed in [Mock(returncode=1,stdout=response.stdout),
                        Mock(returncode=0,stdout=b'{"ok":false,"error":"private-password"}'),
                        Mock(returncode=0,stdout=b'{"ok":true,"ok":true,"result":{}}'),
                        Mock(returncode=0,stdout=b'invalid')]:
            with patch('rate_limit_tool.tool_binding',return_value={'path':'/native-tool'}), \
                    patch('rate_limit_tool.verified_tool',return_value=nullcontext('/native-tool')), \
                    patch('subprocess.run',return_value=changed),self.assertRaises(RuntimeError) as raised:
                support.instance_text("password='private-password'",2)
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
