#!/usr/bin/env python3
"""验证当前产品不再暴露已移除的服务身份能力。历史记录与测试断言不参与扫描。"""

from __future__ import annotations

import argparse
import re
from pathlib import Path


REMOVED = re.compile(
    r"service[_-]accounts?|service[_-]delegations?|service[_-]credentials?|"
    r"service[_-]access[_-]audits?|ServiceAccount|ServiceDelegation|ServiceCredential|"
    r"ServiceAccessAudit|PepperKeyring|AgentQuery|AgentPageQuery|"
    r"Agent(?:Capability|User|Department|Post|DictionaryItem|Dictionary)Response|"
    r"PrebuiltApiEnvelope|"
    r"agent[_-](?:query|capabilit|directory|limiter)|RyFrameApiKey|X-RyFrame-Delegation|"
    r"/api/v1/agent/|"
    r"Agent API|Agent 查询|服务账号|服务委托|服务访问审计|个人服务委托|服务身份"
)
REMOVED_PATH_MARKERS = (
    "/crates/ryframe-application/src/agent/",
    "/crates/ryframe-application/src/ports/service_accounts/",
    "/crates/ryframe-application/src/system/service_account/",
    "/crates/ryframe-application/src/service_identity_secret.rs",
    "/crates/ryframe-api/src/dto/agent_dto.rs",
    "/crates/ryframe-api/src/handlers/agent_handler.rs",
    "/crates/ryframe-db/src/application_ports/agent/",
    "/crates/ryframe-db/src/application_ports/service_accounts/",
    "/crates/ryframe-db/src/repositories/service_authorization_repo.rs",
    "/src/api/generated/operations/agent.ts",
    "/src/api/generated/schema/agent.ts",
    "/src/features/service-accounts/",
    "/src/views/system/service-accounts/",
)
SUFFIXES = {
    ".rs",
    ".toml",
    ".json",
    ".sql",
    ".yml",
    ".yaml",
    ".js",
    ".cjs",
    ".mjs",
    ".ts",
    ".cts",
    ".mts",
    ".vue",
}


def scan_files(paths: list[Path]) -> list[str]:
    errors = []
    for path in sorted(set(paths)):
        normalized_path = "/" + path.as_posix().lstrip("/")
        if REMOVED.search(path.name) or any(
            marker in normalized_path for marker in REMOVED_PATH_MARKERS
        ):
            errors.append(f"已移除的产品文件仍存在：{path}")
        if path.suffix not in SUFFIXES and not path.name.startswith(".env"):
            continue
        for number, line in enumerate(
            path.read_text(encoding="utf-8-sig").splitlines(), 1
        ):
            if REMOVED.search(line):
                errors.append(f"已移除的产品定义仍存在：{path}:{number}")
    return errors


def product_files(root: Path) -> list[Path]:
    directories = [
        *root.glob("crates/*/src"),
        *root.glob("crates/*/build_support"),
        root / "config",
        root / "catalog",
        root / "deploy",
    ]
    return [
        path
        for directory in directories
        for path in directory.rglob("*")
        if path.is_file()
    ] + [
        root / "Cargo.toml",
        *root.glob("crates/*/Cargo.toml"),
        *root.glob("crates/*/build.rs"),
        root / "openapi/openapi.json",
        root / "sql/ryframe_config.sql",
    ]


def frontend_files(frontend: Path) -> list[Path]:
    return [
        frontend / "package.json",
        frontend / "openapi/openapi.json",
        frontend / "vite.config.ts",
        *frontend.glob("playwright*.ts"),
        *[path for path in (frontend / "src").rglob("*") if path.is_file()],
        *[
            path
            for path in (frontend / "scripts").rglob("*")
            if path.is_file()
            and "tests" not in path.relative_to(frontend / "scripts").parts
        ],
    ]


def check(root: Path, frontend: Path | None = None) -> list[str]:
    errors = scan_files(product_files(root))
    if frontend is not None:
        errors.extend(scan_files(frontend_files(frontend)))
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--frontend-dir", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    if args.frontend_dir is not None and not (args.frontend_dir / "src").is_dir():
        parser.error("--frontend-dir 必须指向现有前端项目")
    errors = check(root, args.frontend_dir)
    for error in errors:
        print(error)
    if not errors:
        print("服务身份删除范围检查通过")
    return bool(errors)


if __name__ == "__main__":
    raise SystemExit(main())
