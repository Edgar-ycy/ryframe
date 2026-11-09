"""剩余Python消费者读取原生库存续作确认；不执行初始化或续作。"""
from pathlib import Path

from devex_clone_target_evidence import evidence


def inventory_resume_confirmed(backend: Path, output: Path, initialized: dict) -> bool:
    """成功确认及只读重试谱系全部由受管 Rust 证据协议核对。"""
    try:
        return evidence("inventory-resume-confirmed", backend, output,
                        initialized=initialized)["confirmed"] is True
    except (AttributeError, IndexError, KeyError, OSError, TypeError, ValueError):
        return False
