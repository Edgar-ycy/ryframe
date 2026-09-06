"""有界等待联合发布 CI，并保存可复核的运行、job 和源码组合证据。"""

from __future__ import annotations

import argparse
from hashlib import sha256
import json
import os
import re
import subprocess
import tempfile
import time
import zipfile
from pathlib import Path, PurePosixPath

from full_stack_provenance import (
    ARCHIVE_RECEIPT_LIMITS,
    source_pair_receipt,
    validate_archive_evidence,
    verify_source_pair_receipt,
)

from release_evidence import (
    EvidenceError,
    Requirement,
    latest_run,
    validate_fixture,
    validate_pair,
    validate_run,
)

API_DEADLINE: float | None = None
ARTIFACT_ARCHIVE_MAX_BYTES = 256 * 1024 * 1024


def _api_timeout() -> float:
    remaining = 60 if API_DEADLINE is None else min(60, API_DEADLINE - time.monotonic())
    if remaining <= 0:
        raise EvidenceError("发布 CI 证据采集超过总截止时间")
    return remaining


def api(endpoint: str):
    result = subprocess.run(
        ["gh", "api", endpoint],
        capture_output=True,
        check=True,
        timeout=_api_timeout(),
    )
    return json.loads(result.stdout)


def _artifact_size(artifact: dict) -> int:
    size = artifact.get("size_in_bytes")
    if type(size) is not int or not 0 < size <= ARTIFACT_ARCHIVE_MAX_BYTES:
        raise EvidenceError("全栈证据 ZIP 缺少有效大小或超过下载硬上限")
    return size


def download_archive(artifact: dict, endpoint: str, destination: Path) -> None:
    expected_size = _artifact_size(artifact)
    with destination.open("xb") as stream:
        subprocess.run(
            ["gh", "api", endpoint],
            stdout=stream,
            stderr=subprocess.PIPE,
            check=True,
            timeout=_api_timeout(),
        )
    actual_size = destination.stat().st_size
    if actual_size != expected_size or actual_size > ARTIFACT_ARCHIVE_MAX_BYTES:
        raise EvidenceError("全栈证据 ZIP 实际大小与受控元数据不一致")


def pages(endpoint: str, field: str) -> list[dict]:
    result = []
    for page in range(1, 101):
        separator = "&" if "?" in endpoint else "?"
        items = api(f"{endpoint}{separator}per_page=100&page={page}")[field]
        result.extend(items)
        if len(items) < 100:
            return result
    raise EvidenceError("证据分页超出安全上限，拒绝截断后判定成功")


def collect(requirement: Requirement) -> dict | None:
    prefix = f"repos/{requirement.repository}/actions"
    runs = pages(f"{prefix}/workflows/{requirement.workflow}/runs?head_sha={requirement.sha}", "workflow_runs")
    selected = latest_run(runs, requirement)
    if selected is None:
        return None
    run = api(f"{prefix}/runs/{selected['id']}")
    if run.get("id") != selected["id"]:
        raise EvidenceError("运行详情与已选择的 run ID 不一致")
    jobs = []
    if run.get("status") == "completed":
        jobs = pages(f"{prefix}/runs/{run['id']}/attempts/{run['run_attempt']}/jobs", "jobs")
    # 拉取 jobs 期间的重跑也必须重新等待。
    current = api(f"{prefix}/runs/{run['id']}")
    newest = latest_run(pages(f"{prefix}/workflows/{requirement.workflow}/runs?head_sha={requirement.sha}", "workflow_runs"), requirement)
    identity = lambda item: tuple(item.get(field) for field in ("id", "run_attempt", "head_sha", "status", "conclusion"))
    if newest is None or identity(current) != identity(run) or identity(newest) != identity(run):
        return None
    return validate_run(run, jobs, requirement, jobs_attempt=run["run_attempt"])


def archive_evidence_entries(archive: zipfile.ZipFile) -> list[tuple[str, bytes]]:
    entries = []
    for entry in archive.infolist():
        name = PurePosixPath(entry.filename).name
        if name not in ARCHIVE_RECEIPT_LIMITS:
            continue
        limit = ARCHIVE_RECEIPT_LIMITS[name]
        if entry.is_dir() or type(entry.file_size) is not int or not 0 < entry.file_size <= limit:
            raise EvidenceError(f"全栈产物 {name} 大小无效")
        with archive.open(entry) as stream:
            raw = stream.read(limit + 1)
            trailing = stream.read(1)
        if len(raw) != entry.file_size or len(raw) > limit or trailing:
            raise EvidenceError(f"全栈产物 {name} 实际大小与归档元数据不一致")
        entries.append((entry.filename, raw))
    return entries


def pair_receipts(
    evidence: dict,
    backend_sha: str,
    frontend_sha: str,
    fixture_sha256: str,
) -> dict:
    prefix = f"repos/{evidence['repository']}/actions"
    artifacts = pages(f"{prefix}/runs/{evidence['run_id']}/artifacts", "artifacts")
    selected = []
    for fixture, suffix in (("core", ""), ("device", "-device")):
        name = f"ryframe-full-stack-{evidence['run_id']}-{evidence['attempt']}{suffix}"
        matches = [
            item
            for item in artifacts
            if item.get("name") == name and not item.get("expired")
        ]
        if len(matches) != 1:
            raise EvidenceError(f"真实全栈 {fixture} 证据产物缺失、重复或过期")
        artifact = matches[0]
        if type(artifact.get("id")) is not int or artifact["id"] <= 0:
            raise EvidenceError(f"真实全栈 {fixture} 证据产物缺少有效 ID")
        _artifact_size(artifact)
        selected.append((fixture, name, artifact))

    result = {}
    with tempfile.TemporaryDirectory(prefix="ryframe-release-evidence-") as temporary:
        directory = Path(temporary)
        for fixture, name, artifact in selected:
            archive_path = directory / f"{fixture}.zip"
            endpoint = f"{prefix}/artifacts/{artifact['id']}/zip"
            download_archive(artifact, endpoint, archive_path)
            with zipfile.ZipFile(archive_path) as archive:
                try:
                    validated = validate_archive_evidence(
                        archive_evidence_entries(archive),
                        fixture=fixture,
                        backend_sha=backend_sha,
                        frontend_sha=frontend_sha,
                        run_id=evidence["run_id"],
                        attempt=evidence["attempt"],
                        fixture_sha256=fixture_sha256 if fixture == "device" else None,
                    )
                except ValueError as error:
                    raise EvidenceError(str(error)) from None
                receipt = {
                    **validated["source_pair"],
                    "archive_evidence": {
                        name: validated[name]
                        for name in ("build_evidence", "runtime", "runtime_evidence")
                    },
                }
                receipt["artifact"] = {
                    "id": artifact["id"],
                    "name": name,
                    "size_in_bytes": _artifact_size(artifact),
                }
                if fixture == "device":
                    receipt["fixture"] = validated["fixture_receipt"]
                result[fixture] = receipt
    return result


def requirements(args) -> list[Requirement]:
    return [
        Requirement(args.backend_repository, "ci.yml", args.backend_sha, (
            "Plan & Preflight", "Rust Gate", "Resource & Contract Gate",
            "MySQL 8.4, Redis 7.4 & AWS-LC TLS Integration", "Windows Smoke",
            "Security, Supply Chain & Deployment", "Required",
        ), args.tag),
        Requirement(args.frontend_repository, "ci.yml", args.frontend_sha, (
            "Static Gate (Node 24)", "Unit Tests (Node 24)", "Production Build (Node 24)",
            "Browser Smoke (Node 24)", "Windows Smoke", "Required",
        ), args.tag),
        Requirement(args.backend_repository, "extended-ci.yml", args.backend_sha,
                    ("Real API MySQL Redis Chrome E2E", "Generated Device Data Migration E2E", "Linux DevEx Cgroup Memory"), args.tag),
        Requirement(args.frontend_repository, "extended-ci.yml", args.frontend_sha,
                    ("Node 22 Compatibility & Supply Chain",), args.tag),
    ]


def _remote_tag_ref(requirement: Requirement, expected_oid: str) -> None:
    endpoint = f"repos/{requirement.repository}/git/ref/tags/{requirement.tag}"
    reference = api(endpoint)
    target = reference.get("object") if isinstance(reference, dict) else None
    if (
        not isinstance(reference, dict)
        or reference.get("ref") != f"refs/tags/{requirement.tag}"
        or not isinstance(target, dict)
        or target.get("type") != "tag"
        or target.get("sha") != expected_oid
    ):
        raise EvidenceError(f"远端 annotated tag 已移动或对象身份错误：{requirement.repository}")


def _remote_tag_commit(requirement: Requirement, tag_oid: str) -> str:
    current = tag_oid
    visited = set()
    for _ in range(8):
        if current in visited:
            raise EvidenceError(f"远端 annotated tag 出现循环：{requirement.repository}")
        visited.add(current)
        value = api(f"repos/{requirement.repository}/git/tags/{current}")
        target = value.get("object") if isinstance(value, dict) else None
        if (
            not isinstance(value, dict)
            or value.get("sha") != current
            or not isinstance(target, dict)
        ):
            raise EvidenceError(f"远端 annotated tag 对象响应无效：{requirement.repository}")
        kind, target_sha = target.get("type"), target.get("sha")
        if not isinstance(target_sha, str) or not re.fullmatch(r"[0-9a-f]{40}", target_sha):
            raise EvidenceError(f"远端 annotated tag 目标无效：{requirement.repository}")
        if kind == "commit":
            return target_sha
        if kind != "tag":
            raise EvidenceError(f"远端 annotated tag 未指向提交：{requirement.repository}")
        current = target_sha
    raise EvidenceError(f"远端 annotated tag 解引用层数超限：{requirement.repository}")


def validate_remote_tags(
    required: list[Requirement], tag_oids: tuple[str, str]
) -> list[dict]:
    if (
        len(required) < 2
        or len(tag_oids) != 2
        or any(requirement.tag is None for requirement in required[:2])
        or any(
            not isinstance(oid, str) or not re.fullmatch(r"[0-9a-f]{40}", oid)
            for oid in tag_oids
        )
    ):
        raise EvidenceError("远端 tag 核验缺少精确的双方身份")
    targets = list(zip(required[:2], tag_oids, strict=True))
    for requirement, tag_oid in targets:
        _remote_tag_ref(requirement, tag_oid)
    result = []
    for requirement, tag_oid in targets:
        commit = _remote_tag_commit(requirement, tag_oid)
        if commit != requirement.sha:
            raise EvidenceError(f"远端 annotated tag 解引用提交错误：{requirement.repository}")
        result.append(
            {
                "repository": requirement.repository,
                "tag": requirement.tag,
                "tag_oid": tag_oid,
                "commit": commit,
            }
        )
    # 解引用期间任一仓库移动 tag，都不能使用先前的成功读取继续发布。
    for requirement, tag_oid in targets:
        _remote_tag_ref(requirement, tag_oid)
    return result


def await_evidence(required, timeout: float, collect_run=collect, sleep=time.sleep, clock=time.monotonic):
    deadline = clock() + timeout
    while True:
        evidence = [collect_run(item) for item in required]
        if all(item is not None for item in evidence):
            return evidence
        remaining = deadline - clock()
        if remaining <= 0:
            raise EvidenceError("等待 CI 超时：仍有缺失或未完成的运行")
        sleep(min(15, remaining))


def coordinated_evidence(
    required,
    timeout,
    read_pairs=pair_receipts,
    collect_run=collect,
    sleep=time.sleep,
    clock=time.monotonic,
    *,
    fixture_sha256=None,
    tag_oids=None,
    read_tags=validate_remote_tags,
):
    if fixture_sha256 is None:
        raise EvidenceError("缺少发布源码中的 Device fixture 摘要")
    deadline = clock() + timeout
    while True:
        evidence = await_evidence(required, max(0, deadline - clock()), collect_run, sleep, clock)
        receipts = read_pairs(
            evidence[2], required[0].sha, required[1].sha, fixture_sha256
        )
        if set(receipts) != {"core", "device"}:
            raise EvidenceError("全栈证据必须同时包含 core 和 device")
        for receipt in receipts.values():
            validate_pair(receipt, evidence[2], required[0].sha, required[1].sha)
        validate_fixture(
            receipts["device"].get("fixture"),
            required[0].sha,
            required[1].sha,
            fixture_sha256,
        )
        # 下载全栈产物期间，任何一个仓库开始重跑都必须重新等待整组证据。
        if [collect_run(item) for item in required] == evidence:
            result = {"runs": evidence, "source_pairs": receipts}
            if tag_oids is not None:
                result["remote_tags"] = read_tags(required, tag_oids)
            return result
        remaining = deadline - clock()
        if remaining <= 0:
            raise EvidenceError("源码组合复核期间 CI 发生变化，等待超时")
        sleep(min(15, remaining))


def main() -> None:
    global API_DEADLINE
    parser = argparse.ArgumentParser(description=__doc__)
    source_pair = parser.add_mutually_exclusive_group()
    source_pair.add_argument("--record-pair", type=Path)
    source_pair.add_argument("--verify-pair", type=Path)
    parser.add_argument("--backend-dir", type=Path, default=Path.cwd())
    parser.add_argument("--frontend-dir", type=Path)
    parser.add_argument("--backend-repository")
    parser.add_argument("--frontend-repository")
    parser.add_argument("--backend-sha")
    parser.add_argument("--frontend-sha")
    parser.add_argument("--backend-tag-oid")
    parser.add_argument("--frontend-tag-oid")
    parser.add_argument("--tag")
    parser.add_argument("--timeout", type=int, default=5400)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.record_pair or args.verify_pair:
        if args.frontend_dir is None:
            parser.error("记录或复核源码组合需要 --frontend-dir")
        run_id = int(os.environ["GITHUB_RUN_ID"])
        attempt = int(os.environ["GITHUB_RUN_ATTEMPT"])
        if args.verify_pair:
            verify_source_pair_receipt(
                args.verify_pair,
                args.backend_dir,
                args.frontend_dir,
                run_id,
                attempt,
            )
            print("全栈源码组合复核通过")
            return
        receipt = source_pair_receipt(
            args.backend_dir, args.frontend_dir, run_id, attempt
        )
        args.record_pair.parent.mkdir(parents=True, exist_ok=True)
        with args.record_pair.open("x", encoding="utf-8", newline="\n") as output:
            output.write(json.dumps(receipt, indent=2) + "\n")
        return
    for field in ("backend_sha", "frontend_sha"):
        if not re.fullmatch(r"[0-9a-f]{40}", getattr(args, field) or ""):
            parser.error(f"{field} 必须是精确 SHA")
    for field in ("backend_repository", "frontend_repository"):
        if not re.fullmatch(r"[\w.-]+/[\w.-]+", getattr(args, field) or ""):
            parser.error(f"{field} 必须是 owner/repo")
    tag_oids = (args.backend_tag_oid, args.frontend_tag_oid)
    if any(tag_oids) and not all(
        re.fullmatch(r"[0-9a-f]{40}", value or "") for value in tag_oids
    ):
        parser.error("backend_tag_oid 与 frontend_tag_oid 必须同时提供精确对象 OID")
    if not args.output or not re.fullmatch(r"v\d+\.\d+\.\d+", args.tag or "") or not 1 <= args.timeout <= 14400:
        parser.error("需要有效 --output、--tag 和 1..14400 秒超时")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    API_DEADLINE = time.monotonic() + args.timeout
    try:
        fixture = (
            Path(__file__).resolve().parents[1]
            / "crates/ryframe-generator/tests/fixtures/device.toml"
        )
        fixture_sha256 = sha256(fixture.read_bytes()).hexdigest()
        evidence = coordinated_evidence(
            requirements(args),
            args.timeout,
            fixture_sha256=fixture_sha256,
            tag_oids=tag_oids if all(tag_oids) else None,
        )
    except (EvidenceError, OSError, subprocess.SubprocessError, ValueError, KeyError, zipfile.BadZipFile) as error:
        failure = {
            "status": "failed",
            "error": str(error),
            "backend_sha": args.backend_sha,
            "frontend_sha": args.frontend_sha,
            "tag": args.tag,
        }
        if all(tag_oids):
            failure.update(
                backend_tag_oid=tag_oids[0], frontend_tag_oid=tag_oids[1]
            )
        args.output.write_text(
            json.dumps(failure, ensure_ascii=False, indent=2) + "\n",
            encoding="utf-8",
        )
        raise
    args.output.write_text(json.dumps({"status": "success", **evidence}, indent=2) + "\n", encoding="utf-8")
    print("双方日常 CI、Extended CI 和精确源码组合验证通过")


if __name__ == "__main__":
    main()
