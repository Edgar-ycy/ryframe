from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Mapping
from typing import Any


ALWAYS_REQUIRED = (
    "check",
    "integration",
    "security-audit",
    "deployment-assets",
)
ALL_JOBS = (*ALWAYS_REQUIRED, "consumer-contract", "supply-chain", "windows-smoke")
EVENTS = ("push", "pull_request", "schedule", "workflow_dispatch")
RESULTS = ("success", "failure", "cancelled", "skipped")


def validate_required_jobs(event: str, results: Mapping[str, str]) -> list[str]:
    errors: list[str] = []
    if event not in EVENTS:
        errors.append(f"不支持的 GitHub 事件：{event}")
    missing = sorted(set(ALL_JOBS) - set(results))
    extra = sorted(set(results) - set(ALL_JOBS))
    if missing:
        errors.append(f"缺少 required job 结果：{', '.join(missing)}")
    if extra:
        errors.append(f"包含未知 required job：{', '.join(extra)}")
    if errors:
        return errors

    expected = {name: "success" for name in ALWAYS_REQUIRED}
    expected["consumer-contract"] = "success" if event == "pull_request" else "skipped"
    expected["supply-chain"] = (
        "success" if event in ("schedule", "workflow_dispatch") else "skipped"
    )
    expected["windows-smoke"] = "skipped" if event == "schedule" else "success"
    for name in ALL_JOBS:
        actual = results[name]
        if actual not in RESULTS:
            errors.append(f"{name} 返回未知结果：{actual}")
        elif actual != expected[name]:
            errors.append(f"{name} 期望 {expected[name]}，实际 {actual}")
    return errors


def parse_needs_json(value: str) -> dict[str, str]:
    try:
        document: Any = json.loads(value)
    except json.JSONDecodeError as error:
        raise ValueError(f"needs JSON 无效：{error}") from error
    if not isinstance(document, dict):
        raise ValueError("needs JSON 必须是对象")
    results: dict[str, str] = {}
    for name, need in document.items():
        if not isinstance(name, str) or not name:
            raise ValueError("needs JSON 的 job 名称必须是非空字符串")
        if not isinstance(need, dict):
            raise ValueError(f"needs JSON 的 {name} 必须是对象")
        result = need.get("result")
        if not isinstance(result, str) or not result:
            raise ValueError(f"needs JSON 的 {name}.result 必须是非空字符串")
        results[name] = result
    return results


def main() -> int:
    parser = argparse.ArgumentParser(description="校验 Required 汇总 job 的事件矩阵")
    parser.add_argument("--event", required=True)
    parser.add_argument("--needs-json", required=True)
    args = parser.parse_args()
    try:
        results = parse_needs_json(args.needs_json)
    except ValueError as error:
        parser.error(str(error))
    errors = validate_required_jobs(args.event, results)
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    print(f"Required 汇总校验通过（event={args.event}）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
