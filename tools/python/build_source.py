"""为原生构建证据的只读调用筛选子进程环境。"""

from __future__ import annotations

import os


def build_process_environment(environment: dict[str, str] | None = None) -> dict[str, str]:
    """保留操作系统/第三方构建环境，移除项目配置和旧构建提交覆盖。"""
    source = os.environ if environment is None else environment
    result = {}
    for name, value in source.items():
        upper = name.upper()
        project = (
            upper.startswith("RYFRAME_")
            or upper.startswith("VITE_")
            or upper.startswith("SNOWFLAKE_")
            or (upper.startswith("APP_") and upper != "APP_ENV")
        )
        if not project:
            result[name] = value
    return result
