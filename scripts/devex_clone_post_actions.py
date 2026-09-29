"""复制后固定调度动作的逐项持久意图与显式核对，不重写已有确认。"""
import json
import re
from pathlib import Path
from queue import Queue, Empty
from threading import Thread

from devex_clone_post_model import (action_input, exact_directory, inspect_disabled,
                                    validate_api_row, validate_before)
from devex_clone_model import exact
from devex_clone_run_state import binding
from devex_clone_capture import read_json, write_json
from devex_clone_source_proof import bound_file
from devex_clone_post_process import Producer, cleanup_failure
from full_stack_process import process_identity


class Bridge(Producer):
    def __init__(self, context, *, kind="session"):
        super().__init__(context, kind)
        self.number, self.responses = 0, Queue()
        Thread(target=self.reader, daemon=True).start()

    def reader(self):
        while True:
            line = self.child.stdout.readline(1024 * 1024 + 1)
            if not line:
                self.responses.put(None)
                return
            if len(line) > 1024 * 1024 or not line.endswith(b'\n'):
                self.responses.put(None)
                return
            self.responses.put(line)

    def call(self, operation, *, timeout=180, **values):
        if process_identity(self.child.pid) != self.identity:
            raise ValueError('会话 bridge 已退出或被替换')
        self.number += 1
        self.child.stdin.write((json.dumps({'id': self.number, 'operation': operation, **values}) + '\n').encode())
        self.child.stdin.flush()
        try:
            line = self.responses.get(timeout=timeout)
        except Empty:
            raise TimeoutError('会话操作结果未知；不重放') from None
        if line is None:
            raise ValueError('会话响应未知；不重放')
        value = json.loads(line)
        if value.get('id') != self.number or value.get('ok') is not True:
            write_json(self.context.output / f'http-failure-{self.number}.json', value)
            raise ValueError('精确会话请求失败；保留诊断，不重放')
        return value['data']

    def login(self, *, tenant_ids=None, post_request=None, template_identities=None):
        request = self.context.request if post_request is None else post_request
        selected = self.context.selected
        config = {'contract': {'timeout_ms': 120000, 'pacing': request['pacing']['contract']},
                  'bindings': {'scope_id': self.context.target_config['scope_id'], 'api_url': selected['api_url'],
                               'frontend_url': selected['frontend_url'], 'pacing': request['pacing']['bindings']}}
        values = {} if tenant_ids is None else {'tenant_ids': tenant_ids}
        if template_identities is not None:
            if tenant_ids is None:
                raise ValueError('模板身份只能随明确租户集合初始化')
            values['template_identities'] = template_identities
        return self.call('initialize', backend=str(self.context.backend), frontend=request['frontend_root'],
                         config=config, identity=request['source_admin'], artifacts=str(self.context.output / 'pacing'),
                         run_dir=str(self.context.directory_root), attempt=int(self.context.output.name.removeprefix('attempt-')), **values)


def validate_authorization(context, auth):
    admin = context.request['source_admin']
    if (any(auth.get(field) != admin[field] for field in ('subject_id', 'tenant_id', 'username'))
            or (auth.get('is_super_admin') is not True and '*' not in auth.get('permissions', [])
                and not {'monitor:schedule:list', 'monitor:schedule:edit'}.issubset(auth.get('permissions', [])))):
        raise ValueError('实际目标会话与明确管理员或权限不匹配')


def action_directories(context, action):
    root = context.base / ('schedule-' + action['schedule_id'])
    exact_directory(root)
    if not root.exists():
        return root, []
    directories = sorted(root.iterdir())
    if any(not path.is_dir() or path.name != f'{index:04d}' for index, path in enumerate(directories, 1)):
        raise ValueError('调度操作证据目录不连续或包含未知文件')
    for path in directories:
        exact_directory(path)
    return root, directories


def intent(context, action, directory):
    value = read_json(directory / 'intent.json')
    if (value.get('action') != action or value.get('registration') != context.request_binding
            or value.get('operation') != 'put_monitor_schedules_by_id_status'
            or value.get('automatic_retry') is not False):
        raise ValueError('调度意图不属于当前登记的动作')
    for name in ('before', 'api_before'):
        path = bound_file(context.backend, value[name])
        if path != directory / ('before.json' if name == 'before' else 'api-before.json'):
            raise ValueError('调度前像证据越界')
    before = read_json(Path(value['before']['path']))
    validate_before(context.backend, action, before)
    validate_api_row(read_json(Path(value['api_before']['path'])), action, True, action['expected_version'])
    return before


def inspect_current(context, bridge, action, directory):
    before = intent(context, action, directory)
    current = context.row(action)
    result = inspect_disabled(context.backend, action, before['row'], current['row'],
                              before['database_now'], current['database_now'])
    api = bridge.call('get', schedule_id=action['schedule_id']) if bridge else None
    if api is not None and result['status'] != 'needs_reconciliation':
        disabled = result['status'] == 'disabled_row_verified'
        validate_api_row(api, action, not disabled, action['expected_version'] + int(disabled))
    return {'inspection': result, 'current': current, 'api': api}


def confirmed(context, action, directory):
    value = read_json(directory / 'confirmed.json')
    exact(value, {'registration', 'action', 'observed', 'api_execution_proven', 'evidence'})
    if value.get('registration') != context.request_binding or value.get('action') != action:
        raise ValueError('调度确认不属于当前动作')
    if type(value['api_execution_proven']) is not bool:
        raise ValueError('API 响应证明必须明确来自成功响应或核对')
    common = {'intent.json', 'before.json', 'api-before.json'}
    if value['api_execution_proven']:
        expected = common | {'api-response.json', 'after.json'}
        evidence_name = 'after.json'
    else:
        remaining = set(value['evidence']) - common
        if len(remaining) != 1 or not re.fullmatch(r'reconcile-[0-9]{4,}\.json', next(iter(remaining))):
            raise ValueError('核对确认必须绑定唯一明确 reconcile 证据')
        evidence_name = next(iter(remaining))
        expected = common | {evidence_name}
    exact(value['evidence'], expected)
    for name, item in value['evidence'].items():
        path = bound_file(context.backend, item)
        if path != directory / name:
            raise ValueError('调度确认引用其他操作证据')
    evidence = read_json(directory / evidence_name)
    if value['api_execution_proven']:
        validate_api_row(read_json(directory / 'api-response.json'), action, False, action['expected_version'] + 1)
    else:
        exact(evidence, {'registration', 'intent', 'observed'})
        if evidence['registration'] != context.request_binding or evidence['intent'] != binding(directory / 'intent.json'):
            raise ValueError('调度核对证据不属于原请求')
        evidence = evidence['observed']
    if evidence != value['observed']:
        raise ValueError('调度确认内嵌行与原始观察不同')
    before = intent(context, action, directory)
    observed = value['observed']
    actual = inspect_disabled(context.backend, action, before['row'], observed['current']['row'],
                             before['database_now'], observed['current']['database_now'])
    if actual != observed['inspection'] or actual['status'] != 'disabled_row_verified':
        raise ValueError('调度确认缺少完整禁用后像')
    validate_api_row(observed['api'], action, False, action['expected_version'] + 1)
    return value


def save_confirmation(context, action, directory, observed, names, *, response_proven):
    if observed['inspection']['status'] != 'disabled_row_verified':
        raise ValueError('完整调度后像不匹配')
    write_json(directory / 'confirmed.json', {'registration': context.request_binding, 'action': action,
        'observed': observed, 'api_execution_proven': response_proven,
        'evidence': {name: binding(directory / name) for name in names}})


def verify_confirmations(context, bridge=None):
    records = []
    for pending in context.copy['plan']['pending_target_actions']:
        action = action_input(pending)
        _, directories = action_directories(context, action)
        if not directories or not (directories[-1] / 'confirmed.json').exists():
            raise ValueError('复制后调度动作尚未全部确认')
        directory = directories[-1]
        previous = confirmed(context, action, directory)
        observed = inspect_current(context, bridge, action, directory)
        if observed['inspection'] != previous['observed']['inspection']:
            raise ValueError('已确认调度的完整行发生变化')
        records.append(binding(directory / 'confirmed.json'))
    return records


def latest_reconciliation(context, action, directory):
    files = sorted(directory.glob('reconcile-*.json'))
    if not files:
        raise ValueError('已有未知写入必须先显式 reconcile，不能重放')
    value = read_json(files[-1])
    if value.get('intent') != binding(directory / 'intent.json') or value.get('registration') != context.request_binding:
        raise ValueError('调度核对属于其他意图')
    before = intent(context, action, directory)
    observed = value['observed']
    inspected = inspect_disabled(context.backend, action, before['row'], observed['current']['row'],
                                 before['database_now'], observed['current']['database_now'])
    if inspected != observed['inspection'] or inspected['status'] != 'unchanged_before':
        raise ValueError('上次核对未得到原前像，不能继续写入')
    validate_api_row(observed['api'], action, True, action['expected_version'])
    return value


def run_action(context, bridge, action, root, directories):
    if directories:
        directory = directories[-1]
        if (directory / 'confirmed.json').exists():
            previous = confirmed(context, action, directory)
            observed = inspect_current(context, bridge, action, directory)
            if observed['inspection'] != previous['observed']['inspection']:
                raise ValueError('已确认调度变化，不能重新执行')
            return
        if not (directory / 'intent.json').exists():
            # 尚未发送请求的部分本地准备也保留，下一序号重新准备。
            if {p.name for p in directory.iterdir()} - {'before.json', 'api-before.json'}:
                raise ValueError('未登记请求的目录包含未知写入证据')
        else:
            latest_reconciliation(context, action, directory)
    root.mkdir(exist_ok=True)
    directory = root / f'{len(directories) + 1:04d}'
    directory.mkdir()
    context.guard()
    before = context.row(action)
    validate_before(context.backend, action, before)
    api = bridge.call('get', schedule_id=action['schedule_id'])
    validate_api_row(api, action, True, action['expected_version'])
    write_json(directory / 'before.json', before)
    write_json(directory / 'api-before.json', api)
    context.guard()
    write_json(directory / 'intent.json', {'registration': context.request_binding, 'action': action,
        'before': binding(directory / 'before.json'), 'api_before': binding(directory / 'api-before.json'),
        'operation': 'put_monitor_schedules_by_id_status', 'automatic_retry': False})
    response = bridge.call('disable', schedule_id=action['schedule_id'], version=action['expected_version'])
    write_json(directory / 'api-response.json', response)
    validate_api_row(response, action, False, action['expected_version'] + 1)
    observed = inspect_current(context, bridge, action, directory)
    write_json(directory / 'after.json', observed)
    context.guard()
    save_confirmation(context, action, directory, observed,
                      ('intent.json', 'before.json', 'api-before.json', 'api-response.json', 'after.json'), response_proven=True)


def execute_actions_body(context, bridge, mode):
    pending = [(action_input(value),) for value in context.copy['plan']['pending_target_actions']]
    actions = [(item[0], *action_directories(context, item[0])) for item in pending]
    # 在任何下一项写入之前，先拒绝所有尚未显式核对的旧意图。
    if mode == 'schedules':
        for action, _, directories in actions:
            if directories and (directories[-1] / 'intent.json').exists() and not (directories[-1] / 'confirmed.json').exists():
                latest_reconciliation(context, action, directories[-1])
    context.guard()
    context.administrator()
    bridge.session_requested = True
    auth = bridge.login()
    validate_authorization(context, auth)
    write_json(context.output / 'authentication.json', auth)
    unresolved = []
    for action, root, directories in actions:
        context.guard()
        if mode == 'schedules':
            run_action(context, bridge, action, root, directories)
        elif directories and (directories[-1] / 'intent.json').exists():
            directory = directories[-1]
            observed = inspect_current(context, bridge, action, directory)
            if (directory / 'confirmed.json').exists():
                if confirmed(context, action, directory)['observed']['inspection'] != observed['inspection']:
                    raise ValueError('已确认调度后像漂移')
            else:
                name = 'reconcile-' + context.output.name.removeprefix('attempt-') + '.json'
                write_json(directory / name, {'registration': context.request_binding,
                    'intent': binding(directory / 'intent.json'), 'observed': observed})
                context.guard()
                if observed['inspection']['status'] == 'disabled_row_verified':
                    save_confirmation(context, action, directory, observed,
                                      ('intent.json', 'before.json', 'api-before.json', name), response_proven=False)
                elif observed['inspection']['status'] == 'needs_reconciliation':
                    unresolved.append(action['schedule_id'])
    if mode == 'schedules':
        records = verify_confirmations(context)
    else:
        records = []
    return {'status': 'needs_reconciliation' if unresolved else ('target_schedule_actions_verified' if mode == 'schedules' else 'target_schedule_actions_reconciled'),
            'confirmed': records, 'unresolved': unresolved, 'registration': context.request_binding,
            'worker_must_remain_stopped': True, 'restore_qualified': False}


def execute_actions(context, bridge, mode):
    bridge.session_requested = False
    try:
        result = execute_actions_body(context, bridge, mode)
    except BaseException as original:
        # 注销和来源检查都必须有界；其失败不能覆盖最初的未知写入错误。
        cleanup = [("session-final-guard", context.guard)]
        if bridge.session_requested:
            cleanup.insert(0, ("session-logout", lambda: bridge.call('close', timeout=10)))
        for operation, action in cleanup:
            try:
                action()
            except Exception as error:
                cleanup_failure(context, operation, error, original)
        raise
    bridge.call('close', timeout=10)
    context.guard()
    return result
