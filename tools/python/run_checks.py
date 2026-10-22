"""通过显式参数安装协调器工具绑定，再原样执行 unittest。"""

from __future__ import annotations

import runpy
import sys

from rate_limit_tool import install_binding


_INVALID_ARGUMENTS = "检查入口参数或限流工具绑定无效"


def parse_arguments(arguments: list[str]) -> tuple[str | None, list[str]]:
    if arguments and arguments[0] == "--rate-limit-tool-binding":
        if len(arguments) < 3 or arguments[2] != "--":
            raise ValueError(_INVALID_ARGUMENTS)
        binding, remaining = arguments[1], arguments[3:]
    elif arguments and arguments[0] == "--":
        binding, remaining = None, arguments[1:]
    else:
        raise ValueError(_INVALID_ARGUMENTS)
    if remaining[:2] != ["-m", "unittest"]:
        raise ValueError(_INVALID_ARGUMENTS)
    return binding, remaining[1:]


def execute(arguments: list[str], *, runner=runpy.run_module) -> None:
    try:
        binding, unittest_arguments = parse_arguments(arguments)
        if binding is not None:
            install_binding(binding)
    except (ValueError, TypeError):
        print(_INVALID_ARGUMENTS, file=sys.stderr)
        raise SystemExit(2) from None
    sys.argv = unittest_arguments
    runner("unittest", run_name="__main__", alter_sys=True)


if __name__ == "__main__":
    execute(sys.argv[1:])
