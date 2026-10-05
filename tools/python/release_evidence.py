"""联合发布证据的纯判定逻辑；只接受精确源码和最新运行尝试。"""

from __future__ import annotations

from dataclasses import dataclass
from hashlib import sha256
from pathlib import PurePosixPath
import re


class EvidenceError(ValueError):
    """证据缺失、身份不匹配或门禁失败。"""


EMPTY_SHA256 = sha256(b"").hexdigest()
BUSINESS_OUTPUTS = {
    "backend": (
        "crates/order-business/src/resources/mod.rs",
        "crates/order-business/src/generated/entities/order.rs",
        "crates/order-business/src/generated/openapi/order.rs",
        "crates/order-business/src/generated/handlers/order.rs",
        "crates/order-business/migrations/m_resource_initial_order.rs",
    ),
    "frontend": (
        "src/generated/resources/order/api.ts",
        "src/generated/resources/order/fields.ts",
        "src/generated/resources/order/index.ts",
        "src/generated/resources/order/page.vue",
        "src/generated/resources/order/registration.ts",
    ),
}


def _sha256(value: object) -> str | None:
    return (
        value
        if isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value)
        else None
    )


def _generated_files(snapshot: dict, name: str) -> dict[str, str]:
    entries = snapshot.get("files")
    if not isinstance(entries, list):
        raise EvidenceError(f"Device {name} 生成文件摘要不是列表")
    files: dict[str, str] = {}
    for entry in entries:
        if not isinstance(entry, dict):
            raise EvidenceError(f"Device {name} 生成文件摘要无效")
        path, digest = entry.get("path"), _sha256(entry.get("sha256"))
        parsed = PurePosixPath(path) if isinstance(path, str) else None
        if (
            parsed is None
            or parsed.is_absolute()
            or parsed.as_posix() != path
            or not path
            or ".." in parsed.parts
            or digest is None
            or path in files
        ):
            raise EvidenceError(f"Device {name} 生成文件摘要无效或重复")
        files[path] = digest
    return files


def _clean_source(value: object, expected_sha: str) -> bool:
    return (
        isinstance(value, dict)
        and set(value) == {"head", "patch_sha256", "files"}
        and value["head"] == expected_sha
        and value["patch_sha256"] == EMPTY_SHA256
        and value["files"] == []
    )


@dataclass(frozen=True)
class Requirement:
    repository: str
    workflow: str
    sha: str
    jobs: tuple[str, ...]
    tag: str | None = None


def latest_run(runs: list[dict], requirement: Requirement) -> dict | None:
    eligible = [
        run for run in runs
        if run.get("head_sha") == requirement.sha
        and run.get("event") == "push"
        and (requirement.tag is None or run.get("head_branch") == requirement.tag)
    ]
    if not eligible:
        return None
    # 同 SHA 新运行以及已有运行的最新重跑均不得被旧成功掩盖。
    return max(eligible, key=lambda run: (run.get("run_number", 0), run.get("id", 0), run.get("run_attempt", 0)))


def validate_run(run: dict, jobs: list[dict], requirement: Requirement, *, jobs_attempt: int) -> dict | None:
    if run.get("head_sha") != requirement.sha:
        raise EvidenceError("运行的源码 SHA 与发布目标不一致")
    if run.get("event") != "push" or (
        requirement.tag is not None and run.get("head_branch") != requirement.tag
    ):
        raise EvidenceError("运行不是目标协调 tag 的 push 验收")
    if not isinstance(run.get("id"), int) or not isinstance(run.get("run_attempt"), int) or run["run_attempt"] < 1:
        raise EvidenceError("运行缺少有效的 run ID 或 attempt")
    if run.get("status") in {"queued", "in_progress", "waiting", "pending", "requested"}:
        return None
    if run.get("status") != "completed" or run.get("conclusion") != "success":
        raise EvidenceError(f"运行 {run['id']} / {run['run_attempt']} 未成功：{run.get('conclusion')}")
    # GitHub job 响应不保证包含 run_attempt；由显式 attempts/{attempt}/jobs 请求绑定。
    if jobs_attempt != run["run_attempt"]:
        raise EvidenceError("job 列表不属于当前运行尝试")
    for job in jobs:
        if job.get("head_sha") != requirement.sha or job.get("run_id") != run["id"]:
            raise EvidenceError("job 的源码或运行身份不匹配")
        if "run_attempt" in job and job["run_attempt"] != jobs_attempt:
            raise EvidenceError("job 返回的 attempt 与请求不一致")
        if type(job.get("id")) is not int or job["id"] <= 0:
            raise EvidenceError("job 缺少有效 ID")
    for name in requirement.jobs:
        matches = [job for job in jobs if job.get("name") == name]
        if len(matches) != 1:
            raise EvidenceError(f"必需 job 缺失或重复：{name}")
        job = matches[0]
        if job.get("status") != "completed" or job.get("conclusion") != "success":
            raise EvidenceError(f"必需 job 未成功：{name} ({job.get('conclusion')})")
    # 包括可复用工作流展开的 job，不能用汇总成功掩盖失败或跳过。
    if any(job.get("status") != "completed" or job.get("conclusion") != "success" for job in jobs):
        raise EvidenceError("发布运行包含失败、取消、跳过或未完成的 job")
    return {
        "repository": requirement.repository, "workflow": requirement.workflow,
        "sha": requirement.sha, "run_id": run["id"], "attempt": run["run_attempt"],
        "jobs": [{"id": job["id"], "name": job["name"]} for job in jobs],
    }


def validate_pair(receipt: dict, evidence: dict, backend_sha: str, frontend_sha: str) -> None:
    expected = {
        "backend_sha": backend_sha, "frontend_sha": frontend_sha,
        "run_id": evidence["run_id"], "attempt": evidence["attempt"],
    }
    sources = receipt.get("sources")
    if (
        receipt.get("format_version") != 1
        or any(receipt.get(key) != value for key, value in expected.items())
        or not isinstance(sources, dict)
        or set(sources) != {"backend", "frontend"}
        or not _clean_source(sources["backend"], backend_sha)
        or not _clean_source(sources["frontend"], frontend_sha)
    ):
        raise EvidenceError("真实全栈证据与本次源码组合或运行尝试不一致")


def validate_fixture(
    receipt: object,
    backend_sha: str,
    frontend_sha: str,
    expected_fixture_sha256: str,
) -> None:
    identity = {"format_version": 1, "fixture": "business", "status": "ready"}
    if not isinstance(receipt, dict) or any(
        receipt.get(key) != value for key, value in identity.items()
    ):
        raise EvidenceError("业务 crate 生成工作树收据缺失或未完成")
    fixture_sha256 = _sha256(receipt.get("fixture_sha256"))
    if (
        fixture_sha256 in {None, EMPTY_SHA256}
        or fixture_sha256 != expected_fixture_sha256
    ):
        raise EvidenceError("业务 crate fixture 摘要与发布源码不一致")
    sources, generated_sources = receipt.get("sources"), receipt.get("generated")
    if not isinstance(sources, dict) or not isinstance(generated_sources, dict):
        raise EvidenceError("Device 缺少源码或生成内容指纹")
    generated_files = {}
    for name, expected in (("backend", backend_sha), ("frontend", frontend_sha)):
        source = sources.get(name)
        if not _clean_source(source, expected):
            raise EvidenceError("Device 必须从本次精确 SHA 的干净源码生成")
        generated = generated_sources.get(name)
        if (
            not isinstance(generated, dict)
            or generated.get("head") != expected
            or _sha256(generated.get("patch_sha256")) is None
        ):
            raise EvidenceError("Device 缺少生成内容的源码指纹")
        files = _generated_files(generated, name)
        generated_files[name] = files
        if generated.get("patch_sha256") == EMPTY_SHA256 and not files:
            raise EvidenceError(f"Device {name} 没有实际生成变化")
        missing = set(BUSINESS_OUTPUTS[name]).difference(files)
        if missing:
            raise EvidenceError(f"Device {name} 缺少必要生成输出：{sorted(missing)}")
        if any(files[path] == EMPTY_SHA256 for path in BUSINESS_OUTPUTS[name]):
            raise EvidenceError(f"Device {name} 必要生成输出为空")
    if (
        generated_files["backend"]["crates/order-business/src/resources/mod.rs"]
        != fixture_sha256
    ):
        raise EvidenceError("Device fixture 摘要与隔离工作树中的资源定义不一致")
