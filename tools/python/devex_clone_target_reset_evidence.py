"""fresh reset 最终账本、直接前像与报告证据。"""
from pathlib import Path
import hashlib
import json
import re

from devex_clone import read_json
from devex_clone_capture import unique_object
from devex_clone_model import exact, linked
from devex_clone_run_state import binding
from restore_build import file_digest

PHASES = {"preflight", "object_storage", "redis", "databases", "control_baseline",
          "tenant_baselines", "verification", "release"}
COMMAND_FIELDS = {"command", "returncode", "error_type", "stdout", "stderr"}


def validate_plan_receipt(path: Path, executable: str, plan: dict) -> dict:
    """将 reset plan 命令的脱敏全文重新绑定到原 manifest 与摘要。"""
    value = read_json(path)
    exact(value, COMMAND_FIELDS)
    stdout = value["stdout"].replace("\r\n", "\n") if isinstance(value["stdout"], str) else ""
    try:
        document, digest_line = stdout.rsplit("\nplan_hash=", 1)
        parsed = json.loads(document, object_pairs_hook=unique_object)
    except (json.JSONDecodeError, ValueError) as error:
        raise ValueError("reset plan 命令输出不是完整清单与摘要") from error
    expected = json.loads(json.dumps(plan["manifest"]))
    expected["credential_version"] = "[REDACTED]"
    restored, replacements = re.subn(
        r'("credential_version"\s*:\s*)"\[REDACTED\]"',
        lambda match: match.group(1) + json.dumps(
            plan["manifest"]["credential_version"], ensure_ascii=False), document)
    if (value["command"] != [executable, "plan"] or type(value["returncode"]) is not int
            or value["returncode"] != 0 or value["error_type"] is not None or value["stderr"] != ""
            or not digest_line.endswith("\n") or digest_line[:-1] != plan["plan_hash"]
            or parsed != expected or replacements != 1
            or hashlib.sha256(restored.encode()).hexdigest() != digest_line[:-1]):
        raise ValueError("reset plan 命令收据与固定清单不一致")
    return binding(path)

def reset_completed(output: Path, manifest: dict, sha: str) -> dict:
    stem = f"test-{manifest['scope_id']}-{sha}"
    directory = output / "reset-state"
    report_path, ledger_path = directory / (stem + ".report.json"), directory / (stem + ".ledger.json")
    file_digest(report_path)
    file_digest(ledger_path)
    report, ledger = read_json(report_path), read_json(ledger_path)
    identity = {key: manifest[key] for key in ("environment", "scope_id", "code_sha", "config_sha", "credential_version")}
    for value in (report, ledger):
        if value.get("plan_hash") != sha or any(value.get(key) != item for key, item in identity.items()):
            raise ValueError("reset 报告或账本不属于当前精确 plan")
        if set(value["phases"]) != PHASES or any(item["status"] != "complete" or not item["completed_at"] for item in value["phases"].values()):
            raise ValueError("reset 有未完成阶段或锁释放证据缺失")
        if not value["resources"] or any(item["status"] != "complete" for item in value["resources"].values()):
            raise ValueError("reset 存在未确认资源")
    if (report.get("report_version") != 2 or ledger.get("ledger_version") != 4
            or report.get("status") != "completed" or report.get("failed_phase") is not None
            or report.get("completed_at") != ledger["phases"]["release"]["completed_at"]
            or report["phases"] != ledger["phases"] or report["resources"] != ledger["resources"]):
        raise ValueError("fresh 初始化必须本次 completed；reused、失败或释放中断均拒绝")
    previous_path = directory / ("." + ledger_path.name + ".previous")
    if linked(previous_path):
        raise ValueError("reset 上一版账本不能经过链接")
    file_digest(previous_path)
    previous = read_json(previous_path)
    if (set(previous) != set(ledger)
            or any(previous[key] != ledger[key] for key in set(ledger) - {"updated_at", "phases"})
            or not isinstance(previous.get("updated_at"), str)
            or previous["updated_at"] > ledger["updated_at"]
            or set(previous["phases"]) != PHASES
            or any(previous["phases"][key] != ledger["phases"][key]
                   for key in PHASES - {"release"})):
        raise ValueError("reset 上一版账本不属于最终账本的直接前像")
    prior_release, release = previous["phases"]["release"], ledger["phases"]["release"]
    if prior_release != {**release, "status": "running", "completed_at": None}:
        raise ValueError("reset 上一版账本没有证明最终 release 的直接前像")
    if set(report["databases"]) != {f"{db['host']}:{db['port']}/{db['database']}" for db in manifest["databases"]}:
        raise ValueError("reset 完成报告数据库集合不符")
    if (report["redis_namespace"] != manifest["redis"]["namespace"]
            or set(report["object_prefixes"]) != {item["bucket"] + ":" + item["prefix"] for item in manifest["object_storage"]["prefixes"]}):
        raise ValueError("reset 完成报告 Redis 或对象集合不符")
    return {"report": {"file": str(report_path.relative_to(output)), **file_digest(report_path)},
            "ledger": {"file": str(ledger_path.relative_to(output)), **file_digest(ledger_path)},
            "completed_at": report["completed_at"], "status": "completed"}
