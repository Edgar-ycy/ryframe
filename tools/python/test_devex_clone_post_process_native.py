"""真实隔离 Node stub：父控制器崩溃后子请求仍在等待，必须精确回收。"""
import importlib.util
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import time
import unittest
from workspace_directory import WorkspaceDirectory

sys.path.insert(0, str(Path(__file__).resolve().parent))
import devex_clone_post as post
import devex_clone_post_process as process
from devex_clone_capture import read_json, write_json
from devex_clone_run_state import (begin, binding, bind_controller_attempt, finish, initialize_state,
                                  load_state, recover_lock, run_lock)
from full_stack_process import process_identity, terminate_owned_process
from restore_build import file_digest


PARENT = '''
import importlib.util, json, sys, time
from pathlib import Path
from types import SimpleNamespace
fixture = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
sys.path[:0] = fixture["imports"]
from devex_clone_run_state import begin, bind_controller_attempt, run_lock, binding
backend, directory = Path(fixture["backend"]), Path(fixture["directory"])
spec = importlib.util.spec_from_file_location("fixture_producer", backend / "tools/python/devex_clone_post_process.py")
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
with run_lock(directory) as owner:
    kind = fixture["kind"]
    stage = module.PHASES[kind][0]
    mode = {"session":"schedules", "existing":"verify", "quota-plan":"quotas-plan", "quota-apply":"quotas-apply", "quota-reconcile":"quotas-reconcile"}[kind]
    number = begin(directory, stage, mode, {})
    bind_controller_attempt(directory, number, owner)
    output = directory / stage / f"attempt-{number:04d}"; output.mkdir(parents=True)
    for name in ("business-target.json",): (output / name).write_text("{}", encoding="utf-8")
    context = SimpleNamespace(backend=backend, directory_root=directory, output=output, request=fixture["request"],
        request_binding=binding(directory / (stage + ".json")), private={"target_api": fixture["environment"]})
    producer = module.Producer(context, fixture["kind"])
    message = {"operation":"start", "run_dir":str(directory), "attempt":number}
    if fixture["kind"].startswith("quota-"):
        message = {**fixture["quota_grant"], "id":1, "operation":"initialize", "run_dir":str(directory), "attempt":number,
            "artifacts":str(output / "pacing")}
    if fixture["kind"] == "session":
        message.update(id=1, operation="initialize", backend=str(backend), frontend=str(backend),
            config={}, identity={}, artifacts=str(output / "pacing"))
    producer.child.stdin.write((json.dumps(message) + "\\n").encode()); producer.child.stdin.flush()
    time.sleep(120)
'''


class NativeProducerTests(unittest.TestCase):
    def exercise(self, kind):
        root = next(path for path in Path(__file__).resolve().parents if (path / 'Cargo.toml').is_file())
        node = shutil.which('node')
        if node is None:
            self.skipTest('Node.js 不在 CI PATH 中')
        node = Path(node).resolve()
        if not node.is_file():
            self.skipTest('未登记本机 Node stub 工具')
        with WorkspaceDirectory(dir=root / '.local-tests/tmp', prefix='post-node-crash-') as temporary:
            backend = Path(temporary).resolve()
            python, scripts, directory = backend / 'tools/python', backend / 'tools/js', backend / '.local-tests/run'
            python.mkdir(parents=True)
            scripts.mkdir(parents=True)
            directory.mkdir(parents=True)
            (python / 'devex_clone_post_process.py').write_bytes(Path(process.__file__).read_bytes())
            for filename in ('devex_clone_session_bridge.mjs', 'devex_clone_existing.mjs', 'devex_clone_quota_bridge.mjs'):
                (scripts / filename).write_bytes((Path(process.__file__).parent.parent / 'js' / filename).read_bytes())
            marker = backend / 'stub-request-waiting.json'
            stub = "import { writeFileSync } from 'node:fs';\nconst waiting = async () => { writeFileSync(process.env.STUB_MARKER, '{}'); setInterval(() => {}, 1000); await new Promise(() => {}) };\n"
            (scripts / 'request.mjs').write_text(stub + 'export const operationCatalog = async () => new Map();\nexport class Session { constructor(_config, _catalog, identity) { this.identity = identity } async login() { await waiting() } async request() { await waiting() } }', encoding='utf-8')
            (scripts / 'pacing.mjs').write_text('export const createPacing = async () => ({createPreparationControls: async () => ({}),close: async () => {}})', encoding='utf-8')
            (scripts / 'restore_reference_existing.mjs').write_text(stub + 'export const verifyCloneExisting = waiting;', encoding='utf-8')
            write_json(directory / 'manifest.json', {'kind': 'native-stub-only'})
            initialize_state(directory)
            request = {key: None for key in post.FIELDS}
            request.update(format_version=1, kind='devex-clone-post-copy', run_manifest=binding(directory / 'manifest.json'),
                node={'path': str(node), 'sha256': file_digest(node)['sha256']},
                reference_plan={'path': str(backend / 'plan.json')}, dataset={'path': str(backend / 'dataset.json')})
            for filename in ('plan.json', 'dataset.json'):
                write_json(backend / filename, {})
            stage = process.PHASES[kind][0]
            if stage == 'seed-runtime':
                from devex_clone_seed import FIELDS
                request = {**dict.fromkeys(FIELDS), 'format_version': 1, 'kind': 'devex-clone-seed-runtime',
                    'run_manifest': binding(directory / 'manifest.json'), 'node': request['node'],
                    'identity_plan': binding(backend / 'plan.json'), 'identity_state': str(directory / 'seed-runtime/identities')}
            quota_grant = None
            if kind.startswith('quota-'):
                identity = {'subject_id': '1', 'tenant_id': 'system', 'username': 'admin',
                            'password': 'stub-only-password', 'client_address': '198.19.20.1'}
                environment = {'backend_dir': str(backend), 'frontend_dir': str(backend), 'scope_id': 'stub',
                    'api_url': 'http://127.0.0.1:18999/', 'frontend_url': 'http://127.0.0.1:4199/',
                    'system_admin': {key: value for key, value in identity.items() if key != 'subject_id'},
                    'quota': {'system_max_users': 200, 'tenant_max_users': 100},
                    'tenants': [{'tenant_id': 'stub-' + str(index)} for index in range(10)],
                    'pacing': {'contract': {}, 'bindings': {}}, 'passwords': {'system': 'stub-users', 'tenant': 'stub-users'}, 'database': {'control': {'password': 'stub-database'}, 'targets': []}}
                from restore_reference_plan import plan_hash
                for tenant in environment['tenants']:
                    tenant['admin'] = {**environment['system_admin'], 'tenant_id': tenant['tenant_id']}
                private_path = directory / 'private.json'
                write_json(private_path, environment)
                public = json.loads(json.dumps(environment))
                public.pop('passwords')
                public['database']['control'].pop('password')
                public['system_admin'].pop('password')
                for tenant in public['tenants']:
                    tenant['admin'].pop('password')
                plan = {'format_version': 1, 'kind': 'devex-identities', 'configuration': binding(private_path), 'environment': public, 'groups': []}
                plan['plan_sha256'] = plan_hash(plan)
                plan_path = directory / 'plan.json'
                write_json(plan_path, plan)
                request['identity_plan'] = binding(plan_path)
                (scripts / 'config.mjs').write_bytes((Path(process.__file__).parent.parent / 'js/config.mjs').read_bytes())
                (scripts / 'pacing-model.mjs').write_bytes((Path(process.__file__).parent.parent / 'js/pacing-model.mjs').read_bytes())
                quota_grant = {'backend': str(backend), 'frontend': str(backend), 'identity': identity,
                    'tenant_ids': ['system'] + [item['tenant_id'] for item in environment['tenants']],
                    'config': {'contract': {'timeout_ms': 120000, 'pacing': {}}, 'bindings': {
                        'scope_id': 'stub', 'api_url': environment['api_url'], 'frontend_url': environment['frontend_url'], 'pacing': {}}}}
            registration = directory / (stage + '.json')
            write_json(registration, request)
            number = begin(directory, stage, 'register', {})
            finish(directory, number, result={'status': 'seed_runtime_registered' if stage == 'seed-runtime' else 'post_copy_registered',
                                             'registration': binding(registration)})
            fixture = {'backend': str(backend), 'directory': str(directory), 'kind': kind, 'request': request,
                'imports': [str(python), str(root / 'tools/python')],
                'environment': {**os.environ, 'STUB_MARKER': str(marker)}, 'quota_grant': quota_grant}
            write_json(backend / 'fixture.json', fixture)
            parent_file = backend / 'parent.py'
            parent_file.write_text(PARENT, encoding='utf-8')
            log = (backend / 'parent.stderr.log').open('wb')
            parent = subprocess.Popen([sys.executable, str(parent_file), str(backend / 'fixture.json')],
                stdout=subprocess.DEVNULL, stderr=log, creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
            parent_identity, child_identity = process_identity(parent.pid), None
            try:
                deadline = time.monotonic() + 30
                while not marker.exists() and parent.poll() is None and time.monotonic() < deadline:
                    time.sleep(.05)
                self.assertTrue(marker.exists(), '隔离 Node 没有到达无网络 stub 的请求等待点')
                receipt = directory / stage / 'attempt-0002/session-process.json'
                producer_receipt = read_json(receipt)
                child_identity = producer_receipt['identity']
                self.assertEqual(process_identity(child_identity['pid']), child_identity)
                terminate_owned_process(parent_identity, crash=True)
                parent.wait(timeout=10)
                self.assertEqual(process_identity(child_identity['pid']), child_identity, '真实父进程退出后 stub 请求应仍在等待')
                launched_script = scripts / process.KINDS[kind]
                with launched_script.open('a', encoding='utf-8') as stream:
                    stream.write('\n// 原进程启动后工具源码已更新；不改任何旧收据。\n')
                spec = importlib.util.spec_from_file_location('native_fixture_producer', python / 'devex_clone_post_process.py')
                module = importlib.util.module_from_spec(spec)
                spec.loader.exec_module(module)
                with self.assertRaisesRegex(ValueError, '旧 Node 生产者仍在运行'):
                    module.require_quiet(backend, directory)
                recover_lock(backend, directory, binding(directory / 'controller-0002.json'), verify_results=False)
                with run_lock(directory) as owner:
                    number = begin(directory, stage, 'recover-session', {}, verify_results=False)
                    bind_controller_attempt(directory, number, owner, verify_results=False)
                    result = module.recover_session(backend, directory, number, binding(receipt))
                    finish(directory, number, result=result, verify_results=False)
                self.assertFalse(result['already_stopped'])
                self.assertTrue(result['copy_requires_reconciliation'])
                self.assertIsNone(process_identity(child_identity['pid']))
                launched_script.unlink()
                module.require_quiet(backend, directory)
                self.assertEqual(load_state(directory)['attempts'][1]['error_type'], 'ControllerInterrupted')
            finally:
                if parent.poll() is None and parent_identity is not None:
                    terminate_owned_process(parent_identity, crash=True)
                parent.wait(timeout=10)
                if child_identity is not None and process_identity(child_identity['pid']) == child_identity:
                    terminate_owned_process(child_identity, crash=True)
                log.close()

    def test_session_node_survives_parent_crash_and_requires_explicit_recovery(self):
        self.exercise('session')

    def test_existing_reader_node_survives_parent_crash_and_requires_explicit_recovery(self):
        self.exercise('existing')

    def test_quota_apply_node_survives_parent_crash_and_requires_explicit_recovery(self):
        self.exercise('quota-apply')

    def test_both_node_gates_make_no_request_before_release_or_on_wrong_run_and_eof(self):
        root = next(path for path in Path(__file__).resolve().parents if (path / 'Cargo.toml').is_file())
        node = shutil.which('node')
        if node is None:
            self.skipTest('Node.js 不在 CI PATH 中')
        node = Path(node).resolve()
        if not node.is_file():
            self.skipTest('未登记本机 Node stub 工具')
        with WorkspaceDirectory(dir=root / '.local-tests/tmp', prefix='post-node-gates-') as temporary:
            backend = Path(temporary).resolve()
            marker = backend / 'unexpected-request.json'
            stub = "import { writeFileSync } from 'node:fs'; export const verifyCloneExisting = async () => { writeFileSync(process.env.STUB_MARKER, '{}'); throw Error('unexpected request') };"
            scripts = backend / 'tools/js'
            scripts.mkdir(parents=True)
            (scripts / 'restore_reference_existing.mjs').write_text(stub, encoding='utf-8')
            for filename in ('plan.json', 'dataset.json', 'binding.json'):
                write_json(backend / filename, {})
            (backend / 'tools/js/request.mjs').write_text(stub + 'export const operationCatalog = async () => new Map(); export class Session { async login() { await verifyCloneExisting() } }', encoding='utf-8')
            (backend / 'tools/js/pacing.mjs').write_text('export const createPacing = async () => ({createPreparationControls: async () => ({}),close: async () => {}})', encoding='utf-8')
            (scripts / 'department-stage.mjs').write_bytes(
                (Path(process.__file__).parent.parent / 'js/department-stage.mjs').read_bytes())
            for kind, name in process.KINDS.items():
                if kind.startswith("identity-"):
                    continue
                script = backend / 'tools/js' / name
                script.write_bytes((Path(process.__file__).parent.parent / 'js' / name).read_bytes())
                command = [str(node), str(script), '--run-dir', str(backend), '--attempt', '1']
                if kind == 'existing':
                    command += ['--backend-dir', str(backend), '--plan', str(backend / 'plan.json'),
                                '--dataset', str(backend / 'dataset.json'), '--target-binding', str(backend / 'binding.json'), '--write']
                elif kind.startswith('quota-'):
                    command += ['--quota-mode', kind.removeprefix('quota-'), '--identity-plan', str(backend / 'plan.json'),
                                '--identity-plan-sha256', file_digest(backend / 'plan.json')['sha256']]
                elif kind.startswith('department-'):
                    command += ['--department-mode', kind.removeprefix('department-'),
                                '--identity-plan', str(backend / 'plan.json'),
                                '--identity-plan-sha256', file_digest(backend / 'plan.json')['sha256']]
                for message in (b'', (json.dumps({'id': 1, 'operation': 'initialize' if kind == 'session' or kind.startswith('quota-') else 'start',
                                                 'run_dir': str(backend / 'other'), 'attempt': 1, 'backend': str(backend),
                                                 'frontend': str(backend), 'config': {}, 'identity': {}, 'artifacts': str(backend)}) + '\n').encode()):
                    with self.subTest(kind=kind, eof=not message):
                        child = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            env={**os.environ, 'STUB_MARKER': str(marker)},
                            creationflags=subprocess.CREATE_NO_WINDOW if os.name == 'nt' else 0)
                        identity = process_identity(child.pid)
                        try:
                            time.sleep(.1)
                            self.assertIsNone(child.poll())
                            self.assertFalse(marker.exists())
                            stdout, _ = child.communicate(message, timeout=10)
                            self.assertFalse(marker.exists())
                            if (kind == 'session' or kind.startswith('quota-')) and message:
                                self.assertFalse(json.loads(stdout)['ok'])
                        finally:
                            if child.poll() is None and identity is not None:
                                terminate_owned_process(identity, crash=True)
                            child.wait(timeout=10)
                            child.stdin.close()
                            child.stdout.close()
                            child.stderr.close()


if __name__ == '__main__':
    unittest.main()
