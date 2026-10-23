"""浏览器原生控制器调用现有运行时与全栈进程引擎的窄适配器。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess
import sys


def execute(value: dict):
    import reference_fixture_runtime as runtime
    from reference_fixture_service_context import application_settings
    from rate_limit_client import rate_limit_settings
    from restore_frontend_build import validate_frontend_build
    from full_stack_process_tree import read_process_tree, terminate_owned_process_tree
    from full_stack_process_monitor import completion_binding, wait_members

    operation, args = value["operation"], value["arguments"]
    if operation == "bootstrap":
        bootstrap, execution, private = runtime._bootstrap(Path(args["backend"]), Path(args["environment"]))
        return {"bootstrap": str(bootstrap), "execution": str(execution), "private": private,
                "python": str(Path(sys.executable).resolve())}
    if operation == "tools":
        from process_environment import configured
        environment = configured({})
        corepack = shutil.which("corepack", path=environment.get("PATH"))
        node = shutil.which("node", path=environment.get("PATH"))
        if corepack is None or (os.name == "nt" and node is None):
            raise ValueError("缺少Corepack或Node")
        return {"corepack": str(Path(corepack).resolve()),
                "node": str(Path(node).resolve()) if node is not None else None,
                "corepack_home": str(Path(os.environ.get("COREPACK_HOME", str(Path(args["frontend"]) / ".local-tests/corepack"))).resolve())}
    if operation in {"verify", "control", "observe", "worker"}:
        common = (Path(args["backend"]), Path(args["environment"]), Path(args["output"]))
        functions = {"verify": runtime.verify, "control": runtime.run,
                     "observe": runtime.observe_runtime, "worker": runtime.worker_only}
        return functions[operation](*common, *([] if operation == "verify" else [args["action"]]))
    if operation in {"settings", "limits"}:
        bootstrap, execution = Path(args["bootstrap"]), Path(args["execution"])
        if operation == "settings":
            return application_settings(bootstrap, execution)
        return rate_limit_settings(execution, {"APP_ENV": args["app_env"]}, config_dir=bootstrap.parent / "config")
    if operation == "frontend-build":
        _, receipt = validate_frontend_build(Path(args["frontend"]))
        return {"path": str(receipt.path)}
    if operation in {"completion", "terminate"}:
        tree = read_process_tree(Path(args["directory"]), "frontend", args["scope_id"])
        if operation == "terminate":
            terminate_owned_process_tree(tree, crash=True)
        completion = completion_binding(tree)
        return {"tree": tree, "completion": completion, "proof": wait_members(tree, timeout=0)}
    raise ValueError("浏览器依赖接口操作无效")


def launch(value: dict) -> int:
    from full_stack_process_tree import launch_supervised_process
    process = launch_supervised_process(Path(value["directory"]), "frontend", value["scope_id"],
                                        value["command"], Path(value["frontend"]),
                                        value["environment"], subprocess.PIPE)
    if process.supervisor.stdout is None:
        raise ValueError("全栈引擎未提供输出管道")
    shutil.copyfileobj(process.supervisor.stdout, sys.stdout.buffer, 65536)
    sys.stdout.buffer.flush()
    return process.wait(timeout=10)


if __name__ == "__main__":
    try:
        raw = sys.stdin.buffer.read(16 * 1024 * 1024 + 1)
        if len(raw) > 16 * 1024 * 1024:
            raise ValueError("浏览器依赖协议超过上限")
        value = json.loads(raw)
        if "native_tool" in value:
            from rate_limit_tool import install_binding
            install_binding(json.dumps(value["native_tool"], ensure_ascii=False))
        if sys.argv[1:] == ["--launch"]:
            raise SystemExit(launch(value))
        if sys.argv[1:]:
            raise ValueError("浏览器依赖只接受标准输入")
        result = execute(value)
        print(json.dumps({"ok": True, "value": result}, ensure_ascii=False))
    except Exception:
        print('{"ok":false,"error":"浏览器依赖调用失败；保留原始收据，不自动重放"}', file=sys.stderr)
        raise SystemExit(1)
