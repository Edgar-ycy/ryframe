"""以受管锁和稳定快照合并前端浏览器配置，未知发布结果保留现场。"""

from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import uuid

from devex_clone_model import linked
from restore_runtime_evidence import artifact_snapshot, decode_object

MAX_BYTES = 1024 * 1024


@dataclass(frozen=True)
class ToolConfiguration:
    path: Path
    guard: object | None
    value: dict

    @classmethod
    def capture(cls, frontend: Path) -> "ToolConfiguration":
        path = frontend / ".local-tests/frontend-tools.json"
        for entry in (frontend, path.parent, path):
            if linked(entry):
                raise ValueError("业务浏览器工具文件不得经过链接或重解析点")
        if not path.exists():
            return cls(path, None, {})
        guard = artifact_snapshot(path)
        if guard.bytes > MAX_BYTES:
            raise ValueError("业务浏览器工具文件超过 1 MiB")
        raw = path.read_bytes()
        value = decode_object(raw, "业务浏览器工具配置")
        guard.assert_unchanged()
        return cls(path, guard, value)

    def assert_unchanged(self) -> None:
        if linked(self.path.parent) or linked(self.path):
            raise ValueError("业务浏览器工具配置路径发生漂移")
        if self.guard is None:
            if self.path.exists():
                raise ValueError("业务浏览器工具配置被并发创建；禁止覆盖")
        else:
            try:
                self.guard.assert_unchanged()
            except (OSError, ValueError):
                raise ValueError("业务浏览器工具配置内容发生漂移；禁止覆盖") from None

    def publish(self, configuration: dict) -> "ToolConfiguration":
        """锁定同一写集；完整替换 real_browser，保留其他顶层对象。"""
        self.assert_unchanged()
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.assert_unchanged()
        lock = self.path.with_name(self.path.name + ".lock")
        try:
            lock_stream = lock.open("xb")
        except FileExistsError:
            raise ValueError("业务浏览器工具配置正被写入或有未确认写入；保留现场") from None
        pending = self.path.with_name(f".{self.path.name}.{uuid.uuid4().hex}.pending")
        staging = pending.with_suffix(".publish")
        attempted = False
        try:
            with lock_stream:
                self.assert_unchanged()
                value = {**self.value, "real_browser": configuration}
                content = (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2,
                                      allow_nan=False) + "\n").encode("utf-8")
                if len(content) > MAX_BYTES:
                    raise ValueError("业务浏览器工具文件超过 1 MiB")
                with pending.open("xb") as stream:
                    stream.write(content)
                    stream.flush()
                    os.fsync(stream.fileno())
                self.assert_unchanged()
                # pending 独立保留；替换中断或写后核对失败时不删除证据、不重试。
                attempted = True
                if self.guard is None:
                    os.link(pending, self.path)
                else:
                    os.link(pending, staging)
                    self.assert_unchanged()
                    os.replace(staging, self.path)
                published = ToolConfiguration.capture(self.path.parent.parent)
                if published.value != value or self.path.read_bytes() != content:
                    raise ValueError("业务浏览器工具配置发布结果不确定；保留现场")
                published.assert_unchanged()
                pending.unlink()
            lock.unlink()
            return published
        except BaseException:
            lock_stream.close()
            if not attempted:
                pending.unlink(missing_ok=True)
                lock.unlink(missing_ok=True)
            # 不附加原始异常消息，避免 JSON、文件系统或测试注入异常回显密码。
            raise ValueError("业务浏览器工具配置写入失败；核对漂移与待发布证据，禁止自动重放") from None
