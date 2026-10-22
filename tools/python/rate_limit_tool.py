"""读取协调器显式绑定的限流工具，不缓存配置、不通过 PATH 猜测二进制。"""

from __future__ import annotations

from contextlib import contextmanager
import json
from pathlib import Path
import re
import stat

from artifact_digests import file_digest, protect_binary_invocation


# 协调器仅显式安装不可变描述；每次读取重新解析，避免调用方修改共享对象。
_COORDINATOR_BINDING: str | None = None
FIELDS = frozenset({"format_version", "kind", "path", "bytes", "sha256"})
MAX_BINDING_BYTES = 16 * 1024


def parse_binding(raw: str) -> dict:
    if not isinstance(raw, str) or not raw or len(raw.encode("utf-8")) > MAX_BINDING_BYTES:
        raise ValueError("限流工具绑定为空或超过 16 KiB")
    try:
        value = json.loads(raw, object_pairs_hook=_unique_object)
    except (ValueError, TypeError):
        raise ValueError("限流工具绑定不是唯一字段的 JSON 对象") from None
    _validate_binding(value)
    return value


def install_binding(raw: str) -> None:
    global _COORDINATOR_BINDING
    parse_binding(raw)
    if _COORDINATOR_BINDING is not None:
        raise ValueError("限流工具绑定已经安装，不能重复安装")
    _COORDINATOR_BINDING = raw


def tool_binding() -> dict | None:
    raw = _COORDINATOR_BINDING
    return None if raw is None else parse_binding(raw)


def _validate_binding(value: object) -> None:
    if (
        not isinstance(value, dict)
        or set(value) != FIELDS
        or type(value["format_version"]) is not int
        or value["format_version"] != 1
        or value["kind"] != "ryframe-native-rate-limit-tool"
        or not isinstance(value["path"], str)
        or "\0" in value["path"]
        or not Path(value["path"]).is_absolute()
        or type(value["bytes"]) is not int
        or value["bytes"] <= 0
        or not isinstance(value["sha256"], str)
        or not re.fullmatch(r"[a-f0-9]{64}", value["sha256"])
    ):
        raise ValueError("限流工具绑定的结构、版本或摘要无效")


def _unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("限流工具绑定不能重复字段")
        value[key] = item
    return value


def _reject_links(path: Path) -> None:
    if any(
        item.is_symlink()
        or getattr(item.lstat(), "st_file_attributes", 0) & 0x400
        for item in (path, *path.parents)
    ):
        raise ValueError("限流工具绑定不能引用链接或重解析点")
    if not stat.S_ISREG(path.lstat().st_mode):
        raise ValueError("限流工具绑定必须引用普通文件")


@contextmanager
def verified_tool(value: dict | None):
    if value is None:
        yield None
        return
    _validate_binding(value)
    path = Path(value["path"])
    _reject_links(path)
    expected = {key: value[key] for key in ("bytes", "sha256")}
    with protect_binary_invocation({key: value[key] for key in ("path", "sha256")}):
        before = path.stat()
        if file_digest(path) != expected:
            raise ValueError("限流工具字节或摘要与当前协调器绑定不同")
        primary = None
        try:
            yield str(path)
        except BaseException as error:
            primary = error
            raise
        finally:
            try:
                _reject_links(path)
                after = path.stat()
                observed = file_digest(path)
                if (
                    (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino)
                    or observed != expected
                ):
                    raise ValueError("限流工具在执行期间发生身份或内容变化")
            except BaseException as error:
                if primary is None:
                    raise
                raise BaseExceptionGroup(
                    "限流工具执行与收尾校验同时失败", [primary, error]
                ) from None
