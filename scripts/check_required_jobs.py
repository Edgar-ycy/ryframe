from __future__ import annotations

import argparse
import json
import sys
from collections.abc import Mapping
from typing import Any


PLAN_OUTPUTS = (
    "preflight",
    "rust_gate",
    "resource_gate",
    "integration",
    "consumer_contract",
)
PLAN_CONTROLLED_JOBS = {
    "rust-gate": "rust_gate",
    "integration": "integration",
}
ALL_JOBS = (
    "plan",
    *PLAN_CONTROLLED_JOBS,
    "resource-gate",
    "windows-smoke",
    "security-audit",
)
EVENTS = ("push", "pull_request")
RESULTS = ("success", "failure", "cancelled", "skipped")


def validate_required_jobs(
    event: str,
    action: str,
    results: Mapping[str, str],
    plan_outputs: Mapping[str, str],
) -> list[str]:
    errors: list[str] = []
    if event not in EVENTS:
        errors.append(f"不支持的 GitHub 事件：{event}")
    missing = sorted(set(ALL_JOBS) - set(results))
    extra = sorted(set(results) - set(ALL_JOBS))
    if missing:
        errors.append(f"缺少 required job 结果：{', '.join(missing)}")
    if extra:
        errors.append(f"包含未知 required job：{', '.join(extra)}")
    missing_outputs = sorted(set(PLAN_OUTPUTS) - set(plan_outputs))
    extra_outputs = sorted(set(plan_outputs) - set(PLAN_OUTPUTS))
    if missing_outputs:
        errors.append(f"CI plan 缺少输出：{', '.join(missing_outputs)}")
    if extra_outputs:
        errors.append(f"CI plan 包含未知输出：{', '.join(extra_outputs)}")
    invalid_outputs = sorted(
        name for name, value in plan_outputs.items() if value not in ("true", "false")
    )
    if invalid_outputs:
        errors.append(f"CI plan 输出必须是 true/false：{', '.join(invalid_outputs)}")
    if errors:
        return errors

    expected = {name: "skipped" for name in ALL_JOBS}
    expected["plan"] = "success"
    edited = event == "pull_request" and action == "edited"
    for job, output in PLAN_CONTROLLED_JOBS.items():
        enabled = plan_outputs[output] == "true"
        expected[job] = "success" if enabled else "skipped"
    resource_enabled = (
        plan_outputs["resource_gate"] == "true"
        or plan_outputs["consumer_contract"] == "true"
    )
    expected["resource-gate"] = "success" if resource_enabled else "skipped"
    if edited:
        required_edited_plan = {
            "preflight": "false",
            "rust_gate": "false",
            "resource_gate": "false",
            "integration": "false",
            "consumer_contract": "true",
        }
        if dict(plan_outputs) != required_edited_plan:
            errors.append("pull_request.edited 必须只启用 consumer-contract")
    else:
        expected["security-audit"] = "success"
        expected["windows-smoke"] = "success"

    for name in ALL_JOBS:
        actual = results[name]
        if actual not in RESULTS:
            errors.append(f"{name} 返回未知结果：{actual}")
        elif actual != expected[name]:
            errors.append(f"{name} 期望 {expected[name]}，实际 {actual}")
    return errors


def _parse_needs_document(value: str) -> dict[str, Any]:
    try:
        document: Any = json.loads(value)
    except json.JSONDecodeError as error:
        raise ValueError(f"needs JSON 无效：{error}") from error
    if not isinstance(document, dict):
        raise ValueError("needs JSON 必须是对象")
    return document


def parse_needs_json(value: str) -> dict[str, str]:
    document = _parse_needs_document(value)
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


def parse_plan_outputs(value: str) -> dict[str, str]:
    document = _parse_needs_document(value)
    plan = document.get("plan")
    if not isinstance(plan, dict):
        raise ValueError("needs JSON 缺少 plan 对象")
    outputs = plan.get("outputs")
    if not isinstance(outputs, dict):
        raise ValueError("needs JSON 的 plan.outputs 必须是对象")
    parsed: dict[str, str] = {}
    for name, enabled in outputs.items():
        if not isinstance(name, str) or not name:
            raise ValueError("plan.outputs 名称必须是非空字符串")
        if not isinstance(enabled, str) or not enabled:
            raise ValueError(f"plan.outputs.{name} 必须是非空字符串")
        parsed[name] = enabled
    return parsed


def main() -> int:
    parser = argparse.ArgumentParser(description="校验 Required 汇总 job 的动态计划")
    parser.add_argument("--event", required=True)
    parser.add_argument("--action", default="")
    parser.add_argument("--needs-json", required=True)
    args = parser.parse_args()
    try:
        results = parse_needs_json(args.needs_json)
        plan_outputs = parse_plan_outputs(args.needs_json)
    except ValueError as error:
        parser.error(str(error))
    errors = validate_required_jobs(
        args.event,
        args.action,
        results,
        plan_outputs,
    )
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    action_label = f", action={args.action}" if args.action else ""
    print(f"Required 汇总校验通过（event={args.event}{action_label}）")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
