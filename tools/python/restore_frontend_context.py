"""前端 v3 构建上下文的严格结构校验；不读取环境或文件。"""
from __future__ import annotations

import os
from pathlib import Path
import re

TOOLS = ("config/tools.json", ".local-tests/frontend-tools.json")
IDENTITY = ".local-tests/build/source.json"


def exact(value: object, keys: set[str], label: str) -> dict:
    if not isinstance(value, dict) or set(value) != keys:
        raise ValueError(f"{label}字段无效；请使用当前完整字段，删除未知字段")
    return value


def hex_value(value: object, length: int) -> bool:
    return isinstance(value, str) and re.fullmatch(r"[a-f0-9]{%d}" % length, value) is not None


def source_path(value: object) -> bool:
    return (isinstance(value, str) and bool(value)
            and not any(char in value for char in "\0\r\n\\:")
            and all(part not in {"", ".", ".."} for part in value.split("/")))


def path_value(value: object) -> bool:
    return source_path(value) or (
        isinstance(value, str) and Path(value).is_absolute()
        and os.path.normpath(value).replace("\\", "/") == value
        and not any(char in value for char in "\0\r\n\\"))


def descriptors(value: object) -> list[dict]:
    if not isinstance(value, list):
        raise ValueError("前端配置来源必须是有序文件集合")
    names = set()
    for item in value:
        item = exact(item, {"path", "sha256"}, "前端配置来源")
        if not path_value(item["path"]) or not hex_value(item["sha256"], 64):
            raise ValueError("前端配置来源路径或摘要无效")
        if item["path"] in names:
            raise ValueError("前端配置来源重复")
        names.add(item["path"])
    return value


def validate_frontend_build_context(value: object) -> dict:
    value = exact(value, {"command", "mode", "target", "toolchain", "configuration", "identity"},
                  "前端构建上下文")
    toolchain = exact(value["toolchain"], {"node", "pnpm", "vite"}, "前端工具链")
    pnpm = exact(toolchain["pnpm"], {"pinned", "observed"}, "前端 pnpm")
    if (value["command"] != ["vite", "build"] or value["mode"] != "production"
            or value["target"] != "vite-default"
            or any(not isinstance(toolchain[key], str) or not toolchain[key]
                   for key in ("node", "vite"))
            or not isinstance(pnpm["pinned"], str) or not pnpm["pinned"]
            or pnpm["observed"] != pnpm["pinned"]):
        raise ValueError("前端构建命令、模式或工具链无效")
    config = exact(value["configuration"], {"environment", "directory", "files", "tools"},
                   "前端构建配置")
    if (config["environment"] not in ("dev", "test", "prod")
            or config["directory"] != "." and not path_value(config["directory"])):
        raise ValueError("前端 APP_ENV 或配置目录无效；请选择 dev/test/prod 和规范化目录")
    descriptors(config["files"])
    descriptors(config["tools"])
    prefix = "" if config["directory"] == "." else config["directory"].rstrip("/") + "/"
    selected = prefix + f"app.{config['environment']}.toml"
    names = [item["path"] for item in config["files"]]
    if names not in ([selected], [prefix + "app.toml", selected]):
        raise ValueError("前端配置必须按公共、所选环境 TOML 的实际读取顺序绑定")
    names = [item["path"] for item in config["tools"]]
    if names != [name for name in TOOLS if name in names]:
        raise ValueError("前端工具配置来源或优先顺序无效")
    identity = exact(value["identity"], {"frontend_commit", "source_file"}, "前端构建身份")
    commit = identity["frontend_commit"]
    if commit != "development" and not hex_value(commit, 40):
        raise ValueError("前端 frontend_commit 必须为 development 或完整小写 SHA")
    if identity["source_file"] is None:
        if commit != "development":
            raise ValueError("正式前端构建缺少受管身份文件")
    else:
        descriptors([identity["source_file"]])
        if identity["source_file"]["path"] != IDENTITY:
            raise ValueError("前端身份来源不是受管 source.json")
    return value
