"""浏览器原生控制器调用现有运行时与全栈进程引擎的窄适配器。"""
from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import sys


def execute(value: dict):
    import reference_fixture_runtime as runtime
    from reference_fixture_service_context import application_settings
    from rate_limit_client import rate_limit_settings
    from restore_frontend_build import validate_frontend_build

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
    if operation in {"settings", "limits"}:
        bootstrap, execution = Path(args["bootstrap"]), Path(args["execution"])
        if operation == "settings":
            return application_settings(bootstrap, execution)
        return rate_limit_settings(execution, {"APP_ENV": args["app_env"]}, config_dir=bootstrap.parent / "config")
    if operation == "frontend-build":
        _, receipt = validate_frontend_build(Path(args["frontend"]))
        return {"path": str(receipt.path)}
    raise ValueError("浏览器依赖接口操作无效")


if __name__ == "__main__":
    try:
        raw = sys.stdin.buffer.read(16 * 1024 * 1024 + 1)
        if len(raw) > 16 * 1024 * 1024:
            raise ValueError("浏览器依赖协议超过上限")
        value = json.loads(raw)
        if "native_tool" in value:
            from rate_limit_tool import install_binding
            install_binding(json.dumps(value["native_tool"], ensure_ascii=False))
        if sys.argv[1:]:
            raise ValueError("浏览器依赖只接受标准输入")
        result = execute(value)
        print(json.dumps({"ok": True, "value": result}, ensure_ascii=False))
    except Exception:
        print('{"ok":false,"error":"浏览器依赖调用失败；保留原始收据，不自动重放"}', file=sys.stderr)
        raise SystemExit(1)
