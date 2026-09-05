"""为 Windows 测试创建继承工作区权限的临时目录。"""

import shutil
import uuid
from pathlib import Path


class WorkspaceDirectory:
    """避免 Python 3.13 在 Windows 上为 0700 临时目录创建隔离 ACL。"""

    def __init__(self, parent: Path, prefix: str = "tmp-") -> None:
        parent.mkdir(parents=True, exist_ok=True)
        self.path = (parent / f"{prefix}{uuid.uuid4().hex}").resolve()
        self.path.mkdir()
        self.name = str(self.path)

    def cleanup(self) -> None:
        if self.path.exists():
            shutil.rmtree(self.path)

    def __enter__(self) -> str:
        return self.name

    def __exit__(self, _exc_type, _exc_value, _traceback) -> None:
        self.cleanup()
