"""只读核对来源运行配置与参考计划的物理目标；秘密仅在内存比较。"""
from __future__ import annotations

import configparser
import hashlib
import json
import os
import re
from pathlib import Path
from typing import Mapping
from urllib.parse import urlsplit

from full_stack_rate_limit_config import load_app_table


class BindingError(ValueError):
    """错误只包含固定诊断，不包含配置值或秘密。"""


DATABASE_FIELDS = {"host": "HOST", "port": "PORT", "database": "NAME", "username": "USERNAME",
                   "password": "PASSWORD", "tls_mode": "TLS_MODE", "tls_ca": "TLS_CA",
                   "tls_client_cert": "TLS_CLIENT_CERT", "tls_client_key": "TLS_CLIENT_KEY"}
STORAGE_FIELDS = ("backend", "endpoint", "region", "access_key", "secret_key", "use_ssl")
TLS_FILES = ("tls_ca", "tls_client_cert", "tls_client_key")


def demand(condition: bool, code: str) -> None:
    if not condition:
        raise BindingError(code)


def text(value: object) -> str:
    demand(isinstance(value, str) and bool(value) and value == value.strip(), "来源配置缺少明确文本值")
    return value


def port(value: object) -> int:
    demand(type(value) is int and 1 <= value <= 65535, "来源端口必须明确且有效")
    return value


def override(table: dict, variables: Mapping[str, str], fields: dict, prefix: str) -> dict:
    result = dict(table)
    for field, suffix in fields.items():
        name = prefix + suffix
        demand(name + "_FILE" not in variables, "来源文件间接覆盖未被运行收据绑定")
        if name not in variables:
            continue
        value = variables[name]
        if field == "port":
            demand(re.fullmatch(r"\+?[0-9]+", value) is not None, "来源端口覆盖无效")
            value = int(value)
        elif field == "use_ssl":
            demand(value in ("true", "false"), "来源对象 TLS 开关覆盖无效")
            value = value == "true"
        result[field] = value
    return result


def collection(table: dict, field: str, variable: str, variables: Mapping[str, str]) -> list:
    demand(variable + "_FILE" not in variables, "来源目标文件内容未被运行收据绑定")
    result = json.loads(variables[variable]) if variable in variables else table.get(field, [])
    demand(isinstance(result, list), "来源目标必须是明确数组")
    return result


def defaults_connection(backend: Path, declared: dict) -> dict:
    path = Path(declared["defaults_file"])
    demand(path.is_absolute() and path.resolve().is_relative_to((backend / ".local-tests").resolve())
           and path.is_file() and not path.is_symlink(), "来源 MySQL 凭据文件边界无效")
    raw = path.read_bytes()
    demand(hashlib.sha256(raw).hexdigest() == declared["defaults_sha256"], "来源 MySQL 凭据文件摘要已变化")
    content = raw.decode("utf-8")
    config = configparser.ConfigParser(interpolation=None)
    config.read_string(content)
    required = {"host", "port", "user", "password", "ssl-mode"}
    demand(config.sections() == ["client"] and not config.defaults()
           and set(config["client"]) == required, "来源凭据必须显式且仅包含连接认证与无外部文件 TLS 模式")
    # 仅接受本参考设施的无转义写法，拒绝 MySQL 与 ConfigParser 解释可能不同的选项值。
    for line in content.splitlines():
        if not line.strip() or line == "[client]":
            continue
        demand("=" in line, "来源 MySQL 凭据选项格式不明确")
        key, value = line.split("=", 1)
        demand(key in required and bool(value) and value == value.strip()
               and not any(character in value for character in "\"'\\#;\r\n"), "来源 MySQL 凭据选项包含不支持的转义或注释")
    client = config["client"]
    demand(client["host"] in ("127.0.0.1", "::1"), "来源 MySQL 必须是计划中的明确本机地址")
    demand(client["ssl-mode"] in ("REQUIRED", "DISABLED"), "来源 TLS 模式不支持未绑定的外部证书")
    demand(re.fullmatch(r"[0-9]+", client["port"]) is not None, "来源 MySQL 凭据端口无效")
    return {"host": client["host"], "port": port(int(client["port"])), "database": text(declared["database"]),
            "username": client["user"], "password": client["password"], "tls_mode": client["ssl-mode"].lower()}


def runtime_targets(table: dict, variables: Mapping[str, str]) -> dict:
    values = collection(table.get("tenant_data", {}), "targets", "APP_TENANT_DATA_TARGETS", variables)
    targets = {}
    for value in values:
        demand(isinstance(value, dict), "来源租户目标无效")
        key = text(value.get("key"))
        demand(key not in targets, "来源租户目标重复")
        targets[key] = value
    # 当前配置规范化明确注入该保留目标；不推断其他未声明的连接参数。
    if "shared-control" not in targets:
        targets["shared-control"] = {"key": "shared-control", "kind": "control", "mode": "shared"}
    control = targets["shared-control"]
    demand(control.get("kind") == "control" and control.get("mode") == "shared"
           and all(control.get(field) is None for field in (*DATABASE_FIELDS, "password_env", "max_connections")),
           "来源控制目标必须复用明确的 primary 连接")
    return targets


def database_bindings(backend: Path, source: dict, table: dict, variables: Mapping[str, str]) -> list:
    database = table["database"]
    for field in ("replicas", "sources"):
        demand(not collection(database, field, "APP_DATABASE_" + field.upper(), variables),
               "来源参考计划不支持未登记的副本或命名数据源")
    primary = override(database["primary"], variables, DATABASE_FIELDS, "APP_DATABASE_")
    targets = runtime_targets(table, variables)
    declared = source["databases"]
    demand(len(declared) == len(targets) and {item["key"] for item in declared} == set(targets),
           "来源配置与计划必须完整且精确覆盖相同数据库目标")
    result, physical = [], set()
    for item in sorted(declared, key=lambda value: value["key"]):
        target = targets[item["key"]]
        control = item["key"] == "shared-control"
        demand(item["kind"] == ("combined" if control else "tenant")
               and target.get("kind") == ("control" if control else "mysql")
               and item["mode"] == target.get("mode") and item["mode"] in ("shared", "dedicated"),
               "来源数据库逻辑类型或共享模式不同")
        connection = dict(primary if control else target)
        if not control:
            name = text(connection.get("password_env"))
            demand(re.fullmatch(r"APP_[A-Z0-9_]+", name) is not None and not name.endswith("_FILE"),
                   "来源租户密码必须使用运行收据绑定的 APP 环境变量")
            connection["password"] = text(variables.get(name))
        demand(all(connection.get(field) in (None, "") for field in TLS_FILES), "来源 TLS 文件内容未被运行收据绑定")
        expected = defaults_connection(backend, item)
        actual = {field: (port(connection.get(field)) if field == "port" else text(connection.get(field)))
                  for field in expected}
        demand(actual == expected, "来源数据库地址数据库名认证或 TLS 与计划不同")
        identity = (actual["host"], actual["port"], actual["database"].lower())
        demand(identity not in physical, "来源不同逻辑目标引用同一物理数据库")
        physical.add(identity)
        result.append({"key": item["key"], "kind": item["kind"], "mode": item["mode"],
                       "server_uuid": item["server_uuid"], **{field: actual[field] for field in ("host", "port", "database", "tls_mode")}})
    return result


def storage_endpoint(value: object) -> str:
    raw = text(value)
    parsed = urlsplit(raw)
    demand(parsed.scheme in ("http", "https") and parsed.hostname in ("127.0.0.1", "::1")
           and parsed.port is not None and not parsed.username and not parsed.password
           and parsed.path in ("", "/") and not parsed.query and not parsed.fragment,
           "来源对象端点必须是明确协议端口的本机地址")
    return raw.rstrip("/")


def storage_binding(source: dict, table: dict, variables: Mapping[str, str]) -> dict:
    actual = override(table["object_storage"], variables, {field: field.upper() for field in STORAGE_FIELDS}, "APP_OBJECT_STORAGE_")
    demand(actual.get("backend") in ("rustfs", "minio", "s3"), "来源必须使用参考计划的 S3 兼容后端")
    endpoint = storage_endpoint(actual.get("endpoint"))
    expected = source["s3"]
    demand(type(actual.get("use_ssl")) is bool and actual["use_ssl"] == (urlsplit(endpoint).scheme == "https"),
           "来源显式对象端点协议与 use_ssl 必须一致")
    demand(endpoint == storage_endpoint(expected["endpoint"]) and text(actual.get("region")) == text(expected["region"]),
           "来源对象端点或区域与计划不同")
    for field in ("access_key", "secret_key"):
        demand(text(actual.get(field)) == text(variables.get(expected[field + "_env"])), "来源对象认证与计划不同")
    return {"backend": actual["backend"], "endpoint": endpoint, "region": actual["region"]}


def source_binding(backend: Path, plan: dict, variables: Mapping[str, str] | None = None) -> dict:
    """返回可公开的物理描述；调用方另核验 runtime 摘要、进程及实际 ownership。"""
    variables = os.environ if variables is None else variables
    try:
        source = plan["source"]
        demand(variables.get("APP_ENV") == "test" and variables.get("APP_SCOPE_ID") == source["scope_id"],
               "来源环境与计划 scope 必须明确一致")
        table = load_app_table(backend, variables)
        result = {"format_version": 1, "scope_id": source["scope_id"],
                  "databases": database_bindings(backend, source, table, variables),
                  "s3": storage_binding(source, table, variables)}
        digest = hashlib.sha256(json.dumps(result, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
        return {**result, "sha256": digest}
    except BindingError:
        raise
    except Exception:
        # TOML、JSON、INI 解析异常可能包含原配置行，不能传播到失败报告。
        raise BindingError("来源配置不完整或无法安全解析") from None
