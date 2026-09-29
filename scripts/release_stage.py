"""工具共用的版本阶段判断。只读 Workspace 版本与受信 Git 历史，不连接网络。"""

from __future__ import annotations

import argparse
import io
import json
import re
import subprocess
import tomllib
from pathlib import Path


VERSION = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:[-+][0-9A-Za-z.-]+)?")


def workspace_version(content: str) -> str:
    value = (
        tomllib.loads(content).get("workspace", {}).get("package", {}).get("version")
    )
    if not isinstance(value, str) or VERSION.fullmatch(value) is None:
        raise ValueError("Cargo Workspace 缺少有效的 package.version")
    return value


def stable_version(version: str) -> bool:
    match = VERSION.fullmatch(version)
    if match is None:
        raise ValueError("版本必须使用 SemVer")
    return int(match.group(1)) >= 1


def git(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", "-C", str(root), *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
    )
    if result.returncode:
        raise ValueError(f"无法读取稳定版保护历史：{result.stderr.strip()}")
    return result.stdout


def historical_manifests(root: Path, trusted_ref: str) -> list[str]:
    revisions = git(
        root, "log", trusted_ref, "--format=%H", "--", "Cargo.toml"
    ).splitlines()
    if not revisions:
        return []
    result = subprocess.run(
        ["git", "-C", str(root), "cat-file", "--batch"],
        input="".join(f"{revision}:Cargo.toml\n" for revision in revisions).encode(),
        capture_output=True,
        check=True,
        timeout=30,
    )
    stream = io.BytesIO(result.stdout)
    manifests = []
    for _ in revisions:
        header = stream.readline().split()
        if len(header) != 3 or header[1] != b"blob":
            raise ValueError("稳定版保护历史中的 Cargo.toml 无法读取")
        size = int(header[2])
        content = stream.read(size)
        if len(content) != size or stream.read(1) != b"\n":
            raise ValueError("稳定版保护历史不完整")
        manifests.append(content.decode("utf-8"))
    if stream.read():
        raise ValueError("稳定版保护历史包含多余输出")
    return manifests


def release_stage(root: Path, trusted_ref: str = "HEAD") -> dict:
    version = workspace_version((root / "Cargo.toml").read_text(encoding="utf-8"))
    stable = stable_version(version)
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/-]*", trusted_ref):
        raise ValueError("受信 Git 引用无效")
    # 稳定阶段直接保护；开发阶段必须证明没有已生效的稳定版历史。
    if not stable:
        if git(root, "rev-parse", "--is-shallow-repository").strip() != "false":
            raise ValueError(
                "稳定版保护需要完整 Git 历史，浅克隆不能证明仍处于开发阶段"
            )
        tags = git(root, "tag", "--list").splitlines()
        stable = any(re.fullmatch(r"v[1-9]\d*\.\d+\.\d+", tag) for tag in tags)
        for historical in historical_manifests(root, trusted_ref):
            try:
                stable = stable or stable_version(workspace_version(historical))
            except ValueError:
                # 建立 Workspace 之前的清单不定义版本阶段。
                continue
            if stable:
                break
        if stable:
            raise ValueError(
                "稳定版保护已生效，禁止降低 Workspace 版本绕过契约或迁移保护"
            )
    return {"version": version, "stable": stable}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--trusted-ref", default="HEAD")
    args = parser.parse_args()
    print(json.dumps(release_stage(args.root, args.trusted_ref)))


if __name__ == "__main__":
    main()
