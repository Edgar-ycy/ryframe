"""seed 十租户固定根部门的计划、显式写入、核对和只读验证。"""
from types import SimpleNamespace
import re
import time
from uuid import uuid4

from devex_clone_capture import read_json, write_json
from devex_clone_model import digest, exact, linked
from devex_clone_post_actions import Bridge
from devex_clone_post_process import cleanup_failure, inspect_producer
from devex_clone_run_state import binding, load_state
from devex_clone_seed import identity_inputs, latest
from devex_clone_source_proof import bound_file
from full_stack_process import process_identity
from restore_reference_plan import BUCKETS, plan_hash
from devex_clone_department_model import (authorization, classify, confirmation, created_response,
    directories, identity_plan, identity_projection, image, intent, page_data, plan_path,
    prerequisite_path, principal, read_prerequisite, reconciliation, require_prerequisite, snowflake,
    targets, template_identities, validate_plan)


SESSION_CLOSE_SECONDS = 10
CLOSE_MARGIN_SECONDS = 10


def guard(context, request, prerequisite=None, *, quick=False):
    from devex_clone_seed_runtime import current_guard, current_mutation_guard

    if quick:
        if prerequisite is None:
            raise ValueError("部门写入临界区守卫缺少身份前提")
        confirmations = getattr(context, "_department_schedule_confirmations", None)
        if confirmations is None:
            raise ValueError("部门写入临界区缺少本阶段完整守卫结果")
        current_mutation_guard(context, request, prerequisite, confirmations)
    else:
        context._department_schedule_confirmations = current_guard(context, request, old_api=True)
    if prerequisite is not None:
        require_prerequisite(context.backend, context.directory_root, request, prerequisite)


def observe(bridge, action, target):
    listed = bridge.call("list", tenant_id=target["tenant_id"], name=action["body"]["name"])
    authorization(target, listed["authorization"])
    data = page_data(listed["response"])
    detail = None
    desired = [item for item in data["items"] if item.get("name") == action["body"]["name"]]
    if len(desired) == 1:
        identifier = snowflake(desired[0].get("id"))
        detail = bridge.call("detail", tenant_id=target["tenant_id"], department_id=identifier)
    template = bridge.call("template", tenant_id=target["tenant_id"])
    observed = {"list": listed, "detail": detail, "template": template}
    image(action, observed, target)
    return observed


def prerequisite(context, request):
    root = context.directory_root / "seed-runtime/departments"
    path = prerequisite_path(context.directory_root)
    current = identity_projection(context.backend, request, allow_absent=True)
    if current["state"] == "verified":
        raise ValueError("身份已验证后不再补建部门计划")
    value = {"format_version": 1, "kind": "devex-seed-identity-prerequisite",
             "registration": binding(context.directory_root / "seed-runtime.json"), "identity": current}
    if root.exists():
        if linked(root) or not root.is_dir() or not path.exists() or read_json(path) != value:
            raise ValueError("已有部门证据目录或身份前提不同")
    else:
        root.mkdir()
        write_json(path, value)
    return read_prerequisite(context.backend, context.directory_root)


def create_plan(context, request, bridge):
    prior = prerequisite(context, request)
    source = identity_plan(context.backend, request)
    templates = template_identities(context.backend, request)
    actions = []
    for target, template_identity in zip(targets(source), templates):
        action = {"target": {key: target[key] for key in ("slot", "tenant_id")},
                  "body": target["body"], "expected": target["expected"],
                  "before": None, "before_evidence": None, "before_template_paths": None,
                  "principal": None, "template_principal": principal(template_identity)}
        guard(context, request, prior)
        observed = observe(bridge, action, target)
        filename = context.output / (target["slot"] + "-before.json")
        write_json(filename, observed)
        current = image(action, observed, target)
        if (current["department"] is not None
                or action["body"]["name"] in current["template"]["paths"]):
            raise ValueError("固定 seed 部门名称已经存在或模板状态矛盾")
        action["before"] = current["directory"]
        action["before_evidence"] = binding(filename)
        action["before_template_paths"] = current["template"]["paths"]
        action["principal"] = current["principal"]
        actions.append(action)
    guard(context, request, prior)
    plan = {"format_version": 1, "kind": "devex-seed-departments",
            "registration": binding(context.directory_root / "seed-runtime.json"),
            "identity_plan": request["identity_plan"], "prerequisite": binding(prerequisite_path(context.directory_root)),
            "actions": actions}
    plan["plan_sha256"] = plan_hash(plan)
    filename = plan_path(context.directory_root)
    if filename.exists() or linked(filename):
        if linked(filename) or read_json(filename) != plan:
            raise ValueError("已有部门计划不同，不能覆盖或重新规划")
    else:
        write_json(filename, plan)
    validate_plan(context.backend, context.directory_root, request)
    return {"status": "seed_departments_planned", "plan": binding(filename),
            "prerequisite": binding(prerequisite_path(context.directory_root)), "remote_writes": 0,
            "worker_authorized": False, "restore_qualified": False}


def close_timeout(plan):
    """为十个 owner、十个模板会话及 pacing 关闭保留有限预算。"""
    return SESSION_CLOSE_SECONDS * (2 * len(targets(plan)) + 1) + CLOSE_MARGIN_SECONDS


def _close_rpc_timeout(plan, deadline):
    remaining = deadline - time.monotonic() - CLOSE_MARGIN_SECONDS
    if remaining <= 0:
        raise TimeoutError("部门会话关闭已没有安全回收预算")
    return min(SESSION_CLOSE_SECONDS * (2 * len(targets(plan)) + 1), remaining)


def _fingerprints(attempt):
    sources = attempt["sources"]
    exact(sources, {"snapshot", "worktree_fingerprint", "fingerprints"})
    fingerprints = sources["fingerprints"]
    exact(fingerprints, {"product", "test_tools", "support"})
    for value in fingerprints.values():
        exact(value, {"sha256", "files"})
        digest(value["sha256"])
        if type(value["files"]) is not int or value["files"] <= 0:
            raise ValueError("部门计划采用的执行来源摘要无效")
    return fingerprints


def _original_process_stopped(identity):
    actual = process_identity(identity["pid"])
    if actual is None:
        return
    if (not isinstance(actual, dict) or set(actual) != {"pid", "started", "executable"}
            or actual["pid"] != identity["pid"] or not isinstance(actual["started"], str)
            or not actual["started"].isdigit() or not isinstance(identity["started"], str)
            or not identity["started"].isdigit() or int(actual["started"]) <= int(identity["started"])):
        raise ValueError("原部门控制进程仍存在或内核身份不明")


def _controller(context, attempt, *, current):
    number = attempt["number"]
    path = context.directory_root / f"controller-{number:04d}.json"
    descriptor = binding(path)
    value = read_json(bound_file(context.backend, descriptor))
    exact(value, {"format_version", "kind", "owner", "attempt", "attempt_sha256"})
    owner = value["owner"]
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    identity = owner["identity"]
    exact(identity, {"pid", "started", "executable"})
    original = ({**attempt, "status": "running", "finished_at": None,
                 "result": None, "error_type": None})
    if (value["format_version"] != 1 or value["kind"] != "devex-stage-controller"
            or value["attempt"] != number or value["attempt_sha256"] != plan_hash(original)
            or owner["format_version"] != 1 or owner["directory"] != str(context.directory_root)
            or owner["manifest_sha256"] != binding(context.directory_root / "manifest.json")["sha256"]
            or type(identity["pid"]) is not int or identity["pid"] <= 0
            or not isinstance(identity["started"], str) or not identity["started"].isdigit()
            or not isinstance(identity["executable"], str) or not identity["executable"]):
        raise ValueError("部门计划控制器未绑定原始 attempt 和固定 manifest")
    if current:
        lock_owner_path = context.directory_root / "run.lock/owner.json"
        if (read_json(bound_file(context.backend, binding(lock_owner_path))) != owner
                or process_identity(identity["pid"]) != identity):
            raise ValueError("当前部门计划采用未绑定存活持锁控制器")
    else:
        _original_process_stopped(identity)
    if binding(path) != descriptor:
        raise ValueError("部门计划控制器在采用期间变化")
    return descriptor, value


def _failure(context, candidate, controller, controller_value):
    number = candidate["number"]
    path = context.directory_root / f"failure-{number:04d}.json"
    descriptor = binding(path)
    value = read_json(bound_file(context.backend, descriptor))
    exact(value, {"format_version", "kind", "attempt", "stage", "mode", "error_type", "frames", "controller"})
    frames = value["frames"]
    if (value["format_version"] != 1 or value["kind"] != "devex-stage-failure"
            or value["attempt"] != number or value["stage"] != "seed-runtime"
            or value["mode"] != "departments-plan" or value["error_type"] != candidate["error_type"]
            or value["controller"] != controller
            or not isinstance(frames, list) or not frames):
        raise ValueError("原部门计划失败收据不完整或不属于候选 attempt")
    for frame in frames:
        exact(frame, {"file", "function", "line"})
        if (not isinstance(frame["file"], str) or not frame["file"].startswith("scripts/")
                or not isinstance(frame["function"], str) or not frame["function"]
                or type(frame["line"]) is not int or frame["line"] <= 0):
            raise ValueError("原部门计划失败栈结构无效")
    if not any(frame["file"] == "scripts/devex_clone_run.py" and frame["function"] == "execute"
               for frame in frames):
        raise ValueError("原部门计划失败未绑定统一阶段执行边界")
    if read_json(bound_file(context.backend, value["controller"])) != controller_value:
        raise ValueError("部门计划失败收据与控制器不同")
    if binding(path) != descriptor:
        raise ValueError("原部门计划失败收据在采用期间变化")
    return descriptor


def _pre_dispatch_failure(context, attempt):
    """只跳过在部门 Context/Bridge 创建前由 quiet gate 终止的空 attempt。"""
    if attempt["result"] is not None:
        return False
    output = context.directory_root / "seed-runtime" / f"attempt-{attempt['number']:04d}"
    if linked(output):
        raise ValueError("部门计划预检失败目录是链接")
    if output.exists():
        return False
    controller, controller_value = _controller(context, attempt, current=False)
    descriptor = _failure(context, attempt, controller, controller_value)
    failure = read_json(bound_file(context.backend, descriptor))
    frames = [(item["file"], item["function"]) for item in failure["frames"]]
    expected = [("scripts/devex_clone_run.py", "execute"),
                ("scripts/devex_clone_seed_runtime.py", "execute_seed"),
                ("scripts/devex_clone_post_process.py", "require_quiet")]
    if frames[:len(expected)] != expected:
        raise ValueError("无结果部门计划失败不是 Bridge 前 quiet gate 预检")
    return True


def _adoption_result(context, attempt):
    if attempt["result"] is None:
        raise ValueError("部门计划采用链节点缺少外层结果")
    path = bound_file(context.backend, attempt["result"])
    value = read_json(path)
    exact(value, {"status", "plan", "prerequisite", "adopted", "remote_writes",
                  "worker_authorized", "restore_qualified"})
    if not isinstance(value["adopted"], dict):
        raise ValueError("部门计划采用链节点结果无效")
    exact(value["adopted"], {"attempt", "failure", "controller", "current_controller",
          "producer", "authentication", "plan", "prerequisite", "before",
          "original_fingerprints", "current_fingerprints", "cleanup", "guard"})
    if (value["status"] != "seed_departments_planned" or value["remote_writes"] != 0
            or value["worker_authorized"] is not False or value["restore_qualified"] is not False
            or binding(path) != attempt["result"]
            or _empty_adoption_output(context, attempt) != value["adopted"]["guard"]):
        raise ValueError("部门计划采用链节点未发布精确只读结果")
    return value


_GUARD_COMMAND = re.compile(r"(?:command-[a-f0-9]{32}|redis-kernel-[a-f0-9]{32}\.command)\.json")
_GUARD_MYSQL = re.compile(r"mysql-(identity|ownership)-([a-f0-9]{32})\.(json|stdout)")
_GUARD_MYSQL_RETRY = re.compile(r"mysql-(identity|ownership)-retry-[a-f0-9]{32}\.json")
_GUARD_OWNER = re.compile(r"owner-target-([a-z0-9-]+)-[a-f0-9]{32}\.txt")
_GUARD_LAYOUT = re.compile(r"storage-layout-[a-f0-9]{32}\.json")


def _read_only_command(value):
    exact(value, {"command", "returncode", "error_type", "stdout", "stderr"}
          | ({"stdout_file"} if "stdout_file" in value else set()))
    command = value["command"]
    if (not isinstance(command, list) or not command
            or any(not isinstance(item, str) or not item for item in command)
            or value["returncode"] != 0 or value["error_type"] is not None):
        raise ValueError("部门采用守卫命令收据不是成功的固定只读调用")
    executable = command[0].replace("\\", "/").rsplit("/", 1)[-1].lower()
    if executable == "mysql.exe":
        if "--execute" in command:
            index = command.index("--execute")
            if index + 1 >= len(command) or not command[index + 1].lstrip().upper().startswith("SELECT "):
                raise ValueError("部门采用守卫包含非 SELECT MySQL 命令")
    elif executable == "aws.exe":
        if "s3api" not in command:
            raise ValueError("部门采用守卫对象命令缺少 s3api")
        operation = command[command.index("s3api") + 1]
        if operation not in {"head-bucket", "get-bucket-versioning", "list-objects-v2", "get-object"}:
            raise ValueError("部门采用守卫包含对象写命令")
    elif executable == "powershell.exe":
        script = command[-1]
        if ("Get-CimInstance Win32_Process" not in script
                or not script.endswith(".CommandLine | ConvertTo-Json -Compress")):
            raise ValueError("部门采用守卫包含未知 PowerShell 命令")
    elif executable == "wsl.exe":
        if not any(item in {"/usr/bin/cat", "/usr/bin/readlink", "/usr/bin/sha256sum"}
                   for item in command):
            raise ValueError("部门采用守卫包含未知 WSL 命令")
    elif (executable, tuple(command[1:])) not in {("cargo", ("-V",)), ("rustc", ("-Vv",))}:
        raise ValueError("部门采用守卫包含未知外部命令")
    return value


def _guard_artifacts(context, output):
    if linked(output) or not output.is_dir():
        raise ValueError("部门计划采用守卫目录无效")
    artifacts, payloads, referenced = [], set(), set()
    for path in sorted(output.iterdir(), key=lambda item: item.name):
        if path.name == "adoption-guard.json":
            continue
        if linked(path) or not path.is_file():
            raise ValueError("部门计划采用守卫出现目录、链接或未知证据")
        command_match = _GUARD_COMMAND.fullmatch(path.name)
        mysql_match = _GUARD_MYSQL.fullmatch(path.name)
        mysql_retry_match = _GUARD_MYSQL_RETRY.fullmatch(path.name)
        owner_match = _GUARD_OWNER.fullmatch(path.name)
        if command_match:
            value = _read_only_command(read_json(path))
            if "stdout_file" in value:
                stdout = bound_file(context.backend, value["stdout_file"])
                if stdout.parent != output:
                    raise ValueError("部门采用守卫 stdout 跨出当前 attempt")
                referenced.add(stdout)
            for argument in value["command"]:
                candidate = type(path)(argument)
                if candidate.is_absolute() and candidate.parent == output:
                    referenced.add(candidate)
        elif mysql_match and mysql_match.group(3) == "json":
            value = read_json(path)
            exact(value, {"format_version", "kind", "check", "target_key", "returncode",
                          "error_type", "stderr_bytes", "stderr_sha256", "stdout_file"})
            stdout = path.with_suffix(".stdout")
            if (value["format_version"] != 1 or value["kind"] != "mysql-verification-output"
                    or value["check"] != mysql_match.group(1) or value["returncode"] != 0
                    or value["error_type"] is not None or value["stderr_bytes"] != 0
                    or value["stderr_sha256"] != "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    or bound_file(context.backend, value["stdout_file"]) != stdout):
                raise ValueError("部门采用守卫 MySQL 核验收据无效")
            referenced.add(stdout)
        elif mysql_match and mysql_match.group(3) == "stdout":
            payloads.add(path)
        elif mysql_retry_match:
            retry = read_json(path)
            exact(retry, {"format_version", "kind", "check", "target_key",
                          "reason", "first", "second"})
            if (retry["format_version"] != 1 or retry["kind"] != "mysql-verification-retry"
                    or retry["check"] != mysql_retry_match.group(1)
                    or retry["reason"] != "first_returncode_zero_stdout_stderr_exactly_empty"):
                raise ValueError("部门采用守卫 MySQL 空输出重试收据无效")
            for descriptor in (retry["first"], retry["second"]):
                receipt = bound_file(context.backend, descriptor)
                if receipt.parent != output or not _GUARD_MYSQL.fullmatch(receipt.name):
                    raise ValueError("部门采用守卫 MySQL 重试跨出当前 attempt")
        elif owner_match:
            bucket = owner_match.group(1)
            if bucket not in BUCKETS:
                raise ValueError("部门采用守卫对象 owner 桶无效")
            expected = f"ryframe-owner:v1:{context.target_config['scope_id']}:object-storage:{bucket}".encode()
            if path.read_bytes() != expected:
                raise ValueError("部门采用守卫对象 owner 内容无效")
            payloads.add(path)
        elif _GUARD_LAYOUT.fullmatch(path.name):
            layout = read_json(path)
            exact(layout, {"actual_arguments_sha256", "data_dir", "environment_proof",
                           "launch_receipt", "process_receipt", "runtime_transition"})
            bound_file(context.backend, layout["launch_receipt"])
            bound_file(context.backend, layout["process_receipt"])
        else:
            raise ValueError("部门计划采用链节点出现 Node、动作或未知证据")
        artifacts.append(binding(path))
    if payloads != referenced:
        raise ValueError("部门采用守卫本地输出没有被唯一只读命令收据覆盖")
    return artifacts


def _empty_adoption_output(context, attempt):
    output = context.directory_root / "seed-runtime" / f"attempt-{attempt['number']:04d}"
    artifacts = _guard_artifacts(context, output)
    manifest_path = output / "adoption-guard.json"
    if not manifest_path.exists() and not linked(manifest_path):
        return None
    manifest = read_json(bound_file(context.backend, binding(manifest_path)))
    exact(manifest, {"format_version", "kind", "attempt", "artifacts"})
    if (manifest["format_version"] != 1 or manifest["kind"] != "devex-department-adoption-guard"
            or manifest["attempt"] != attempt["number"] or manifest["artifacts"] != artifacts):
        raise ValueError("部门计划采用守卫清单与当前只读证据不同")
    return binding(manifest_path)


def _publish_adoption_guard(context, attempt):
    if _empty_adoption_output(context, attempt) is not None:
        raise ValueError("部门计划采用守卫清单已经存在")
    output = context.directory_root / "seed-runtime" / f"attempt-{attempt['number']:04d}"
    write_json(output / "adoption-guard.json", {"format_version": 1,
        "kind": "devex-department-adoption-guard", "attempt": attempt["number"],
        "artifacts": _guard_artifacts(context, output)})
    return _empty_adoption_output(context, attempt)


def _guard_only_failure(context, attempt):
    output = context.directory_root / "seed-runtime" / f"attempt-{attempt['number']:04d}"
    if not output.exists() or linked(output):
        return False
    controller, controller_value = _controller(context, attempt, current=False)
    descriptor = _failure(context, attempt, controller, controller_value)
    failure = read_json(bound_file(context.backend, descriptor))
    frames = [(item["file"], item["function"]) for item in failure["frames"]]
    expected = [("scripts/devex_clone_run.py", "execute"),
                ("scripts/devex_clone_seed_runtime.py", "execute_seed"),
                ("scripts/devex_clone_department.py", "execute_departments"),
                ("scripts/devex_clone_department.py", "adopt_failed_plan"),
                ("scripts/devex_clone_department.py", "_adoption_evidence"),
                ("scripts/devex_clone_department.py", "_empty_adoption_output")]
    if frames[:len(expected)] != expected:
        return False
    if _empty_adoption_output(context, attempt) is not None:
        raise ValueError("无结果部门计划失败不是采用前完整只读守卫失败")
    if not _guard_artifacts(context, output):
        raise ValueError("部门计划只读守卫失败缺少外部核验收据")
    return True


def _skipped_failure(context, attempt):
    return _pre_dispatch_failure(context, attempt) or _guard_only_failure(context, attempt)


def _adoption_evidence(context, request, state):
    current = state["attempts"][-1]
    failed = []
    for attempt in reversed(state["attempts"][:-1]):
        if (attempt["stage"], attempt["mode"], attempt["status"]) != (
                "seed-runtime", "departments-plan", "failed"):
            break
        failed.append(attempt)
    failed.reverse()
    if not failed:
        return None, None
    tip, skipped = failed[-1], []
    while tip["result"] is None and _skipped_failure(context, tip):
        skipped.append(failed.pop())
        if not failed:
            raise ValueError("部门计划采用链只有空预检失败，没有可采用原始计划")
        tip = failed[-1]
    tip_result = None if tip["result"] is None else _adoption_result(context, tip)
    origin_number = tip["number"] if tip_result is None else tip_result["adopted"]["attempt"]
    if type(origin_number) is not int:
        raise ValueError("部门计划采用链未绑定唯一原始 attempt")
    lineage = [item for item in failed if item["number"] >= origin_number]
    if (not lineage or lineage[0]["number"] != origin_number or lineage[0]["result"] is not None
            or [item["number"] for item in lineage] != list(range(origin_number, tip["number"] + 1))):
        raise ValueError("部门计划采用链不能连续追溯唯一原始 attempt")
    current_controller, _ = _controller(context, current, current=True)
    candidate = lineage[0]
    candidate_fingerprints = _fingerprints(candidate)
    if any(_fingerprints(item)["product"] != candidate_fingerprints["product"]
           for item in skipped):
        raise ValueError("部门计划预检失败链包含不同产品构建来源")
    candidate_controller, candidate_controller_value = _controller(context, candidate, current=False)
    observed = inspect_producer(context.backend, context.directory_root, candidate)
    if (observed is None or observed["alive"] or observed.get("producer_kind") != "department-plan"
            or observed.get("request") != request):
        raise ValueError("原部门计划 Node 未确认退出或来源登记不同")
    if observed["controller"] != candidate_controller_value:
        raise ValueError("原部门计划生产者与失败控制器不同")
    failure = _failure(context, candidate, candidate_controller, candidate_controller_value)
    prior = read_prerequisite(context.backend, context.directory_root)
    require_prerequisite(context.backend, context.directory_root, request, prior)
    plan_descriptor = binding(plan_path(context.directory_root))
    prerequisite_descriptor = binding(prerequisite_path(context.directory_root))
    plan = validate_plan(context.backend, context.directory_root, request)
    output = context.directory_root / "seed-runtime" / f"attempt-{candidate['number']:04d}"
    before = []
    for action in plan["actions"]:
        path = bound_file(context.backend, action["before_evidence"])
        if path != output / f"{action['target']['slot']}-before.json":
            raise ValueError("部门计划前像跨 attempt 或租户槽位")
        before.append(action["before_evidence"])
    root = prerequisite_path(context.directory_root).parent
    if (linked(root) or not root.is_dir()
            or {item.name for item in root.iterdir()} != {prerequisite_path(context.directory_root).name}):
        raise ValueError("原部门计划后出现部门动作目录或未知写入证据")
    source = identity_plan(context.backend, request)
    source_targets = targets(source)
    templates = template_identities(context.backend, request)
    authentication_path = output / "authentication.json"
    authentication_descriptor = binding(authentication_path)
    expected_authentication = {"scope_id": source["environment"]["scope_id"],
        "tenant_ids": [item["tenant_id"] for item in source_targets],
        "department_name": source_targets[0]["body"]["name"],
        "template_principals": [principal(item) for item in templates]}
    if read_json(bound_file(context.backend, authentication_descriptor)) != expected_authentication:
        raise ValueError("原部门计划初始化未绑定当前十租户和模板主体")
    cleanup_path = output / "department-logout-error.json"
    cleanup = None
    if cleanup_path.exists() or linked(cleanup_path):
        if read_json(bound_file(context.backend, binding(cleanup_path))) != {"error_type": "ValueError"}:
            raise ValueError("原部门计划关闭诊断发生变化")
        cleanup = binding(cleanup_path)
    if (binding(plan_path(context.directory_root)) != plan_descriptor
            or binding(prerequisite_path(context.directory_root)) != prerequisite_descriptor):
        raise ValueError("原部门计划或身份前提在采用期间变化")
    base = {"attempt": candidate["number"], "failure": failure,
            "controller": candidate_controller,
            "producer": observed["receipt"], "authentication": authentication_descriptor,
            "plan": plan_descriptor, "prerequisite": prerequisite_descriptor,
            "before": before, "original_fingerprints": candidate_fingerprints,
            "cleanup": cleanup}
    for adopted in lineage[1:]:
        adopted_fingerprints = _fingerprints(adopted)
        if adopted_fingerprints["product"] != candidate_fingerprints["product"]:
            raise ValueError("部门计划采用链包含不同产品构建来源")
        if adopted["result"] is None:
            if not _skipped_failure(context, adopted):
                raise ValueError("部门计划采用链包含无结果的非预检失败")
            continue
        adopted_controller, adopted_controller_value = _controller(context, adopted, current=False)
        _failure(context, adopted, adopted_controller, adopted_controller_value)
        adopted_guard = _empty_adoption_output(context, adopted)
        if adopted_guard is None:
            raise ValueError("部门计划采用链成功节点缺少只读守卫清单")
        expected_evidence = {**base, "current_controller": adopted_controller,
                             "current_fingerprints": adopted_fingerprints,
                             "guard": adopted_guard}
        expected_result = {"status": "seed_departments_planned", "plan": plan_descriptor,
            "prerequisite": prerequisite_descriptor, "adopted": expected_evidence,
            "remote_writes": 0, "worker_authorized": False, "restore_qualified": False}
        if _adoption_result(context, adopted) != expected_result:
            raise ValueError("部门计划采用链结果与原始证据或当前节点不同")
    current_fingerprints = _fingerprints(current)
    if current_fingerprints["product"] != candidate_fingerprints["product"]:
        raise ValueError("原部门计划与当前产品构建来源不同，禁止采用")
    if _empty_adoption_output(context, current) is not None:
        raise ValueError("当前部门计划采用提前出现守卫清单")
    return {**base, "current_controller": current_controller,
            "current_fingerprints": current_fingerprints}, prior


def adopt_failed_plan(context, request):
    state = load_state(context.directory_root)
    if len(state["attempts"]) < 2:
        return None
    current = state["attempts"][-1]
    candidate = state["attempts"][-2]
    if (candidate["stage"], candidate["mode"], candidate["status"]) != (
            "seed-runtime", "departments-plan", "failed"):
        return None
    number = current["number"]
    if (current["stage"], current["mode"], current["status"]) != (
            "seed-runtime", "departments-plan", "running") or context.output != (
            context.directory_root / "seed-runtime" / f"attempt-{number:04d}"):
        raise ValueError("部门计划采用必须属于紧邻的当前重试 attempt")
    guard(context, request)
    identity_projection(context.backend, request, allow_absent=True)
    context.administrator()
    evidence, prior = _adoption_evidence(context, request, state)
    guard(context, request, prior)
    repeated, _ = _adoption_evidence(context, request, load_state(context.directory_root))
    if repeated != evidence:
        raise ValueError("部门计划采用复核期间证据发生变化")
    evidence["guard"] = _publish_adoption_guard(context, state["attempts"][-1])
    return {"status": "seed_departments_planned", "plan": evidence["plan"],
            "prerequisite": evidence["prerequisite"], "adopted": evidence,
            "remote_writes": 0, "worker_authorized": False, "restore_qualified": False}


def load_plan(backend, directory, request, *, current=None):
    record = latest(directory, "seed-runtime", {"departments-plan"}, current=current)
    result = read_json(bound_file(backend, record["result"]))
    if result.get("status") != "seed_departments_planned" or result.get("plan") != binding(plan_path(directory)):
        raise ValueError("部门计划缺少当前登记的外层成功证明")
    return validate_plan(backend, directory, request)


def save_observed(context, attempt, observed):
    filename = attempt / ("observed-" + context.output.name.removeprefix("attempt-") + ".json")
    write_json(filename, observed)
    return binding(filename)


def save_confirmation(context, plan, action, target, attempt, observed, evidence, *, response_proven):
    if classify(action, observed, target) != "after":
        raise ValueError("部门完整后像不符")
    write_json(attempt / "confirmed.json", {"plan": binding(plan_path(context.directory_root)),
        "target": action["target"], "observed": observed, "post_attempted": True,
        "api_response_proven": response_proven, "evidence": evidence})


def reconciled_before(context, plan, action, target, attempt):
    previous_intent = intent(context.backend, context.directory_root, plan, action, attempt)
    if (attempt / "api-response.json").exists():
        raise ValueError("已有部门成功响应必须先核对为后像，禁止重放")
    files = sorted(attempt.glob("reconcile-*.json"))
    if not files:
        raise ValueError("存在未知部门写入，必须先显式核对，不能重放")
    if any(reconciliation(context.backend, context.directory_root, plan, action, target, attempt, filename) != "before"
           for filename in files):
        raise ValueError("上次部门核对未证明完整空前像，禁止新写入")
    return previous_intent


def apply_one(context, request, prerequisite, plan, action, target, bridge):
    root, previous = directories(context.directory_root, action)
    idempotency_key = str(uuid4())
    if previous and (previous[-1] / "confirmed.json").exists():
        confirmation(context.backend, context.directory_root, plan, action, target, previous[-1])
        if classify(action, observe(bridge, action, target), target) != "after":
            raise ValueError("已确认部门发生变化")
        return False
    if previous and (previous[-1] / "intent.json").exists():
        idempotency_key = reconciled_before(context, plan, action, target, previous[-1])["idempotency_key"]
    elif previous:
        allowed = {"before.json"}
        if {item.name for item in previous[-1].iterdir()} - allowed:
            raise ValueError("未登记 POST 的部门目录含未知证据")
    root.mkdir(parents=True, exist_ok=True)
    attempt = root / f"{len(previous) + 1:04d}"
    attempt.mkdir()
    guard(context, request, prerequisite, quick=True)
    before = observe(bridge, action, target)
    write_json(attempt / "before.json", before)
    if classify(action, before, target) != "before":
        raise ValueError("POST 前租户部门目录不再为空")
    guard(context, request, prerequisite, quick=True)
    write_json(attempt / "intent.json", {"plan": binding(plan_path(context.directory_root)),
        "target": action["target"], "before": binding(attempt / "before.json"), "body": action["body"],
        "operation": "post_system_depts", "idempotency_key": idempotency_key, "automatic_retry": False})
    response = bridge.call("create", tenant_id=target["tenant_id"], body=action["body"],
                           idempotency_key=idempotency_key)
    write_json(attempt / "api-response.json", response)
    created = created_response(action, target, response)
    observed = observe(bridge, action, target)
    current = image(action, observed, target)
    if current["department"] != created or classify(action, observed, target) != "after":
        raise ValueError("创建响应与随后完整目录及导入模板不同")
    descriptor = save_observed(context, attempt, observed)
    guard(context, request, prerequisite, quick=True)
    save_confirmation(context, plan, action, target, attempt, observed,
        {name: binding(attempt / (name + ".json")) for name in ("intent", "before", "api-response")}
        | {"observed": descriptor}, response_proven=True)
    return True


def reconcile_one(context, request, prerequisite, plan, action, target, bridge):
    _, attempts = directories(context.directory_root, action)
    if not attempts:
        return None
    attempt = attempts[-1]
    if (attempt / "confirmed.json").exists():
        confirmation(context.backend, context.directory_root, plan, action, target, attempt)
        if classify(action, observe(bridge, action, target), target) != "after":
            raise ValueError("已确认部门后像漂移")
        return "after"
    if not (attempt / "intent.json").exists():
        return None
    intent(context.backend, context.directory_root, plan, action, attempt)
    observed = observe(bridge, action, target)
    descriptor = save_observed(context, attempt, observed)
    state = classify(action, observed, target)
    response = attempt / "api-response.json"
    if response.exists():
        created = created_response(action, target, read_json(response))
        if state != "after" or image(action, observed, target)["department"] != created:
            raise ValueError("已有部门成功响应与当前完整后像矛盾")
        guard(context, request, prerequisite, quick=True)
        save_confirmation(context, plan, action, target, attempt, observed,
            {name: binding(attempt / (name + ".json"))
             for name in ("intent", "before", "api-response")} | {"observed": descriptor},
            response_proven=True)
        return state
    filename = attempt / ("reconcile-" + context.output.name.removeprefix("attempt-") + ".json")
    write_json(filename, {"intent": binding(attempt / "intent.json"), "observed": descriptor, "state": state})
    guard(context, request, prerequisite, quick=True)
    if state == "after":
        save_confirmation(context, plan, action, target, attempt, observed,
            {"intent": binding(attempt / "intent.json"), "before": binding(attempt / "before.json"),
             "observed": descriptor, "reconcile": binding(filename)}, response_proven=False)
    elif state != "before":
        raise ValueError("未知部门写入既非完整空前像也非唯一完整后像")
    return state


def verify_all(context, request, prerequisite, plan, source_targets, bridge):
    confirmed, observed = [], []
    for action, target in zip(plan["actions"], source_targets):
        _, attempts = directories(context.directory_root, action)
        if not attempts:
            raise ValueError("部门尚未覆盖全部普通租户")
        confirmation(context.backend, context.directory_root, plan, action, target, attempts[-1])
        guard(context, request, prerequisite, quick=True)
        value = observe(bridge, action, target)
        if classify(action, value, target) != "after":
            raise ValueError("部门最终只读验证发现完整后像变化")
        filename = context.output / (target["slot"] + "-observed.json")
        write_json(filename, value)
        observed.append(binding(filename))
        confirmed.append(binding(attempts[-1] / "confirmed.json"))
    return confirmed, observed


def execute_body(context, request, bridge, mode):
    source_targets = targets(identity_plan(context.backend, request))
    templates = template_identities(context.backend, request)
    plan = None
    if mode == "departments-plan":
        guard(context, request)
        identity_projection(context.backend, request, allow_absent=True)
    else:
        plan = load_plan(context.backend, context.directory_root, request, current=None)
        prior = read_prerequisite(context.backend, context.directory_root)
        require_prerequisite(context.backend, context.directory_root, request, prior)
        guard(context, request, prior)
    context.administrator()
    auth = bridge.login(post_request=context.request, tenant_ids=[item["tenant_id"] for item in source_targets],
                        template_identities=templates)
    expected_grant = {"scope_id": identity_plan(context.backend, request)["environment"]["scope_id"],
                      "tenant_ids": [item["tenant_id"] for item in source_targets],
                      "department_name": source_targets[0]["body"]["name"],
                      "template_principals": [principal(item) for item in templates]}
    if auth != expected_grant:
        raise ValueError("部门 bridge 初始化未绑定固定 scope、租户和名称")
    write_json(context.output / "authentication.json", auth)
    if mode == "departments-plan":
        return create_plan(context, request, bridge)
    if mode == "departments-apply":
        for action, target in zip(plan["actions"], source_targets):
            _, attempts = directories(context.directory_root, action)
            if attempts and (attempts[-1] / "intent.json").exists() and not (attempts[-1] / "confirmed.json").exists():
                reconciled_before(context, plan, action, target, attempts[-1])
    guard(context, request, prior, quick=True)
    writes, unresolved = 0, []
    if mode == "departments-apply":
        for action, target in zip(plan["actions"], source_targets):
            guard(context, request, prior, quick=True)
            writes += int(apply_one(context, request, prior, plan, action, target, bridge))
    elif mode == "departments-reconcile":
        for action, target in zip(plan["actions"], source_targets):
            guard(context, request, prior, quick=True)
            if reconcile_one(context, request, prior, plan, action, target, bridge) == "before":
                unresolved.append(action["target"]["slot"])
    elif mode == "departments-verify":
        confirmed, observed = verify_all(context, request, prior, plan, source_targets, bridge)
        return {"status": "seed_departments_verified", "plan": binding(plan_path(context.directory_root)),
                "confirmed": confirmed, "observed": observed, "remote_writes": 0,
                "worker_authorized": False, "restore_qualified": False}
    else:
        raise ValueError("未知部门阶段")
    confirmed = []
    for action, target in zip(plan["actions"], source_targets):
        _, attempts = directories(context.directory_root, action)
        if attempts and (attempts[-1] / "confirmed.json").exists():
            confirmation(context.backend, context.directory_root, plan, action, target, attempts[-1])
            confirmed.append(binding(attempts[-1] / "confirmed.json"))
    return {"status": "seed_departments_applied" if mode == "departments-apply" else "seed_departments_reconciled",
            "plan": binding(plan_path(context.directory_root)), "confirmed": confirmed, "unresolved": unresolved,
            "remote_writes": writes, "worker_authorized": False, "restore_qualified": False}


def execute_departments(context, request, mode):
    if mode == "departments-plan":
        adopted = adopt_failed_plan(context, request)
        if adopted is not None:
            return adopted
    source, private = identity_inputs(context.backend, context.directory_root, request, context.request)
    timeout = close_timeout(source)
    producer_context = SimpleNamespace(**{**vars(context), "request": request,
        "request_binding": binding(context.directory_root / "seed-runtime.json"),
        "private": {**context.private, "target_api": private}})
    bridge = Bridge(producer_context, kind=mode.replace("departments-", "department-"))
    deadline, close_requested, producer_close_requested = None, False, False

    def request_close():
        nonlocal close_requested
        close_requested = True
        return bridge.call("close", timeout=_close_rpc_timeout(source, deadline))

    def close_producer():
        nonlocal producer_close_requested
        producer_close_requested = True
        bridge.close()

    try:
        result = execute_body(context, request, bridge, mode)
        deadline = time.monotonic() + timeout
        request_close()
        close_producer()
        guard(context, request, read_prerequisite(context.backend, context.directory_root))
    except BaseException as original:
        if deadline is None:
            deadline = time.monotonic() + timeout
        cleanup_steps = [] if close_requested else [("department-logout", request_close)]
        if not producer_close_requested:
            cleanup_steps.append(("department-producer-close", close_producer))
        for name, cleanup in cleanup_steps:
            try:
                cleanup()
            except Exception as error:
                cleanup_failure(context, name, error, original)
        raise
    return result
