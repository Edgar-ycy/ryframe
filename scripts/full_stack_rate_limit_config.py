"""从真实配置来源导出浏览器登录预算，不读取或输出凭据。"""

from __future__ import annotations

import argparse
import json
import os
import re
import tomllib
from pathlib import Path
from typing import Mapping

LOGIN_RULE = "POST /api/v1/auth/login"
DEFAULT_LOGIN_CAPACITY = 5
DEFAULT_API_WINDOW_SECS = 60


def _merge(base: dict, override: dict) -> None:
    for key, value in override.items():
        if isinstance(base.get(key), dict) and isinstance(value, dict):
            _merge(base[key], value)
        else:
            base[key] = value


def _positive_integer(value: object, name: str, maximum: int) -> int:
    if type(value) is not int or not 1 <= value <= maximum:
        raise ValueError(f"{name} 必须是有效的正整数")
    return value


def load_app_table(backend: Path, environment: Mapping[str, str]) -> dict:
    """复用当前隔离环境的配置文件合并；各消费者单独处理其明确的环境覆盖。"""
    app_env = environment.get("APP_ENV")
    if app_env not in ("dev", "test"):
        raise ValueError("真实浏览器登录预算必须显式使用 dev/test 隔离环境")
    directory = environment.get("APP_CONFIG_DIR", "config")
    if not directory.strip():
        raise ValueError("APP_CONFIG_DIR 不能为空")
    config = Path(directory)
    if not config.is_absolute():
        config = backend / config
    table = tomllib.loads((config / "app.toml").read_text(encoding="utf-8"))
    override = config / f"app.{app_env}.toml"
    if override.exists():
        _merge(table, tomllib.loads(override.read_text(encoding="utf-8")))
    return table


def rate_limit_settings(backend: Path, environment: Mapping[str, str]) -> dict:
    table = load_app_table(backend, environment)
    rate_limit = table.get("rate_limit", {})
    if not isinstance(rate_limit, dict):
        raise ValueError("rate_limit 必须是配置表")
    for key, default in (("enabled", True), ("enable_user_rate_limit", False)):
        value = rate_limit.get(key, default)
        variable = "APP_RATE_LIMIT_" + key.upper()
        if variable in environment:
            raw = environment[variable]
            if raw not in ("true", "false"):
                raise ValueError(f"{variable} 必须是 true/false")
            value = raw == "true"
        if type(value) is not bool:
            raise ValueError(f"rate_limit.{key} 必须是布尔值")
        rate_limit[key] = value
    if rate_limit["enabled"] is not True:
        raise ValueError("真实浏览器验收必须保留有效的服务端限流")
    limits = rate_limit.get("api_limits", {})
    if not isinstance(limits, dict):
        raise ValueError("rate_limit.api_limits 必须是配置表")
    for name, value in limits.items():
        if not name.strip():
            raise ValueError("限流规则名不能为空")
        _positive_integer(value, "rate_limit.api_limits", 2**32 - 1)
    rate_limit["api_limits"] = limits
    for key, default, maximum in (("capacity", 100, 2**32 - 1), ("window_secs", 60, 86_400),
                                  ("user_capacity", 500, 2**32 - 1), ("user_window_secs", 60, 86_400),
                                  ("api_window_secs", DEFAULT_API_WINDOW_SECS, 86_400)):
        value = rate_limit.get(key, default)
        variable = "APP_RATE_LIMIT_" + key.upper()
        if variable in environment:
            raw = environment[variable]
            if not re.fullmatch(r"\+?[0-9]+", raw):
                raise ValueError(f"{variable} 必须是正整数字符串")
            value = int(raw)
        rate_limit[key] = _positive_integer(value, f"rate_limit.{key}", maximum)
    return rate_limit


def login_rate_limit(backend: Path, environment: Mapping[str, str]) -> dict[str, int]:
    rate_limit = rate_limit_settings(backend, environment)
    return {
        "capacity": _positive_integer(rate_limit["api_limits"].get(LOGIN_RULE, DEFAULT_LOGIN_CAPACITY), LOGIN_RULE, 10_000),
        "window_secs": rate_limit["api_window_secs"],
    }


def rate_limit_authority(backend: Path, environment: Mapping[str, str]) -> dict:
    settings = rate_limit_settings(backend, environment)
    return {"format_version": 1,
            "global": {"capacity": settings["capacity"], "window_ms": settings["window_secs"] * 1000},
            "user": {"enabled": settings["enable_user_rate_limit"], "capacity": settings["user_capacity"],
                     "window_ms": settings["user_window_secs"] * 1000},
            "api": {"rules": settings["api_limits"], "window_ms": settings["api_window_secs"] * 1000},
            "login": {"capacity": settings["api_limits"].get(LOGIN_RULE, DEFAULT_LOGIN_CAPACITY),
                      "window_ms": settings["api_window_secs"] * 1000}}


def login_budget_environment(backend: Path, environment: Mapping[str, str], state_path: Path) -> dict[str, str]:
    if not state_path.is_absolute() or any(char in str(state_path) for char in "\r\n"):
        raise ValueError("登录预算账本必须使用明确的绝对文件路径")
    limits = login_rate_limit(backend, environment)
    return {
        "RYFRAME_E2E_LOGIN_BUDGET_STATE": str(state_path),
        "RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY": str(limits["capacity"]),
        "RYFRAME_E2E_LOGIN_RATE_LIMIT_WINDOW_SECS": str(limits["window_secs"]),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--environment-file", type=Path)
    output.add_argument("--authority", action="store_true")
    args = parser.parse_args()
    if args.authority:
        print(json.dumps(rate_limit_authority(Path.cwd(), os.environ)))
        return
    if args.environment_file is None:
        print(json.dumps(login_rate_limit(Path.cwd(), os.environ)))
        return
    raw = os.environ.get("RYFRAME_E2E_LOGIN_BUDGET_STATE", "")
    if not raw:
        raise ValueError("必须显式设置 RYFRAME_E2E_LOGIN_BUDGET_STATE")
    values = login_budget_environment(Path.cwd(), os.environ, Path(raw))
    with args.environment_file.open("a", encoding="utf-8", newline="\n") as output:
        output.writelines(f"{key}={value}\n" for key, value in values.items())
    print("已从当前配置导出登录预算容量与窗口")


if __name__ == "__main__":
    main()
