"""C70 源代次因主机重启中断的只读核验；不启动、停止或重放任何真实资源。

观察只接受唯一已收尾的 C70 成功 START 且其后没有停止、恢复或其他源阶段，并核对
失败验收前缀仍然完整。进程代次是否退出只由主机启动证明判定，不按 PID 猜测；重启
丢失的缓存状态不会被记为清理成功，构建环境漂移只作为阻塞项如实报告。
"""

from __future__ import annotations

from pathlib import Path

from devex_clone_capture import read_json
from devex_clone_model import exact
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, require_closed_port
from host_boot import host_boot_proof, provably_exited, verify_host_boot_proof
from restore_reference_plan import plan_hash

START = "source-generation-start"
ROLES = ("api", "worker")
MEMBER_KEYS = ("process", "supervisor", "monitor")
PROOF_MAX_AGE_SECONDS = 1800


def start_record(backend: Path, directory: Path) -> dict:
    """唯一已收尾的 C70 成功 START，且其后没有停止、恢复或其他源阶段。"""
    from devex_clone_seed_generation_lineage_retry import completed

    attempts = load_state(directory)["attempts"]
    if any(row["status"] == "running" for row in attempts):
        raise ValueError("统一运行仍有未收尾阶段，必须先核对实际控制器")
    authority = completed(backend, directory, attempts)
    if authority is None:
        raise ValueError("缺少已收尾并绑定 C69 授权的唯一源代次 START")
    start = authority["completed_successor"]
    if start["status"] != "passed":
        raise ValueError("主机重启观察只接受最高源代次已经成功启动")
    later = [row for row in attempts if row["number"] > start["number"]]
    if any(row["stage"] == "seed-runtime" for row in later):
        raise ValueError("源代次之后存在停止、恢复或其他源阶段，不能按重启中断观察")
    return {"start": start, "later": later}


def generation(backend: Path, directory: Path, start: dict) -> dict:
    """读取 C70 外层收据、请求副本、启动意图与运行配置。"""
    from devex_clone_seed_generation import REQUEST_FIELDS, START_FIELDS

    start_path = bound_file(backend, start["result"])
    if start_path != directory / "results" / f"{start['number']:04d}.json":
        raise ValueError("源代次收据不属于固定账本位置")
    receipt = read_json(start_path)
    exact(receipt, START_FIELDS)
    if receipt["status"] != "seed_source_generation_running" or receipt["restore_qualified"] is not False:
        raise ValueError("源代次收据状态不是运行中的冻结来源")
    request = read_json(bound_file(backend, receipt["request"]))
    exact(request, REQUEST_FIELDS)
    output = directory / receipt["generation_id"]
    if output.name != f"g{start['number']:04d}" or read_json(output / "request.json") != request:
        raise ValueError("源代次目录、请求副本与账本不一致")
    intent = read_json(bound_file(backend, receipt["intent"]))
    exact(intent, {"format_version", "kind", "request", "runtime_directory", "operations"})
    if (read_json(bound_file(backend, intent["request"])) != request
            or intent["runtime_directory"] != str(output / "runtime")
            or set(intent["operations"]) != set(ROLES)):
        raise ValueError("源代次启动意图与收据或角色集合不一致")
    runtime = read_json(output / "runtime" / "runtime.json")
    return {"receipt": receipt, "request": request, "output": output, "intent": intent,
            "runtime": runtime}


def failed_prefix(directory: Path) -> dict:
    """被中断的失败验收前缀：只接受精确文件集合与已知失败记录。"""
    from restore_source_runtime import FAILED_ROOT_ENTRIES, _producer_bindings

    names = frozenset(entry.name for entry in directory.iterdir())
    if names != frozenset(FAILED_ROOT_ENTRIES):
        raise ValueError("来源验收失败前缀不是未清理、未采集后像的精确集合")
    failure = read_json(directory / "failed.json")
    exact(failure, {"status", "error"})
    if failure != {"status": "failed", "error": "ValueError"}:
        raise ValueError("来源验收失败前缀不是已知的缓存阶段失败")
    return {"directory": str(directory), "failed": binding(directory / "failed.json"),
            "producer": _producer_bindings(directory),
            "audit": binding(directory / "audit/login-audit.json"),
            "before": binding(directory / "before/image.json"),
            "manifest": sorted(names)}


def role_evidence(tree: dict, proof: dict) -> dict:
    """按主机启动证明核对单个角色的全部创建身份；缺项按未证明处理。"""
    members = {key: {"identity": tree[key], "exited": provably_exited(tree[key], proof)}
               for key in MEMBER_KEYS}
    return {"operation_id": tree["operation_id"], "members": members,
            "all_exited": all(item["exited"] for item in members.values())}


def port_evidence(urls: list[str]) -> list[dict]:
    """精确端口观察：只把明确关闭记为关闭，异常只记录类型。"""
    rows = []
    for url in urls:
        try:
            require_closed_port(url)
            rows.append({"url": url, "closed": True})
        except ValueError as error:
            rows.append({"url": url, "closed": False, "error_type": type(error).__name__})
    return rows


def completion_receipt(runtime: Path, role: str, operation_id: str) -> dict | None:
    """已存在的成员关闭收据只作证据登记，不替代主机启动证明。"""
    path = runtime / f"{role}-members-{operation_id}-stopped.json"
    return binding(path) if path.is_file() else None


def role_urls(backend: Path, request: dict, contract: dict) -> dict:
    """从冻结私有环境推导 API 与 Worker 的精确端点。"""
    from devex_clone_source_proof import load_app_table

    private = read_json(bound_file(backend, request["source_environment"]))["environment"]
    app = load_app_table(backend, private)["app"]
    host = private.get("APP_APP_HOST", app.get("host"))
    port = private.get("APP_APP_PORT", app.get("port"))
    return {"api": f"http://{host}:{port}", "worker": contract["worker_ready_url"]}


def roles(backend: Path, facts: dict, proof: dict) -> tuple[dict, list[str]]:
    """按启动意图核对双角色进程树与端点，并登记已有成员关闭收据。"""
    runtime = facts["output"] / "runtime"
    urls = role_urls(backend, facts["request"], facts["runtime"])
    observed, blockers = {}, []
    for role in ROLES:
        tree = read_json(runtime / f"{role}-tree.json")
        if (tree["operation_id"] != facts["intent"]["operations"][role]
                or tree["scope_id"] != facts["runtime"]["scope_id"]):
            raise ValueError(f"{role} 进程树不属于 C70 启动意图")
        ports = port_evidence([urls[role]])
        evidence = {**role_evidence(tree, proof), "ports": ports,
                    "completion": completion_receipt(runtime, role, tree["operation_id"])}
        observed[role] = evidence
        if not evidence["all_exited"]:
            blockers.append(f"{role} 代次无法证明已随主机重启退出")
        if not all(row["closed"] for row in ports):
            blockers.append(f"{role} 端点没有明确关闭")
    return observed, blockers


def storage_generation(facts: dict, proof: dict) -> tuple[dict, list[str]]:
    """C70 冻结的对象存储代次：证明身份已经退出且端点关闭。"""
    storage = facts["request"]["current_storage"]
    identity = storage["storage"]["identity"]
    exited = provably_exited(identity, proof)
    ports = port_evidence([storage["api_url"], storage["console_url"]])
    blockers = []
    if not exited:
        blockers.append("对象存储代次无法证明已随主机重启退出")
    if not all(row["closed"] for row in ports):
        blockers.append("对象存储端点没有明确关闭")
    return {"attempt": storage["attempt"], "identity": identity, "exited": exited,
            "request": storage["request"], "ports": ports}, blockers


def cache_generations(backend: Path, directory: Path) -> tuple[dict, list[str]]:
    """复用只读缓存状态；内核启动标识变化才证明重启前代次已经退出。"""
    from devex_clone_cache import cache_status

    try:
        status = cache_status(backend, directory)
    except ValueError as error:
        return {"status": "error", "error_type": type(error).__name__}, ["缓存代次无法完成只读观察"]
    observed, blockers = [], []
    for row in status.get("processes", []):
        linux = row["process"].get("linux", {})
        recorded, current = linux.get("identity", {}).get("boot_id"), linux.get("boot_id")
        changed = bool(recorded) and bool(current) and recorded != current
        observed.append({"attempt": row["attempt"], "state": row["process"]["state"],
                         "linux_alive": linux.get("alive"), "recorded_boot_id": recorded,
                         "current_boot_id": current, "boot_id_changed": changed})
        if row["process"]["state"] != "stopped" or linux.get("alive") is not False or not changed:
            blockers.append(f"缓存代次 {row['attempt']} 没有明确退出证据")
    if not observed:
        blockers.append("缓存登记为空，无法核对重启前代次")
    return {"status": status["status"], "request": status["request"], "generations": observed}, blockers


def build_context_state(backend: Path, facts: dict) -> dict:
    """如实报告构建环境漂移：漂移不是重启事实，但会阻塞后续收口。"""
    from devex_clone_tools import verify_evidence
    from restore_build import build_context

    execution = Path(facts["request"]["execution_backend"])
    expected = facts["request"]["expected_backend_sha"]
    observed = {}
    for name, field, root in (("backend", "backend_build", backend),
                              ("maintenance", "maintenance_build", execution)):
        path = bound_file(root, facts["request"][field])
        try:
            receipt = read_json(path) if name == "backend" else verify_evidence(execution, path)
            matches = (receipt["build"] == build_context(execution) if name == "backend"
                       else receipt["source"]["snapshot"]["head"] == expected
                       and receipt["backend_root"] == str(execution))
            observed[name] = {"matches": matches, "evidence": binding(path)}
        except ValueError as error:
            observed[name] = {"matches": False, "error_type": type(error).__name__, "evidence": binding(path)}
    return observed


def observe(backend: Path, directory: Path) -> dict:
    """完整只读观察：绑定重启中断现场并列出继续收口前的阻塞项。"""
    directory = directory.resolve()
    tail = start_record(backend, directory)
    facts = generation(backend, directory, tail["start"])
    prefix = failed_prefix(facts["output"] / "verification")
    proof = host_boot_proof()
    verify_host_boot_proof(proof, max_age_seconds=PROOF_MAX_AGE_SECONDS)
    role_facts, blockers = roles(backend, facts, proof)
    storage, storage_blockers = storage_generation(facts, proof)
    cache, cache_blockers = cache_generations(backend, directory)
    blockers += storage_blockers + cache_blockers
    build = build_context_state(backend, facts)
    if not all(row["matches"] for row in build.values()):
        blockers.append("构建环境与登记收据不一致，收口前必须成对重新登记构建收据")
    history = [row for row in load_state(directory)["attempts"] if row["number"] < tail["start"]["number"]]
    return {"status": "source_generation_reboot_observed", "attempt": tail["start"]["number"],
            "start": tail["start"]["result"], "history_length": len(history),
            "history_sha256": plan_hash(history),
            "service_attempts": [row["number"] for row in tail["later"]],
            "verification": prefix, "reboot": proof, "roles": role_facts,
            "storage": storage, "cache": cache,
            "build_context": build,
            "blockers": blockers, "closure_ready": not blockers,
            "remote_writes": 0, "restore_qualified": False}
