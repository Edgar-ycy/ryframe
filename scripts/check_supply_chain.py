from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_POLICY = ROOT / "scripts" / "supply_chain_policy.json"
DEFAULT_WORKFLOWS = ROOT / ".github" / "workflows"
TOOL_NAMES = ("cargo-audit", "cargo-deny", "cargo-cyclonedx", "trivy")
SEMVER = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
ACTION_REF = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_./-]+@[0-9a-f]{40}")
CONTAINER_ACTION_REF = re.compile(
    r"docker://[A-Za-z0-9_.:/-]+@sha256:[0-9a-f]{64}"
)
USES_LINE = re.compile(r"^\s*(?:-\s*)?uses:\s*([^\s#]+)", re.MULTILINE)


class PolicyError(ValueError):
    pass


def _object(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise PolicyError(f"{label} 必须是对象")
    return value


def _list(value: Any, label: str) -> list[Any]:
    if not isinstance(value, list):
        raise PolicyError(f"{label} 必须是数组")
    return value


def _exact_keys(value: dict[str, Any], expected: set[str], label: str) -> None:
    actual = set(value)
    if actual != expected:
        missing = sorted(expected - actual)
        extra = sorted(actual - expected)
        raise PolicyError(f"{label} 字段不匹配，缺少={missing}，多余={extra}")


def _text(value: Any, label: str, *, minimum: int = 1) -> str:
    if not isinstance(value, str) or len(value.strip()) < minimum:
        raise PolicyError(f"{label} 必须是至少 {minimum} 个字符的非空字符串")
    return value.strip()


def _expiry(value: Any, label: str, today: dt.date) -> dt.date:
    raw = _text(value, label)
    try:
        expires = dt.date.fromisoformat(raw)
    except ValueError as exc:
        raise PolicyError(f"{label} 必须是 YYYY-MM-DD 日期") from exc
    if expires <= today:
        raise PolicyError(f"{label} 已到期或不是未来日期：{raw}")
    return expires


def load_policy(path: Path = DEFAULT_POLICY, *, today: dt.date | None = None) -> dict[str, Any]:
    check_date = today or dt.date.today()
    try:
        policy = _object(json.loads(path.read_text(encoding="utf-8")), "供应链策略")
    except (OSError, json.JSONDecodeError) as exc:
        raise PolicyError(f"无法读取供应链策略 {path}: {exc}") from exc

    _exact_keys(
        policy,
        {
            "schema_version",
            "tools",
            "vulnerability_gate",
            "dependency_graph_exceptions",
        },
        "供应链策略",
    )
    if policy["schema_version"] != 1:
        raise PolicyError("schema_version 只允许为 1")

    tools = _object(policy["tools"], "tools")
    _exact_keys(tools, set(TOOL_NAMES), "tools")
    for name, version in tools.items():
        if not isinstance(version, str) or SEMVER.fullmatch(version) is None:
            raise PolicyError(f"工具 {name} 必须固定到完整三段版本")

    gate = _object(policy["vulnerability_gate"], "vulnerability_gate")
    _exact_keys(gate, {"severities", "exceptions"}, "vulnerability_gate")
    severities = _list(gate["severities"], "vulnerability_gate.severities")
    if len(severities) != len(set(severities)) or set(severities) != {"HIGH", "CRITICAL"}:
        raise PolicyError("漏洞门禁必须且只能覆盖 HIGH、CRITICAL")

    seen_vulnerabilities: set[tuple[str, str, str, str | None]] = set()
    for index, item in enumerate(_list(gate["exceptions"], "vulnerability_gate.exceptions")):
        exception = _object(item, f"漏洞例外[{index}]")
        allowed = {
            "id",
            "package",
            "installed_version",
            "owner",
            "expires",
            "reason",
            "target",
        }
        required = allowed - {"target"}
        actual = set(exception)
        if not required.issubset(actual) or not actual.issubset(allowed):
            raise PolicyError(f"漏洞例外[{index}] 字段不完整或包含未知字段")
        advisory_id = _text(exception["id"], f"漏洞例外[{index}].id")
        if re.fullmatch(r"[A-Z0-9][A-Z0-9._:-]+", advisory_id) is None:
            raise PolicyError(f"漏洞例外[{index}].id 格式无效")
        package = _text(exception["package"], f"漏洞例外[{index}].package")
        installed = _text(
            exception["installed_version"],
            f"漏洞例外[{index}].installed_version",
        )
        target = exception.get("target")
        if target is not None:
            target = _text(target, f"漏洞例外[{index}].target")
        _text(exception["owner"], f"漏洞例外[{index}].owner", minimum=3)
        _expiry(exception["expires"], f"漏洞例外[{index}].expires", check_date)
        _text(exception["reason"], f"漏洞例外[{index}].reason", minimum=12)
        identity = (advisory_id, package, installed, target)
        if identity in seen_vulnerabilities:
            raise PolicyError(f"漏洞例外[{index}] 重复")
        seen_vulnerabilities.add(identity)

    seen_packages: set[tuple[str, str]] = set()
    graph_exceptions = _list(
        policy["dependency_graph_exceptions"],
        "dependency_graph_exceptions",
    )
    for index, item in enumerate(graph_exceptions):
        exception = _object(item, f"依赖图例外[{index}]")
        _exact_keys(
            exception,
            {"package", "version", "enforcement", "owner", "expires", "reason"},
            f"依赖图例外[{index}]",
        )
        package = _text(exception["package"], f"依赖图例外[{index}].package")
        version = _text(exception["version"], f"依赖图例外[{index}].version")
        if exception["enforcement"] != "must_be_absent_from_resolved_graph":
            raise PolicyError(f"依赖图例外[{index}] 必须使用失败关闭的执行方式")
        _text(exception["owner"], f"依赖图例外[{index}].owner", minimum=3)
        _expiry(exception["expires"], f"依赖图例外[{index}].expires", check_date)
        _text(exception["reason"], f"依赖图例外[{index}].reason", minimum=12)
        identity = (package, version)
        if identity in seen_packages:
            raise PolicyError(f"依赖图例外[{index}] 重复")
        seen_packages.add(identity)

    return policy


def _step_block(lines: list[str], uses_index: int) -> str:
    uses_line = lines[uses_index]
    indent = len(uses_line) - len(uses_line.lstrip())
    step_indent = max(0, indent - 2)
    end = len(lines)
    marker = re.compile(rf"^ {{{step_indent}}}-\s")
    for index in range(uses_index + 1, len(lines)):
        if marker.match(lines[index]):
            end = index
            break
    return "\n".join(lines[uses_index:end])


def validate_workflows(workflow_dir: Path, policy: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    files = sorted((*workflow_dir.glob("*.yml"), *workflow_dir.glob("*.yaml")))
    if not files:
        return [f"没有找到工作流：{workflow_dir}"]

    combined: list[str] = []
    for path in files:
        text = path.read_text(encoding="utf-8")
        combined.append(text)
        for match in USES_LINE.finditer(text):
            reference = match.group(1).strip("'\"")
            if reference.startswith("./"):
                continue
            valid = (
                CONTAINER_ACTION_REF.fullmatch(reference)
                if reference.startswith("docker://")
                else ACTION_REF.fullmatch(reference)
            )
            if valid is None:
                line = text.count("\n", 0, match.start()) + 1
                errors.append(f"{path}:{line} action 未固定到提交 SHA 或镜像 digest：{reference}")

        lines = text.splitlines()
        for index, line in enumerate(lines):
            if "uses: taiki-e/install-action@" not in line:
                continue
            block = _step_block(lines, index)
            if re.search(r"^\s*fallback:\s*none\s*$", block, re.MULTILINE) is None:
                errors.append(f"{path}:{index + 1} install-action 必须禁用 fallback")
            if not any(f"{name}@" in block for name in TOOL_NAMES):
                errors.append(f"{path}:{index + 1} install-action 未声明固定版本工具")

        for match in re.finditer(r"\bdocker\s+(?:run|pull)\b", text):
            start = match.start()
            tail = text[start:]
            boundary = re.search(r"\n\s{6}-\s", tail)
            block = tail[: boundary.start()] if boundary else tail
            if re.search(r"@sha256:[0-9a-f]{64}\b", block) is None:
                line = text.count("\n", 0, start) + 1
                errors.append(f"{path}:{line} docker 外部镜像未固定到 sha256 digest")

    all_workflows = "\n".join(combined)
    for name, version in policy["tools"].items():
        occurrences = re.findall(rf"\b{re.escape(name)}@([^\s,]+)", all_workflows)
        if not occurrences:
            errors.append(f"工作流没有安装策略声明的工具 {name}@{version}")
        for actual in occurrences:
            if actual != version:
                errors.append(f"工具 {name} 版本漂移：期望 {version}，实际 {actual}")
        bare = re.search(rf"^\s*tool:\s*{re.escape(name)}\s*$", all_workflows, re.MULTILINE)
        if bare is not None:
            errors.append(f"工具 {name} 使用了自动升级写法")

    required_snippets = (
        "cargo cyclonedx",
        "trivy image",
        "--format cyclonedx",
        "--trivy-report",
        "actions/upload-artifact@",
    )
    for snippet in required_snippets:
        if snippet not in all_workflows:
            errors.append(f"工作流缺少供应链门禁：{snippet}")
    return errors


def validate_cyclonedx(path: Path, *, require_reproducible: bool = False) -> list[str]:
    try:
        document = _object(json.loads(path.read_text(encoding="utf-8")), "CycloneDX")
    except (OSError, json.JSONDecodeError, PolicyError) as exc:
        return [f"无法读取 CycloneDX 清单 {path}: {exc}"]
    errors: list[str] = []
    if document.get("bomFormat") != "CycloneDX":
        errors.append("SBOM 的 bomFormat 必须是 CycloneDX")
    if document.get("specVersion") not in {"1.5", "1.6", "1.7"}:
        errors.append("SBOM 规范版本不得低于 1.5")
    components = document.get("components")
    if not isinstance(components, list) or not components:
        errors.append("SBOM 必须包含非空 components")
    metadata = document.get("metadata")
    if not isinstance(metadata, dict) or not isinstance(metadata.get("component"), dict):
        errors.append("SBOM 必须声明顶层 metadata.component")
    if require_reproducible and document.get("serialNumber") is not None:
        errors.append("可复现 SBOM 不得包含随机 serialNumber")
    return errors


def evaluate_trivy_report(report_path: Path, policy: dict[str, Any]) -> list[str]:
    try:
        report = _object(json.loads(report_path.read_text(encoding="utf-8")), "Trivy 报告")
    except (OSError, json.JSONDecodeError, PolicyError) as exc:
        return [f"无法读取 Trivy 报告 {report_path}: {exc}"]
    results = report.get("Results")
    if not isinstance(results, list):
        return ["Trivy 报告缺少 Results 数组"]

    gate = policy["vulnerability_gate"]
    severities = set(gate["severities"])
    exceptions = gate["exceptions"]
    used: set[int] = set()
    errors: list[str] = []
    for result in results:
        if not isinstance(result, dict):
            errors.append("Trivy Results 包含非对象条目")
            continue
        target = result.get("Target")
        vulnerabilities = result.get("Vulnerabilities") or []
        if not isinstance(vulnerabilities, list):
            errors.append(f"Trivy 目标 {target!r} 的 Vulnerabilities 不是数组")
            continue
        for vulnerability in vulnerabilities:
            if not isinstance(vulnerability, dict):
                errors.append(f"Trivy 目标 {target!r} 包含无效漏洞条目")
                continue
            severity = vulnerability.get("Severity")
            if severity not in severities:
                continue
            advisory_id = vulnerability.get("VulnerabilityID")
            package = vulnerability.get("PkgName")
            installed = vulnerability.get("InstalledVersion")
            matched = False
            for index, exception in enumerate(exceptions):
                if (
                    exception["id"] == advisory_id
                    and exception["package"] == package
                    and exception["installed_version"] == installed
                    and (exception.get("target") is None or exception["target"] == target)
                ):
                    used.add(index)
                    matched = True
                    break
            if not matched:
                errors.append(
                    f"未放行的 {severity} 漏洞：{advisory_id} "
                    f"{package}@{installed} target={target}"
                )
    for index, exception in enumerate(exceptions):
        if index not in used:
            errors.append(
                "漏洞例外未被报告使用，必须删除或校正："
                f"{exception['id']} {exception['package']}@{exception['installed_version']}"
            )
    return errors


def resolved_graph_violations(tree_output: str, policy: dict[str, Any]) -> list[str]:
    resolved = {line.strip() for line in tree_output.splitlines() if line.strip()}
    errors: list[str] = []
    for exception in policy["dependency_graph_exceptions"]:
        package_id = f"{exception['package']} v{exception['version']}"
        if package_id in resolved:
            errors.append(f"条件依赖例外已进入实际构建图，必须升级：{package_id}")
    return errors


def verify_cargo_graph(policy: dict[str, Any]) -> list[str]:
    completed = subprocess.run(
        [
            "cargo",
            "tree",
            "--locked",
            "--workspace",
            "--all-features",
            "--target",
            "all",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ],
        cwd=ROOT,
        check=False,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if completed.returncode != 0:
        return [f"cargo tree 执行失败：{completed.stderr.strip()}"]
    return resolved_graph_violations(completed.stdout, policy)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="校验供应链策略、固定版本和安全报告")
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    parser.add_argument("--workflow-dir", type=Path, default=DEFAULT_WORKFLOWS)
    parser.add_argument("--trivy-report", type=Path)
    parser.add_argument("--cyclonedx", type=Path)
    parser.add_argument("--require-reproducible-cyclonedx", action="store_true")
    parser.add_argument("--verify-cargo-graph", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        policy = load_policy(args.policy)
    except PolicyError as exc:
        print(f"供应链策略无效：{exc}", file=sys.stderr)
        return 1

    errors = validate_workflows(args.workflow_dir, policy)
    if args.cyclonedx is not None:
        errors.extend(
            validate_cyclonedx(
                args.cyclonedx,
                require_reproducible=args.require_reproducible_cyclonedx,
            )
        )
    if args.trivy_report is not None:
        errors.extend(evaluate_trivy_report(args.trivy_report, policy))
    if args.verify_cargo_graph:
        errors.extend(verify_cargo_graph(policy))
    if errors:
        for error in errors:
            print(f"供应链门禁失败：{error}", file=sys.stderr)
        return 1
    print("供应链策略、固定版本和安全报告校验通过")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
