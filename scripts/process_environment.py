"""构造受限子进程环境，并串行切换已绑定的明确环境。"""

from __future__ import annotations

from contextlib import contextmanager
import copy
import hashlib
import json
import os
import threading

_ENVIRONMENT_LOCK = threading.RLock()
_SYSTEM_NAMES = {
    "APPDATA",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "COMMONPROGRAMW6432",
    "COMSPEC",
    "HOME",
    "HOMEDRIVE",
    "HOMEPATH",
    "LANG",
    "LOCALAPPDATA",
    "NUMBER_OF_PROCESSORS",
    "OS",
    "PATH",
    "PATHEXT",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "PROGRAMW6432",
    "SHELL",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "TMPDIR",
    "USER",
    "USERPROFILE",
    "WINDIR",
}


def _binding(value: dict[str, str]) -> str:
    encoded = json.dumps(
        value, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def configured(private: dict, inherited: dict | None = None) -> dict:
    """将私有配置叠加到安全基础环境，不继承业务配置、凭据或代理。"""
    inherited = os.environ if inherited is None else inherited
    for values in (private, inherited):
        if (not isinstance(values, dict) and values is not os.environ) or any(
            not isinstance(key, str) or not isinstance(value, str)
            for key, value in values.items()
        ):
            raise ValueError("进程环境必须是文本映射")
    folded = [key.upper() for key in private]
    if len(folded) != len(set(folded)):
        raise ValueError("私有环境包含大小写重复字段")
    private_names = set(folded)
    safe = {}
    for key, value in inherited.items():
        upper = key.upper()
        if upper in private_names:
            continue
        if upper in _SYSTEM_NAMES or upper.startswith("PROCESSOR_") or upper.startswith("LC_"):
            safe[key] = value
    safe.update(private)
    return safe


class Environments:
    """在创建线程串行切换两侧环境，并拒绝登记后的内容漂移。"""

    def __init__(self, source: dict, target: dict):
        self.creator = threading.get_ident()
        self.values = {
            "source": copy.deepcopy(dict(source)),
            "target": copy.deepcopy(dict(target)),
        }
        if any(
            not isinstance(key, str) or not isinstance(value, str)
            for environment in self.values.values()
            for key, value in environment.items()
        ):
            raise ValueError("两侧进程环境必须是明确的文本映射")
        self.bindings = {side: _binding(value) for side, value in self.values.items()}

    @contextmanager
    def use(self, side: str):
        if threading.get_ident() != self.creator or side not in self.values:
            raise ValueError("同一环境代次只允许创建线程串行使用明确侧环境")
        if _binding(self.values[side]) != self.bindings[side]:
            raise ValueError("进程侧环境在登记后变化")
        with _ENVIRONMENT_LOCK:
            original = dict(os.environ)
            installed = False
            try:
                os.environ.clear()
                os.environ.update(self.values[side])
                installed = True
                yield self.values[side]
            finally:
                changed = installed and (
                    dict(os.environ) != self.values[side]
                    or _binding(self.values[side]) != self.bindings[side]
                )
                os.environ.clear()
                os.environ.update(original)
                if changed:
                    raise ValueError("进程命令期间环境发生漂移，已恢复调用方环境")
