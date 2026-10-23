"""全栈协调器复用既有进程归属、Worker 与来源核验；不决定阶段顺序或重试。"""
from __future__ import annotations

import json
import sys
from pathlib import Path



def configuration_directory(path: Path) -> None:
    _configuration("private-directory", {"path": str(path)})


def instance_text(text: str, worker_id: int) -> str:
    return _configuration("instance-text", {"text": text, "worker_id": worker_id})["text"]


def _configuration(operation: str, request: dict) -> dict:
    """仅传输已有 Rust 文件操作，不复制目录权限或配置改写规则。"""
    import os
    import shutil
    import subprocess
    from process_environment import configured
    from rate_limit_tool import tool_binding, verified_tool
    payload = json.dumps(request, ensure_ascii=False).encode("utf-8")
    if len(payload) > 4 * 1024 * 1024:
        raise ValueError("文件操作请求超过 4 MiB")
    root = Path(__file__).resolve().parents[2]
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    with verified_tool(tool_binding()) as executable:
        command = ([executable] if executable is not None else
                   [os.environ.get("CARGO") or shutil.which("cargo") or "cargo",
                    "run", "--quiet", "--locked", "-p", "xtask", "--"])
        result = subprocess.run([*command, "__ci-databases", operation], cwd=root,
                                env=configured({}), input=payload, stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, timeout=180, check=False, **options)
    if len(result.stdout) > 4 * 1024 * 1024:
        raise RuntimeError("文件操作响应超过 4 MiB；保留现场")
    try:
        value = json.loads(result.stdout, object_pairs_hook=unique)
    except (ValueError, UnicodeError):
        raise RuntimeError("文件操作未确认完成；保留现场，不自动重放") from None
    if (not isinstance(value, dict) or set(value) != {"ok", "result"}
            or value["ok"] is not True or result.returncode != 0):
        raise RuntimeError("文件操作未确认完成；保留现场，不自动重放")
    return value["result"]


def execute(value):
    if not isinstance(value, dict) or set(value) != {'format_version', 'operation', 'backend', 'directory', 'tool', 'arguments'}:
        raise ValueError('全栈底层请求字段无效')
    if type(value['format_version']) is not int or value['format_version'] != 1:
        raise ValueError('全栈底层协议版本无效')
    from rate_limit_tool import install_binding
    install_binding(json.dumps(value['tool']))
    backend, directory = Path(value['backend']), Path(value['directory'])
    if not backend.is_absolute() or not directory.is_absolute():
        raise ValueError('全栈底层请求必须绑定绝对目录')
    operation = value['operation']
    if operation == 'build':
        from ci_full_stack_resources import build_binaries
        import subprocess
        def run(command, **options):
            result = subprocess.run(command, text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, **{k:v for k,v in options.items() if k != 'capture_output'})
            if result.returncode:
                raise ValueError('全栈构建失败；核对 Cargo 阶段与退出状态')
            return result
        return build_binaries(run, backend, directory)
    if operation == 'register':
        from full_stack_runtime import register_runtime
        return register_runtime(backend, directory)
    if operation in {'worker-start', 'worker-stop'}:
        from full_stack_worker import control
        return control(operation.removeprefix('worker-'), backend, directory, timeout=180)
    if operation in {'api-launch', 'api-probe', 'api-stop', 'api-port-free'}:
        return api(operation, backend, directory)
    raise ValueError('未知全栈底层操作')


def api(operation, backend, directory):
    from full_stack_runtime import verify_runtime, process_environment
    from full_stack_process_tree import launch_supervised_process, read_process_tree, terminate_owned_process_tree
    from full_stack_process import read_process, process_identity, assert_identity
    from process_sockets import verify_listener
    if operation == 'api-port-free':
        from full_stack_worker import wait_for_port_free
        from rate_limit_client import load_app_table
        table = load_app_table(backend, {'APP_ENV':'test'}, config_dir=directory / 'config')
        wait_for_port_free(f"http://127.0.0.1:{table['app']['port']}/readyz")
        return {'state':'free'}
    receipt = verify_runtime(backend, directory)
    if operation == 'api-launch':
        from full_stack_worker import ensure_port_free
        from rate_limit_client import load_app_table
        table = load_app_table(backend, {'APP_ENV':'test'}, config_dir=Path(receipt['api_config_dir']))
        url = f"http://127.0.0.1:{table['app']['port']}/readyz"
        ensure_port_free(url)
        if any((directory / name).exists() for name in ('api.json', 'api-tree.json')):
            raise ValueError('已有 API 进程收据，拒绝重复启动')
        with (directory / 'api.log').open('ab') as log:
            process = launch_supervised_process(directory, 'api', receipt['scope_id'],
                [receipt['artifacts']['api']['path'], '--config-dir', receipt['api_config_dir']],
                backend, process_environment(), log)
        return {'pid':process.pid, 'identity':process.tree['process']}
    tree_path = directory / 'api-tree.json'
    process_path = directory / 'api.json'
    if not tree_path.is_file() and not process_path.is_file():
        if operation == 'api-stop':
            if list(directory.glob('api-*.json')):
                raise ValueError('API 启动成员证据不完整，拒绝声称已回收')
            return {'state':'absent'}
        raise ValueError('API 未登记进程树')
    tree = read_process_tree(directory, 'api', receipt['scope_id'])
    if not process_path.is_file() or read_process(directory, 'api', receipt['scope_id']) != tree['process']:
        if operation == 'api-stop':
            terminate_owned_process_tree(tree)
        raise ValueError('API 产品进程与进程树收据不一致，保留现场')
    if operation == 'api-stop':
        terminate_owned_process_tree(tree)
        return {'state':'stopped'}
    if not assert_identity(process_identity(tree['process']['pid']), tree['process']):
        raise ValueError('API 已退出；保留创建身份和进程树收据')
    from rate_limit_client import load_app_table
    table = load_app_table(backend, {'APP_ENV':'test'}, config_dir=Path(receipt['api_config_dir']))
    verify_listener(tree['process']['pid'], f"http://127.0.0.1:{table['app']['port']}/readyz")
    return {'state':'running'}


def unique(pairs):
    result = {}
    for key, value in pairs:
        if key in result: raise ValueError('重复字段')
        result[key] = value
    return result


def main():
    if len(sys.argv) != 1:
        print('protocol_error: only bounded stdin is accepted', file=sys.stderr)
        return 2
    try:
        data = sys.stdin.buffer.read(32769)
        if len(data) > 32768: raise ValueError('全栈底层请求超过限额')
        result = execute(json.loads(data, object_pairs_hook=unique))
        print(json.dumps({'ok':True,'result':result}, ensure_ascii=False))
        return 0
    except BaseException as error:
        # 既有核验器可能带回含凭据的配置错误；原始异常不进入公开响应。
        print(json.dumps({'ok':False,'error':'full_stack_support_failed; preserve receipts; no automatic replay', 'category':type(error).__name__}))
        return 1


if __name__ == '__main__':
    raise SystemExit(main())
