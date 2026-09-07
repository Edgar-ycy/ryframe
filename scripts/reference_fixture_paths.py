"""参考夹具审阅计划中的隔离运行路径。"""

from __future__ import annotations

from pathlib import Path


def service_run(review: dict) -> Path:
    """为一份审阅计划派生唯一服务账本目录。"""
    try:
        root = Path(review["future_root"])
        backend = Path(review["scopes"]["seed"]["backend_dir"])
    except (KeyError, TypeError) as error:
        raise ValueError("审阅计划缺少 future_root 或 seed 后端路径") from error
    allowed = backend / ".local-tests/reference-fixture"
    if (
        not root.is_absolute()
        or not backend.is_absolute()
        or root.parent != allowed
        or root.name in {"", ".", ".."}
    ):
        raise ValueError("审阅计划 future_root 必须是 Device 参考夹具目录的直接子目录")
    return root / "service-run"
