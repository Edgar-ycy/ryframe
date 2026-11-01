"""读取并校验全栈二进制清单中的角色与文件路径。"""

from __future__ import annotations

import json
from pathlib import Path

BINARIES = (("bin-reset", "ryframe-reset"), ("bin-migrate", "ryframe-migrate"),
            ("bin-api", "ryframe"), ("bin-worker", "ryframe-worker"))


def read_binaries(output_dir: Path) -> dict[str, str]:
    binaries = json.loads((output_dir / "binaries.json").read_text(encoding="utf-8"))
    if set(binaries) != {name for _, name in BINARIES}:
        raise ValueError("全栈二进制清单不完整")
    for value in binaries.values():
        if not Path(value).is_absolute() or not Path(value).is_file():
            raise ValueError("全栈二进制路径无效")
    return binaries
