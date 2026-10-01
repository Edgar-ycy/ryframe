#!/usr/bin/env python3
"""校验生产部署资产及最终镜像的最小安全边界。"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import tarfile
import tempfile
import tomllib
from pathlib import Path
from typing import BinaryIO
from urllib.parse import unquote


ROOT = Path(__file__).resolve().parents[2]
DOCKERFILE = ROOT / "deploy" / "Dockerfile"
COMPOSE_FILE = ROOT / "deploy" / "compose.prod.yml"
ALERT_RULES = ROOT / "deploy" / "prometheus" / "ryframe-alerts.yml"
FIXTURE_ENV = ROOT / "tools" / "fixtures" / "deployment" / "deploy.env"
EXPECTED_BINARIES = {"ryframe", "ryframe-migrate", "ryframe-worker"}
FORBIDDEN_RUNTIME_TOOLS = {
    "bin/bash",
    "bin/sh",
    "usr/bin/apt",
    "usr/bin/apt-get",
    "usr/bin/curl",
    "usr/bin/dpkg",
}
ONLINE_GENERATOR_PATTERN = re.compile(rb"/api/v1/tools/gen|tools:gen(?:[^a-z]|$)")
EXPECTED_BASE_IMAGES = {
    "RUST_IMAGE": (
        "rust:1.98.1-bookworm@"
        "sha256:93ce27a88655056a51dbdd8f5f2d7ddc071c7b0070fb288a37b5a285fc83971e"
    ),
    "RUNTIME_IMAGE": (
        "gcr.io/distroless/base-nossl-debian13:nonroot@"
        "sha256:8c563c1fb5e120606f0d85733049775faed6192e2bd2223ef283a5393eec22b9"
    ),
}
REPOSITORY_BLOB_PREFIX = "https://github.com/Edgar-ycy/ryframe/blob/main/"
ACTION_SHA = re.compile(r"^[^\s@]+@[0-9a-f]{40}$")
CONTAINER_ACTION_DIGEST = re.compile(r"^docker://[^\s@]+@sha256:[0-9a-f]{64}$")
IMMUTABLE_IMAGE = re.compile(r"^[^\s@]+@sha256:[0-9a-f]{64}$")
IMMUTABLE_TAGGED_IMAGE = re.compile(
    r"^[A-Za-z0-9._/-]+:[A-Za-z0-9][A-Za-z0-9._-]*@sha256:[0-9a-f]{64}$"
)


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def parse_env(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for number, raw_line in enumerate(read(path).splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        name, separator, value = line.partition("=")
        if not separator or not re.fullmatch(r"[A-Z][A-Z0-9_]*", name):
            raise ValueError(f"{path}:{number}: 环境变量格式无效")
        if name in values:
            raise ValueError(f"{path}:{number}: 环境变量重复: {name}")
        values[name] = value
    return values


def markdown_anchors(path: Path) -> set[str]:
    """按 GitHub 标题规则生成本文档实际存在的锚点。"""
    anchors: set[str] = set()
    occurrences: dict[str, int] = {}
    fence: str | None = None
    for line in read(path).splitlines():
        fence_match = re.match(r"^[ \t]*(`{3,}|~{3,})", line)
        if fence_match:
            marker = fence_match.group(1)[0]
            if fence is None:
                fence = marker
            elif fence == marker:
                fence = None
            continue
        if fence is not None:
            continue
        match = re.match(r"^#{1,6}[ \t]+(.+?)[ \t]*#*[ \t]*$", line)
        if match is None:
            continue
        heading = match.group(1).strip().lower()
        base = re.sub(r"[^\w\- ]", "", heading)
        base = re.sub(r"[ \t]+", "-", base)
        if not base:
            continue
        count = occurrences.get(base, 0)
        occurrences[base] = count + 1
        anchors.add(base if count == 0 else f"{base}-{count}")
    return anchors


def validate_runbook_url(url: str) -> str | None:
    if not url.startswith(REPOSITORY_BLOB_PREFIX):
        return "不是稳定的绝对仓库 URL"
    relative, separator, fragment = url.removeprefix(REPOSITORY_BLOB_PREFIX).partition("#")
    if not separator or not relative or not fragment:
        return "必须同时包含仓库文档路径和标题锚点"
    document = (ROOT / unquote(relative)).resolve()
    try:
        document.relative_to(ROOT.resolve())
    except ValueError:
        return "文档路径越过仓库边界"
    if not document.is_file():
        return f"对应文档不存在: {relative}"
    decoded_fragment = unquote(fragment).lower()
    if decoded_fragment not in markdown_anchors(document):
        return f"对应标题锚点不存在: {fragment}"
    return None


def check_dockerfile(violations: list[str]) -> None:
    source = read(DOCKERFILE)
    check_rust_toolchain(source, violations)
    normalized = re.sub(r"\\\r?\n\s*", " ", source)
    for argument, expected in EXPECTED_BASE_IMAGES.items():
        actual = re.findall(
            rf"^ARG\s+{re.escape(argument)}=([^\s]+)\s*$",
            source,
            re.MULTILINE,
        )
        if actual != [expected]:
            violations.append(
                f"生产 Dockerfile 的 {argument} 必须精确固定为 {expected}"
            )
        elif not IMMUTABLE_TAGGED_IMAGE.fullmatch(actual[0]):
            violations.append(
                f"生产 Dockerfile 的 {argument} 必须同时保留清晰标签和完整 sha256 摘要"
            )
    built = set(re.findall(r"--bin\s+([a-zA-Z0-9_-]+)", normalized))
    copied = set(
        re.findall(
            r"^COPY\s+--from=builder\s+\S+\s+/usr/local/bin/([a-zA-Z0-9_-]+)\s*$",
            source,
            re.MULTILINE,
        )
    )
    if built != EXPECTED_BINARIES:
        violations.append(f"生产构建二进制必须精确为 {sorted(EXPECTED_BINARIES)}，当前为 {sorted(built)}")
    if copied != EXPECTED_BINARIES:
        violations.append(f"运行镜像二进制必须精确为 {sorted(EXPECTED_BINARIES)}，当前为 {sorted(copied)}")
    if "--no-default-features" not in normalized:
        violations.append("生产构建必须禁用默认 feature")
    for forbidden in ("ryframe-generator", "ryframe-reset", "ryframe-db-reset"):
        if forbidden in source:
            violations.append(f"生产 Dockerfile 不得包含 {forbidden}")
    for forbidden in ("apt-get", "curl", "useradd"):
        if forbidden in source:
            violations.append(f"最小运行镜像不得包含 {forbidden}")
    if 'CMD ["/usr/local/bin/ryframe", "--healthcheck"]' not in source:
        violations.append("生产镜像必须使用 API 内置健康检查")


def check_rust_toolchain(source: str, violations: list[str]) -> None:
    channel = tomllib.loads(read(ROOT / "rust-toolchain.toml"))["toolchain"]["channel"]
    minimum = tomllib.loads(read(ROOT / "Cargo.toml"))["workspace"]["package"]["rust-version"]
    if not re.fullmatch(r"1\.\d+\.\d+", channel):
        violations.append("Rust 工具链必须固定到稳定版的完整版本号")
        return
    if minimum != channel.rsplit(".", 1)[0]:
        violations.append("workspace rust-version 必须与固定 Rust 工具链的主次版本一致")
    image_version = re.findall(r"^ARG RUST_IMAGE=rust:([^\s-]+)-", source, re.MULTILINE)
    if image_version != [channel]:
        violations.append("生产构建镜像的 Rust 版本必须与 rust-toolchain.toml 完全一致")


def check_online_generator(violations: list[str]) -> None:
    source_root = ROOT / "crates" / "ryframe-api" / "src"
    patterns = {
        "在线生成器 handler": re.compile(r"\bgenerator_handler\b"),
        "在线生成器 API": re.compile(r"/api/v1/tools/gen(?:/|[\"'])"),
        "在线生成器 crate": re.compile(r"\bryframe_generator\b"),
    }
    for path in sorted(source_root.rglob("*.rs")):
        source = read(path)
        for label, pattern in patterns.items():
            if pattern.search(source):
                violations.append(f"{path.relative_to(ROOT)}: 生产 API 仍包含{label}")


def check_compose_fixture(violations: list[str]) -> None:
    compose = read(COMPOSE_FILE)
    if "curl" in compose or "CMD-SHELL" in compose:
        violations.append("生产 Compose 健康检查不得依赖 shell 或 curl")
    for command in (
        '["CMD", "/usr/local/bin/ryframe", "--healthcheck"]',
        '["CMD", "/usr/local/bin/ryframe-worker", "--healthcheck"]',
    ):
        if command not in compose:
            violations.append(f"生产 Compose 缺少内置健康检查: {command}")
    try:
        values = parse_env(FIXTURE_ENV)
    except (OSError, ValueError) as error:
        violations.append(str(error))
        return

    required = set(re.findall(r"\$\{([A-Z][A-Z0-9_]*):\?[^}]+}", compose))
    missing = sorted(required - values.keys())
    if missing:
        violations.append(f"Compose 校验环境缺少变量: {', '.join(missing)}")
    image = values.get("RYFRAME_IMAGE", "")
    if not IMMUTABLE_IMAGE.fullmatch(image):
        violations.append("Compose 校验镜像必须使用完整 sha256 摘要")
    forbidden_values = sorted(
        name
        for name in values
        if re.search(r"(?:PASSWORD|SECRET|TOKEN|PRIVATE_KEY)$", name)
    )
    if forbidden_values:
        violations.append(
            "Compose fixture 只能保存无秘密配置或文件路径: " + ", ".join(forbidden_values)
        )
    if not re.search(r"^\s*APP_ENV:\s*prod\s*$", compose, re.MULTILINE):
        violations.append("生产 Compose 必须固定 APP_ENV=prod")


def check_alert_runbooks(violations: list[str]) -> None:
    source = read(ALERT_RULES)
    alerts = list(
        re.finditer(
            r"^[ \t]*- alert:[ \t]*([^\s]+)[ \t]*$", source, re.MULTILINE
        )
    )
    if not alerts:
        violations.append("Prometheus 规则中没有告警")
        return
    for index, alert in enumerate(alerts):
        end = alerts[index + 1].start() if index + 1 < len(alerts) else len(source)
        block = source[alert.start() : end]
        runbook = re.search(
            r'^[ \t]*runbook_url:[ \t]*["\']([^"\']+)["\'][ \t]*$',
            block,
            re.MULTILINE,
        )
        name = alert.group(1)
        if runbook is None:
            violations.append(f"告警 {name} 缺少 runbook_url")
        elif problem := validate_runbook_url(runbook.group(1)):
            violations.append(f"告警 {name} 的 runbook_url {problem}")
    if re.search(r"^[ \t]*runbook:[ \t]*", source, re.MULTILINE):
        violations.append("Prometheus 规则不得继续使用相对 runbook annotation")


def check_pinned_actions(violations: list[str]) -> None:
    workflow_root = ROOT / ".github" / "workflows"
    for path in sorted([*workflow_root.glob("*.yml"), *workflow_root.glob("*.yaml")]):
        for number, line in enumerate(read(path).splitlines(), start=1):
            match = re.match(r"^\s*-?\s*uses:\s*([^\s#]+)", line)
            if match is None:
                continue
            reference = match.group(1).strip("\"'")
            if reference.startswith("./"):
                continue
            if not (
                ACTION_SHA.fullmatch(reference)
                or CONTAINER_ACTION_DIGEST.fullmatch(reference)
            ):
                violations.append(
                    f"{path.relative_to(ROOT)}:{number}: action 必须固定到 commit 或 sha256 digest"
                )


def normalize_archive_name(name: str) -> str:
    return name.removeprefix("./").lstrip("/").rstrip("/")


def stream_contains(source: BinaryIO, pattern: re.Pattern[bytes]) -> bool:
    overlap = b""
    while chunk := source.read(1024 * 1024):
        data = overlap + chunk
        if pattern.search(data):
            return True
        overlap = data[-128:]
    return False


def inspect_image_archive(path: Path) -> list[str]:
    violations: list[str] = []
    with tarfile.open(path, mode="r") as archive:
        members = {
            normalize_archive_name(member.name): member
            for member in archive.getmembers()
            if normalize_archive_name(member.name)
        }
        binary_root = "usr/local/bin/"
        actual = {
            name.removeprefix(binary_root): member
            for name, member in members.items()
            if name.startswith(binary_root) and "/" not in name.removeprefix(binary_root)
        }
        if set(actual) != EXPECTED_BINARIES:
            violations.append(
                "生产镜像 /usr/local/bin 条目必须精确为 "
                f"{sorted(EXPECTED_BINARIES)}，当前为 {sorted(actual)}"
            )
        for name in sorted(EXPECTED_BINARIES & actual.keys()):
            member = actual[name]
            if not member.isfile() or member.issym() or member.islnk() or member.mode & 0o111 == 0:
                violations.append(f"生产镜像 {name} 必须是非链接的可执行普通文件")

        runtime_tools = sorted(FORBIDDEN_RUNTIME_TOOLS & members.keys())
        if runtime_tools:
            violations.append("生产镜像不得包含 shell、包管理器或 curl: " + ", ".join(runtime_tools))
        suspicious = sorted(name for name in members if is_suspicious_generated_tool(name))
        if suspicious:
            violations.append("生产镜像包含 generator/reset 可疑路径: " + ", ".join(suspicious))
        endpoint_hits = scan_online_generator_endpoints(archive, actual)
        if endpoint_hits:
            violations.append(
                "生产镜像二进制仍包含在线生成器端点或权限: " + ", ".join(endpoint_hits)
            )
    return violations


def is_suspicious_generated_tool(name: str) -> bool:
    lowered = name.lower()
    base = lowered.rsplit("/", maxsplit=1)[-1]
    in_product_tree = lowered.startswith(("usr/local/bin/", "opt/ryframe/", "var/lib/ryframe/"))
    return (in_product_tree and ("generator" in base or "reset" in base)) or (
        "ryframe" in base and ("generator" in base or "reset" in base)
    )


def scan_online_generator_endpoints(
    archive: tarfile.TarFile, binaries: dict[str, tarfile.TarInfo]
) -> list[str]:
    hits: list[str] = []
    for name in sorted(EXPECTED_BINARIES & binaries.keys()):
        source = archive.extractfile(binaries[name])
        if source is not None and stream_contains(source, ONLINE_GENERATOR_PATTERN):
            hits.append(f"/usr/local/bin/{name}")
    return hits


def export_image_filesystem(image: str, target: Path) -> None:
    created = subprocess.run(
        ["docker", "create", image], check=True, capture_output=True, text=True
    )
    container_id = created.stdout.strip()
    if not re.fullmatch(r"[0-9a-f]{12,64}", container_id):
        raise ValueError("docker create 未返回合法容器 ID")
    try:
        with target.open("wb") as output:
            subprocess.run(["docker", "export", container_id], check=True, stdout=output)
    finally:
        subprocess.run(
            ["docker", "rm", "--force", container_id],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )


def inspect_image(image: str, expected_commit: str | None) -> list[str]:
    if not re.fullmatch(r"[A-Za-z0-9._/@:+-]+", image):
        raise ValueError("镜像引用格式无效")
    result = subprocess.run(
        ["docker", "image", "inspect", image], check=True, capture_output=True, text=True
    )
    document = json.loads(result.stdout)
    if not isinstance(document, list) or len(document) != 1:
        raise ValueError("docker image inspect 返回了意外结果")
    config = document[0].get("Config") or {}
    violations: list[str] = []
    if config.get("User") != "10001:10001":
        violations.append("生产镜像必须以固定的数值非 root 用户 10001:10001 运行")
    if config.get("Entrypoint") != ["/usr/local/bin/ryframe"]:
        violations.append("生产镜像入口必须固定为 /usr/local/bin/ryframe")
    if expected_commit:
        revision = (config.get("Labels") or {}).get("org.opencontainers.image.revision")
        if revision != expected_commit:
            violations.append("生产镜像 revision 标签与源码提交不一致")

    inspection_root = ROOT / "target" / "image-inspection"
    inspection_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix="ryframe-image-inspect-", dir=inspection_root
    ) as directory:
        archive = Path(directory) / "filesystem.tar"
        export_image_filesystem(image, archive)
        violations.extend(inspect_image_archive(archive))
    return violations


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image", help="检查已经构建的生产镜像")
    parser.add_argument("--expected-commit", help="镜像必须携带的 40 位源码提交")
    args = parser.parse_args()
    if args.expected_commit and not re.fullmatch(r"[0-9a-f]{40}", args.expected_commit):
        parser.error("--expected-commit 必须是 40 位小写十六进制提交")
    if args.expected_commit and not args.image:
        parser.error("--expected-commit 必须与 --image 一起使用")

    violations: list[str] = []
    try:
        if args.image:
            violations.extend(inspect_image(args.image, args.expected_commit))
        else:
            check_dockerfile(violations)
            check_online_generator(violations)
            check_compose_fixture(violations)
            check_alert_runbooks(violations)
            check_pinned_actions(violations)
    except (OSError, ValueError, subprocess.CalledProcessError, json.JSONDecodeError) as error:
        print(f"部署资产检查执行失败: {error}")
        return 1

    if violations:
        print("部署资产检查失败:")
        for violation in violations:
            print(f"  - {violation}")
        return 1
    suffix = "（生产镜像）" if args.image else "（静态）"
    print(f"部署资产检查通过{suffix}。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
