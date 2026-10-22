"""从前端保存的 APP_ENV 重建公开配置和身份事实，不读取 shell 项目环境。"""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re
import tomllib
import unicodedata

from frontend_origin_tool import normalize_origin
from restore_frontend_context import IDENTITY, TOOLS, exact, hex_value
from restore_frontend_paths import descriptor, failure, json_object, ordinary, read_bytes


def _table(value: object, fields: set[str], label: str, field: str) -> dict:
    if not isinstance(value, dict) or not set(value) <= fields:
        raise failure(label, f"{field} 必须是仅含公开已知字段的 table，禁止凭据和工具私有字段")
    return value


# ECMAScript WhiteSpace 与 LineTerminator；U+0085 不在集合中。
JS_WHITESPACE = (
    "\u0009\u000a\u000b\u000c\u000d\u0020\u00a0\u1680\u2000\u2001\u2002\u2003"
    "\u2004\u2005\u2006\u2007\u2008\u2009\u200a\u2028\u2029\u202f\u205f\u3000\ufeff"
)


def _origin(value: object, label: str, field: str, *, empty: bool) -> str:
    if not isinstance(value, str):
        raise failure(label, f"{field} 必须是字符串 HTTP(S) origin")
    try:
        return normalize_origin(value, allow_empty=empty)
    except ValueError as error:
        raise failure(label, f"{field}：{error}") from None


def _validate(value: object, label: str, *, complete: bool = False) -> dict:
    value = _table(value, {"api_origin", "server"}, label, "root")
    if "api_origin" in value:
        _origin(value["api_origin"], label, "api_origin", empty=True)
    server = _table(value.get("server", {}), {"host", "port", "proxy_target"}, label, "server")
    if "host" in server and (
        not isinstance(server["host"], str)
        or not server["host"].strip(JS_WHITESPACE)
        or any(unicodedata.category(char) == "Cc" for char in server["host"])
    ):
        raise failure(label, "server.host 必须是非空、无控制字符的字符串")
    if "port" in server and (
        type(server["port"]) not in (int, float)
        or not 1 <= server["port"] <= 65535
        or server["port"] != int(server["port"])
    ):
        raise failure(label, "server.port 必须为 1..65535 整数")
    if "proxy_target" in server:
        _origin(server["proxy_target"], label, "server.proxy_target", empty=False)
    if complete and (set(value) != {"api_origin", "server"} or set(server) != {"host", "port", "proxy_target"}):
        raise failure(label, "缺少 api_origin 或 server.host/port/proxy_target 必需字段")
    return value


def _toml(raw: bytes, label: str) -> dict:
    try:
        return tomllib.loads(raw.decode("utf-8", errors="strict"))
    except UnicodeError:
        raise failure(label, "UTF8 无效") from None
    except tomllib.TOMLDecodeError as error:
        line, column = getattr(error, "lineno", None), getattr(error, "colno", None)
        if type(line) is not int or type(column) is not int:
            position = re.search(r"\(at line ([1-9][0-9]*), column ([1-9][0-9]*)\)$", str(error))
            line, column = tuple(map(int, position.groups())) if position else (None, None)
        where = f" line {line} column {column}" if line and column else ""
        raise failure(label, f"TOML 语法错误{where}") from None
    except (ValueError, RecursionError):
        raise failure(label, "TOML 解析失败或嵌套超出解析能力") from None


def _source_name(root: Path, path: Path) -> str:
    try:
        return path.relative_to(root).as_posix() or "."
    except ValueError:
        return path.as_posix()


@dataclass(frozen=True)
class FrontendInputs:
    root: Path
    environment: str
    configuration: dict
    identity: dict
    observed: tuple

    def assert_unchanged(self) -> None:
        if _capture(self.root, self.environment) != self:
            raise ValueError("前端配置、工具、身份字节或所选文件集合发生漂移；请稳定文件后重建")


def _capture(root: Path, environment: str) -> FrontendInputs:
    if environment not in ("dev", "test", "prod"):
        raise failure("APP_ENV", "仅支持精确 dev/test/prod")
    root = ordinary(str(root), directory=True)
    observed, tool_files, files = [], [], []

    def read(file: str, *, optional: bool = False, managed: bool = False) -> bytes | None:
        raw = read_bytes(file, optional=optional, missing_parents=managed)
        observed.append((file, raw))
        return raw

    directory = "config"
    for relative in TOOLS:
        file = str(root) + os.sep + relative
        raw = read(file, optional=True, managed=True)
        if raw is None:
            continue
        document = json_object(raw, relative)
        _table(document, {"application_config_dir"}, relative, "root")
        if "application_config_dir" in document:
            directory = document["application_config_dir"]
            if not isinstance(directory, str) or not directory or any(char in directory for char in "\0\r\n"):
                raise failure(relative, "application_config_dir 必须是有效非空路径")
        tool_files.append(descriptor(root, Path(file), raw))
    raw_directory = directory if os.path.isabs(directory) else str(root) + os.sep + directory
    selected_directory = ordinary(raw_directory, directory=True)
    merged = {}
    for index, name in enumerate(("app.toml", f"app.{environment}.toml")):
        file = str(selected_directory) + os.sep + name
        label = f"{file} (APP_ENV={environment})"
        raw = read(file, optional=index == 0)
        if raw is None:
            continue
        document = _validate(_toml(raw, label), label)
        merged = {**merged, **document, "server": {**merged.get("server", {}), **document.get("server", {})}}
        files.append(descriptor(root, Path(file), raw))
    label = f"{selected_directory}/app.{environment}.toml (APP_ENV={environment})"
    _validate(merged, label, complete=True)
    if _origin(merged["api_origin"], label, "api_origin", empty=True) != "":
        raise failure(label, "真实构建要求 api_origin 为空；请修改所选文件为浏览器同源")
    raw = read(str(root) + os.sep + IDENTITY, optional=True, managed=True)
    identity = {"frontend_commit": "development", "source_file": None}
    if raw is not None:
        value = exact(json_object(raw, IDENTITY), {"format_version", "frontend_commit"}, IDENTITY)
        if (
            type(value["format_version"]) not in (int, float)
            or value["format_version"] != 1
            or value["frontend_commit"] != "development"
            and not hex_value(value["frontend_commit"], 40)
        ):
            raise failure(IDENTITY, "请修正 format_version=1 和 frontend_commit=development 或完整小写 SHA")
        identity = {"frontend_commit": value["frontend_commit"], "source_file": descriptor(root, root / IDENTITY, raw)}
    # 产物身份属于 dist 全量清单，同时独立检查其协议与构建身份。
    dist_identity = str(root) + os.sep + "dist/build-identity.json"
    raw = read(dist_identity)
    value = exact(json_object(raw, "dist/build-identity.json"), {"frontend_commit"}, "产物构建身份")
    if value["frontend_commit"] != identity["frontend_commit"]:
        raise failure("dist/build-identity.json", "frontend_commit 与受管构建身份不一致")
    return FrontendInputs(
        root,
        environment,
        {
            "environment": environment,
            "directory": _source_name(root, selected_directory),
            "files": files,
            "tools": tool_files,
        },
        identity,
        tuple(observed),
    )


def verify_frontend_inputs(root: Path, build: dict, *, expected_head: str | None = None) -> FrontendInputs:
    from restore_frontend_context import validate_frontend_build_context

    validate_frontend_build_context(build)
    inputs = _capture(root, build["configuration"]["environment"])
    if inputs.configuration != build["configuration"] or inputs.identity != build["identity"]:
        raise ValueError("前端配置、工具目录或身份来源/摘要不一致；请使用当前文件重新构建 v3 收据")
    if expected_head is not None and inputs.identity["frontend_commit"] != expected_head:
        raise ValueError("正式前端 frontend_commit 与登记的精确干净源码 HEAD 不一致")
    inputs.assert_unchanged()
    return inputs
