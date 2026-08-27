#!/usr/bin/env python3
"""校验后端检查脚本使用的 Python 与固定原生依赖。"""

from __future__ import annotations

import ctypes
import importlib.metadata
import re
import subprocess
import sys
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
if str(SCRIPT_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPT_DIR))
REQUIREMENTS = SCRIPT_DIR / "requirements-ci.txt"
REQUIREMENT_PATTERN = re.compile(r"^([A-Za-z0-9_.-]+)==([^\s\\]+)")
MINIMUM_PYTHON = (3, 11)
NATIVE_PROBE_ARGUMENT = "--native-probe"


def locked_requirements(path: Path, errors: list[str]) -> dict[str, str]:
    try:
        source = path.read_text(encoding="utf-8")
    except OSError as error:
        errors.append(f"无法读取固定 Python 依赖 {path}: {error}")
        return {}
    requirements = {
        match.group(1): match.group(2)
        for line in source.splitlines()
        if (match := REQUIREMENT_PATTERN.match(line.strip())) is not None
    }
    if not requirements:
        errors.append(f"固定 Python 依赖为空：{path}")
    return requirements


def validate_distributions(requirements: dict[str, str], errors: list[str]) -> None:
    for distribution, expected in sorted(requirements.items()):
        try:
            actual = importlib.metadata.version(distribution)
        except importlib.metadata.PackageNotFoundError:
            errors.append(f"缺少固定 Python 依赖：{distribution}=={expected}")
            continue
        if actual != expected:
            errors.append(
                f"Python 依赖版本不匹配：{distribution} 必须为 {expected}，实际为 {actual}"
            )


def suppress_windows_native_error_dialog() -> None:
    if sys.platform != "win32":
        return
    # 原生扩展一旦发生访问冲突，应让父进程收到退出码，不能弹出阻塞 CI 的系统对话框。
    sem_failcriticalerrors = 0x0001
    sem_nogpfaulterrorbox = 0x0002
    ctypes.windll.kernel32.SetErrorMode(  # type: ignore[attr-defined]
        sem_failcriticalerrors | sem_nogpfaulterrorbox
    )


def native_probe() -> int:
    suppress_windows_native_error_dialog()
    errors: list[str] = []
    try:
        from rust_function_size import load_ast_runtime
    except ImportError as error:
        print(f"无法加载 Rust AST 门禁：{error}", file=sys.stderr)
        return 1
    runtime = load_ast_runtime(errors)
    if runtime is None:
        for error in errors:
            print(error, file=sys.stderr)
        return 1

    source = b"pub fn probe() { let value = 1; }\n"
    for iteration in range(64):
        records = runtime.parse_records("native-probe.rs", source, errors)
        if len(records) != 1 or records[0].symbol != "probe":
            errors.append(f"Rust AST 原生探针第 {iteration + 1} 轮返回异常结果")
            break
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    return 0


def validate_environment() -> list[str]:
    errors: list[str] = []
    if sys.version_info < MINIMUM_PYTHON:
        errors.append(
            "Python 版本过低："
            f"至少需要 {MINIMUM_PYTHON[0]}.{MINIMUM_PYTHON[1]}，"
            f"实际为 {sys.version_info.major}.{sys.version_info.minor}"
        )
    requirements = locked_requirements(REQUIREMENTS, errors)
    validate_distributions(requirements, errors)
    return errors


def run_native_probe() -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(Path(__file__).resolve()), NATIVE_PROBE_ARGUMENT],
        cwd=SCRIPT_DIR.parent,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )


def main() -> int:
    if sys.argv[1:] == [NATIVE_PROBE_ARGUMENT]:
        return native_probe()
    if sys.argv[1:]:
        print("用法：python scripts/check_python_environment.py", file=sys.stderr)
        return 2

    errors = validate_environment()
    if not errors:
        completed = run_native_probe()
        if completed.returncode != 0:
            detail = (completed.stderr or completed.stdout).strip()
            errors.append(
                "tree-sitter Rust 原生探针失败"
                f"（exit={completed.returncode}）"
                + (f"：{detail}" if detail else "")
            )
    if errors:
        print("Python environment check failed:", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        print(
            "请在隔离虚拟环境中按 scripts/requirements-ci.txt 安装依赖；"
            "cargo xtask 可通过 RYFRAME_PYTHON 指定该环境的解释器。",
            file=sys.stderr,
        )
        return 1

    print(
        "Python environment check passed "
        f"(executable={sys.executable}, version={sys.version_info.major}."
        f"{sys.version_info.minor}.{sys.version_info.micro})."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
