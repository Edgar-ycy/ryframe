"""固定复制后阶段的原生身份及其他生产者收据；内核身份与参数一致才允许回收。"""
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
from types import SimpleNamespace
import uuid

from devex_clone_capture import read_json, write_json
from devex_clone_model import digest, exact, linked, local_path
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from devex_clone_target_binding import external_file
from devex_clone_target_storage import actual_windows_argv
from full_stack_process import process_identity, terminate_owned_process
from restore_reference_plan import plan_hash

KINDS = {"session": "xtask/src/recovery/clone/native/session.rs", "existing": "xtask/src/recovery/dataset_prepare/existing_native.rs",
         "identity-apply": "xtask/src/data/performance_identities/producer.rs",
         "identity-verify": "xtask/src/data/performance_identities/producer.rs",
         "quota-plan": "xtask/src/data/performance_identities/clone_bridge.rs",
         "quota-apply": "xtask/src/data/performance_identities/clone_bridge.rs",
         "quota-reconcile": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-plan": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-apply": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-reconcile": "xtask/src/data/performance_identities/clone_bridge.rs",
         "department-verify": "xtask/src/data/performance_identities/clone_bridge.rs"}
PHASES = {"session": ("post-copy", {"schedules", "reconcile"}), "existing": ("post-copy", {"verify"}),
          "identity-apply": ("seed-runtime", {"identities-apply"}),
          "identity-verify": ("seed-runtime", {"identities-verify"}),
          "quota-plan": ("seed-runtime", {"quotas-plan"}),
          "quota-apply": ("seed-runtime", {"quotas-apply"}),
          "quota-reconcile": ("seed-runtime", {"quotas-reconcile"}),
          "department-plan": ("seed-runtime", {"departments-plan"}),
          "department-apply": ("seed-runtime", {"departments-apply"}),
          "department-reconcile": ("seed-runtime", {"departments-reconcile"}),
          "department-verify": ("seed-runtime", {"departments-verify"})}
STAGES = {"post-copy", "seed-runtime"}
IDENTITY_RELEASE_ID = re.compile(
    r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}")


def producer_registration(backend, directory, stage, *, cleanup=False):
    if stage == "post-copy":
        from devex_clone_post import registration

        active = registration(backend, directory, cleanup=cleanup)
        return active.request, active.descriptor
    elif stage == "seed-runtime":
        from devex_clone_seed import registered
    else:
        raise ValueError("Node 生产者阶段未登记")
    return registered(backend, directory, cleanup=cleanup), binding(directory / "seed-runtime.json")


def registered_producer(backend, directory, stage, *, cleanup=False):
    return producer_registration(backend, directory, stage, cleanup=cleanup)[0]


def historical_producer_request(backend, directory, stage, descriptor):
    if stage == "post-copy":
        from devex_clone_post import registration

        return registration(backend, directory, descriptor=descriptor, cleanup=True).request
    return registered_producer(backend, directory, stage, cleanup=True)


def cleanup_failure(context, label, error, original):
    try:
        write_json(context.output / (label + "-error.json"), {"error_type": type(error).__name__})
    except Exception as recording_error:
        original.add_note("清理诊断未保存：" + type(recording_error).__name__)


def actual_argv(identity):
    if os.name == "nt":
        return actual_windows_argv(SimpleNamespace(runner=subprocess.run), identity)
    if process_identity(identity["pid"]) != identity:
        raise ValueError("读取 Node 参数前内核身份发生变化")
    command = Path(f'/proc/{identity["pid"]}/cmdline').read_bytes().rstrip(b"\0").split(b"\0")
    if process_identity(identity["pid"]) != identity:
        raise ValueError("读取 Node 参数期间内核身份发生变化")
    return [os.fsdecode(item) for item in command]


def original_attempt(attempt):
    return {**attempt, "status": "running", "finished_at": None, "result": None, "error_type": None}


def native_kind(kind):
    return kind in KINDS


def producer_command(backend, request, directory, number, kind, output, *, tool_override=None,
                     configuration=None):
    if kind not in KINDS:
        raise ValueError("Node 生产者类型未登记")
    native = native_kind(kind)
    script = producer_source(backend, kind)
    tool = producer_tool(request, kind) if tool_override is None else tool_override
    hidden = ("__copy-session" if kind == "session" else
              "__recovery-existing-clone" if kind == "existing" else
              "__identity-producer" if kind.startswith("identity-") else
              "__identity-bridge")
    command = [tool["path"], hidden if native else str(script),
               "--run-dir", str(directory), "--attempt", str(number)]
    if kind == "existing":
        if configuration is None:
            raise ValueError("原生已有数据生产者缺少配置绑定")
        command.extend(["--backend-dir", str(backend), "--plan", request["reference_plan"]["path"],
                        "--dataset", request["dataset"]["path"], "--target-binding", str(output / "business-target.json"),
                        "--configuration", str(configuration), "--write"])
    elif kind in {"identity-apply", "identity-verify"}:
        command.extend(["--identity-plan", request["identity_plan"]["path"],
                        "--identity-state", request["identity_state"], "--identity-mode", kind.removeprefix("identity-"),
                        "--identity-plan-sha256", request["identity_plan"]["sha256"]])
    elif kind in {"quota-plan", "quota-apply", "quota-reconcile"}:
        command.extend(["--quota-mode", kind.removeprefix("quota-"),
                        "--identity-plan", request["identity_plan"]["path"],
                        "--identity-plan-sha256", request["identity_plan"]["sha256"]])
    elif kind in {"department-plan", "department-apply", "department-reconcile", "department-verify"}:
        command.extend(["--department-mode", kind.removeprefix("department-"),
                        "--identity-plan", request["identity_plan"]["path"],
                        "--identity-plan-sha256", request["identity_plan"]["sha256"]])
    return command


def producer_source(backend, kind):
    if native_kind(kind):
        return Path(backend) / KINDS[kind]
    return Path(backend) / "tools/js" / KINDS[kind]


def producer_tool(request, kind):
    if kind in {"session", "existing"}:
        from rate_limit_tool import tool_binding

        value = tool_binding()
        if value is None:
            label = "复制会话" if kind == "session" else "已有数据生产者"
            raise ValueError(f"原生{label}缺少协调器工具绑定")
        return {key: value[key] for key in ("path", "sha256")}
    return request["identity_tool" if native_kind(kind) else "node"]


def inspect_producer(backend, directory, attempt):
    stage = attempt["stage"]
    if stage not in STAGES:
        raise ValueError("Node 生产者历史不属于明确阶段")
    number = attempt["number"]
    output = directory / stage / f"attempt-{number:04d}"
    intent_path, path = output / "session-launch.json", output / "session-process.json"
    if not intent_path.exists() and not path.exists():
        if output.exists() and any((output / name).exists() for name in ("session.stderr.log", "session.stdout.log")):
            raise ValueError("Node 生产者证据不完整，缺少明确启动收据")
        return None
    if linked(output) or linked(intent_path) or linked(path):
        raise ValueError("Node 生产者证据不能是链接")
    if not intent_path.exists() or not path.exists():
        raise ValueError("Node 生产者缺少内核身份收据，不能猜测或继续")
    intent, value = read_json(intent_path), read_json(path)
    kind = intent["producer_kind"]
    native = native_kind(kind)
    tool_key, source_key = ("tool", "source") if native else ("node", "script")
    intent_fields = {"format_version", "kind", "run_manifest", "registration", "controller", "attempt", "producer_kind", "command", tool_key, source_key}
    if kind == "existing":
        intent_fields.add("configuration")
    exact(intent, intent_fields)
    owner_key = "identity_lock_owner" if kind.startswith("identity-") else "producer_owner"
    owner_required = native and kind != "existing"
    owner_recorded = owner_required and owner_key in value
    exact(value, {"format_version", "kind", "launch", "identity"}
          | ({owner_key} if owner_recorded else set()))
    request = historical_producer_request(backend, directory, stage, intent["registration"])
    if (kind not in PHASES or stage != PHASES[kind][0] or attempt["mode"] not in PHASES[kind][1]
            or intent["format_version"] != 1 or intent["kind"] != "devex-post-producer-launch"
            or value["format_version"] != 1 or value["kind"] != "devex-post-producer"
            or value["launch"] != binding(intent_path) or intent["attempt"] != number
            or intent["run_manifest"] != binding(directory / "manifest.json")
            or (stage != "post-copy" and intent["registration"] != binding(directory / (stage + ".json")))):
        raise ValueError("Node 生产者不属于本 run 的明确阶段和固定登记")
    controller_path = bound_file(backend, intent["controller"])
    controller = read_json(controller_path)
    if (controller_path != directory / f"controller-{number:04d}.json"
            or controller.get("kind") != "devex-stage-controller" or controller.get("format_version") != 1
            or controller.get("attempt") != number or controller.get("attempt_sha256") != plan_hash(original_attempt(attempt))
            or controller["owner"]["directory"] != str(directory)
            or controller["owner"]["manifest_sha256"] != intent["run_manifest"]["sha256"]):
        raise ValueError("Node 生产者控制器不属于原始阶段")
    configuration = None
    if kind == "existing":
        configuration = bound_file(backend, intent["configuration"])
        from restore_reference_plan import validate_configuration_binding
        validate_configuration_binding(read_json(configuration), backend)
        if configuration != output / "existing-configuration.json":
            raise ValueError("原生已有数据配置绑定不属于当前 attempt")
    command = producer_command(
        backend, request, directory, number, kind, output,
        tool_override=intent[tool_key] if native else None,
        configuration=configuration)
    persisted_tool = kind not in {"session", "existing"}
    if (intent["command"] != command
            or (persisted_tool and intent[tool_key] != producer_tool(request, kind))
            or intent[source_key]["path"] != str(producer_source(backend, kind))):
        raise ValueError("Node 参数或工具不属于固定 run 和 attempt")
    # 历史执行保留原摘要，但清理不能要求后来更新或移除的工具字节仍留在磁盘。
    exact(intent[tool_key], {"path", "sha256"})
    exact(intent[source_key], {"path", "bytes", "sha256"})
    digest(intent[tool_key]["sha256"])
    digest(intent[source_key]["sha256"])
    if type(intent[source_key]["bytes"]) is not int or intent[source_key]["bytes"] <= 0:
        raise ValueError("Node 历史脚本摘要结构无效")
    exact(value["identity"], {"pid", "started", "executable"})
    if (owner_recorded
            and (not isinstance(value[owner_key], str)
                 or not re.fullmatch(r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}",
                                     value[owner_key]))):
        raise ValueError("原生生产者收据缺少固定 owner")
    if owner_required and not owner_recorded:
        raise ValueError("原生生产者收据必须记录 owner")
    if Path(value["identity"]["executable"]).resolve() != Path(intent[tool_key]["path"]).resolve():
        raise ValueError("Node 内核 executable 与登记工具不同")
    actual = process_identity(value["identity"]["pid"])
    if actual == value["identity"]:
        if actual_argv(actual) != command:
            raise ValueError("Node 实际参数不属于固定 run 和 attempt")
        alive = True
    elif actual is None:
        alive = False
    elif (isinstance(actual, dict) and set(actual) == {"pid", "started", "executable"}
          and type(actual["pid"]) is int and actual["pid"] == value["identity"]["pid"]
          and isinstance(actual["started"], str) and actual["started"].isdigit()
          and isinstance(value["identity"]["started"], str)
          and value["identity"]["started"].isdigit()
          and int(actual["started"]) > int(value["identity"]["started"])):
        alive = False
    else:
        raise ValueError("Node 内核身份不能证明为已结束的原创建代次")
    return {"receipt": binding(path), "value": value, "controller": controller, "alive": alive,
            "producer_kind": kind, "request": request}


def require_quiet(backend, directory, current_number=None):
    for attempt in load_state(directory, verify_results=False)["attempts"]:
        if attempt["stage"] not in STAGES or attempt["number"] == current_number:
            continue
        observed = inspect_producer(backend, directory, attempt)
        if observed and observed["alive"]:
            raise ValueError("旧 Node 生产者仍在运行，先显式 recover-session，再核对未知意图")


def _identity_lock_snapshot(path):
    try:
        before = path.lstat()
    except FileNotFoundError:
        return None
    if linked(path) or not stat.S_ISREG(before.st_mode):
        raise ValueError("身份锁不是固定根目录内的普通文件，保留现场")
    with path.open("rb") as stream:
        opened = os.fstat(stream.fileno())
        content = stream.read(4097)
    try:
        after = path.lstat()
    except FileNotFoundError as error:
        raise ValueError("读取身份锁期间路径消失，拒绝清理") from error
    identities = ((before.st_dev, before.st_ino), (opened.st_dev, opened.st_ino),
                  (after.st_dev, after.st_ino))
    if len(set(identities)) != 1 or linked(path) or not stat.S_ISREG(after.st_mode) or len(content) > 4096:
        raise ValueError("读取身份锁期间文件身份变化或内容越界，保留现场")
    return identities[0], content


def _identity_lock_value(content):
    def unique(pairs):
        value = {}
        for key, item in pairs:
            if key in value:
                raise ValueError("身份锁包含重复字段，保留现场")
            value[key] = item
        return value

    try:
        value = json.loads(content.decode("utf-8"), object_pairs_hook=unique)
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ValueError("身份锁不是明确 JSON，保留现场") from error
    exact(value, {"format_version", "kind", "pid", "plan_sha256", "owner_token"})
    return value


def _identity_release_directory(root, *, create=False):
    directory = root / "lock-releases"
    if not directory.exists():
        if linked(directory):
            raise ValueError("身份锁释放目录是链接，保留现场")
        if not create:
            return None
        try:
            directory.mkdir()
        except FileExistsError:
            pass
    if linked(directory) or not stat.S_ISDIR(directory.lstat().st_mode):
        raise ValueError("身份锁释放目录不是固定普通目录，保留现场")
    return directory


def _identity_release_paths(directory, release_id):
    return (directory / f"{release_id}.lock",
            directory / f"{release_id}.released.json",
            directory / f"{release_id}.released.json.pending")


def _identity_release_inventory(root):
    directory = _identity_release_directory(root)
    if directory is None:
        return None, {}
    records = {}
    patterns = ((re.compile(r"^([0-9a-f-]{36})\.lock$"), "lock"),
                (re.compile(r"^([0-9a-f-]{36})\.released\.json$"), "marker"),
                (re.compile(r"^([0-9a-f-]{36})\.released\.json\.pending$"), "pending"))
    before = sorted(item.name for item in directory.iterdir())
    for name in before:
        match = next(((pattern.fullmatch(name), kind) for pattern, kind in patterns
                      if pattern.fullmatch(name)), None)
        if match is None or IDENTITY_RELEASE_ID.fullmatch(match[0].group(1)) is None:
            raise ValueError("身份锁释放目录存在未知证据，保留现场")
        release_id, kind = match[0].group(1), match[1]
        record = records.setdefault(release_id, set())
        if kind in record:
            raise ValueError("身份锁释放目录存在重复证据，保留现场")
        record.add(kind)
    if before != sorted(item.name for item in directory.iterdir()):
        raise ValueError("身份锁释放目录在读取期间变化，保留现场")
    return directory, records


def _identity_release_marker(release_id, tombstone, content):
    lock = _identity_lock_value(content)
    marker = {"format_version": 1, "kind": "devex-identity-run-lock-release",
              "release_id": release_id, "tombstone_file": tombstone.name,
              "lock_sha256": hashlib.sha256(content).hexdigest(), "lock": lock}
    encoded = (json.dumps(marker, ensure_ascii=False, sort_keys=True, indent=2, separators=(",", ": ")) + "\n").encode("utf-8")
    return lock, marker, encoded


def _identity_release_record(directory, release_id):
    tombstone, marker, pending = _identity_release_paths(directory, release_id)
    snapshot = _identity_lock_snapshot(tombstone)
    if snapshot is None:
        raise ValueError("身份锁 release 记录缺少墓碑，保留现场")
    lock, _, expected = _identity_release_marker(release_id, tombstone, snapshot[1])
    marker_snapshot = _identity_lock_snapshot(marker)
    pending_snapshot = _identity_lock_snapshot(pending)
    if marker_snapshot is not None and marker_snapshot[1] != expected:
        raise ValueError("身份锁 released marker 与墓碑不一致，保留现场")
    if pending_snapshot is not None and pending_snapshot[1] != expected:
        raise ValueError("身份锁 released pending 与墓碑不一致，保留现场")
    if (_identity_lock_snapshot(tombstone) != snapshot
            or (marker_snapshot is not None and _identity_lock_snapshot(marker) != marker_snapshot)
            or (pending_snapshot is not None and _identity_lock_snapshot(pending) != pending_snapshot)):
        raise ValueError("身份锁 release 墓碑在核验期间变化，保留现场")
    return lock, expected, marker_snapshot is not None, pending_snapshot is not None


def _write_identity_release_marker(marker, pending, expected):
    current, staged = _identity_lock_snapshot(marker), _identity_lock_snapshot(pending)
    if current is not None and current[1] != expected:
        raise ValueError("已有身份锁 released marker 不同，保留现场")
    if staged is not None and staged[1] != expected:
        raise ValueError("已有身份锁 released pending 不同，保留现场")
    if current is None and staged is None:
        with pending.open("xb") as stream:
            stream.write(expected)
            stream.flush()
            os.fsync(stream.fileno())
        staged = _identity_lock_snapshot(pending)
    if current is None:
        try:
            os.link(pending, marker)
        except FileExistsError:
            pass
    published = _identity_lock_snapshot(marker)
    if published is None or published[1] != expected:
        raise ValueError("身份锁 released marker 发布后不一致，保留现场")
    if staged is not None and _identity_lock_snapshot(pending) == staged:
        pending.unlink()


def _identity_lock_release_checkpoint(_stage, _paths):
    """测试仅在固定 release 边界注入进程终止或锁替换。"""


def _identity_expected_lock(identity, owner_token, plan_sha256):
    value = {"format_version": 1, "kind": "devex-identity-run-lock", "pid": identity["pid"],
             "plan_sha256": plan_sha256, "owner_token": owner_token}
    expected = (json.dumps(value, ensure_ascii=False, sort_keys=True, indent=2, separators=(",", ": ")) + "\n").encode("utf-8")
    return value, expected


def recover_identity_lock(backend, request, identity, owner_token):
    """固定 Node 死亡后只发布永久 release 记录；绝不按 pathname 删除锁。"""
    root = local_path(backend, request["identity_state"])
    if str(root.resolve()) != request["identity_state"]:
        raise ValueError("身份锁根目录不是生产者绑定的规范绝对路径")
    path = root / "lock"
    directory, records = _identity_release_inventory(root)
    first = None
    if not records:
        first = _identity_lock_snapshot(path)
        if first is None:
            return False
    plan = read_json(bound_file(backend, request["identity_plan"]))
    plan_sha256 = digest(plan.get("plan_sha256"))
    value, expected = _identity_expected_lock(identity, owner_token, plan_sha256)
    matching, incomplete, release_owners = [], [], set()
    for release_id in records:
        lock, marker_bytes, released, pending = _identity_release_record(directory, release_id)
        if lock["owner_token"] in release_owners:
            raise ValueError("同一身份锁 owner 存在多个 released 记录，保留现场")
        release_owners.add(lock["owner_token"])
        if not released or pending:
            incomplete.append(release_id)
        if lock == value:
            matching.append((release_id, marker_bytes, released, pending))
    if len(matching) > 1 or any(item not in {entry[0] for entry in matching} for item in incomplete):
        raise ValueError("身份锁存在不属于固定 owner 的未完成 release，保留现场")
    if matching:
        release_id, marker_bytes, released, pending_exists = matching[0]
        tombstone, marker, pending = _identity_release_paths(directory, release_id)
        if not released or pending_exists:
            _write_identity_release_marker(marker, pending, marker_bytes)
        return not released
    first = first or _identity_lock_snapshot(path)
    if first is None:
        return False
    if _identity_lock_value(first[1]) != value or first[1] != expected:
        raise ValueError("身份锁不属于已确认死亡的固定 Node、计划和 owner，保留现场")
    second = _identity_lock_snapshot(path)
    if second != first:
        raise ValueError("身份锁 claim 前被替换或修改，保留现场")
    _identity_lock_release_checkpoint("before-claim", {"lock": path})
    directory = _identity_release_directory(root, create=True)
    release_id = str(uuid.uuid4())
    tombstone, marker, pending = _identity_release_paths(directory, release_id)
    if any(item.exists() or linked(item) for item in (tombstone, marker, pending)):
        raise ValueError("身份锁随机 release 名称已存在，保留现场")
    path.rename(tombstone)
    if _identity_lock_snapshot(tombstone) != first:
        raise ValueError("身份锁 claim 时被替换；replacement 已保留在墓碑")
    _identity_lock_release_checkpoint("claimed", {"lock": path, "tombstone": tombstone, "marker": marker})
    _, _, marker_bytes = _identity_release_marker(release_id, tombstone, expected)
    _write_identity_release_marker(marker, pending, marker_bytes)
    _identity_lock_release_checkpoint("released", {"lock": path, "tombstone": tombstone, "marker": marker})
    return True


def recover_session(backend, directory, number, descriptor):
    """调用方持统一 run 锁；只处理显式绑定，不扫描进程或共享目录。"""
    path = bound_file(backend, descriptor)
    state = load_state(directory, verify_results=False)
    current_attempt = state["attempts"][-1]
    stage = current_attempt["stage"]
    if (current_attempt["number"] != number or stage not in STAGES
            or current_attempt["mode"] != "recover-session" or current_attempt["status"] != "running"
            or read_json(directory / "run.lock/owner.json")["identity"] != process_identity(os.getpid())):
        raise ValueError("Node 回收必须持当前 run 锁并属于明确恢复阶段")
    attempts = [item for item in state["attempts"] if item["stage"] == stage
                and path == directory / stage / f'attempt-{item["number"]:04d}' / "session-process.json"]
    if len(attempts) != 1 or attempts[0]["number"] >= number or attempts[0]["status"] == "running":
        raise ValueError("只能恢复已经核对控制器退出的历史阶段生产者")
    attempt = attempts[0]
    observed = inspect_producer(backend, directory, attempt)
    if observed is None or observed["receipt"] != descriptor:
        raise ValueError("明确生产者收据发生变化")
    if process_identity(observed["controller"]["owner"]["identity"]["pid"]) is not None:
        raise ValueError("原控制进程仍存在或 PID 已复用，不能回收其 Node")
    output = directory / stage / f"attempt-{number:04d}"
    output.mkdir()
    write_json(output / "session-recovery-intent.json", {"producer": descriptor, "identity": observed["value"]["identity"],
               "action": "terminate_exact_node", "copy_requires_reconciliation": True})
    current = inspect_producer(backend, directory, attempt)
    if current != observed or bound_file(backend, descriptor) != path:
        raise ValueError("回收窗口内 Node 生产者证据变化")
    if current["alive"]:
        terminate_owned_process(current["value"]["identity"])
    if bound_file(backend, descriptor) != path or process_identity(current["value"]["identity"]["pid"]) is not None:
        raise ValueError("Node 生产者回收后身份仍不确定")
    if current["producer_kind"] in {"identity-apply", "identity-verify"}:
        recover_identity_lock(backend, current["request"], current["value"]["identity"],
                              current["value"]["identity_lock_owner"])
    return {"status": "seed_session_recovered" if stage == "seed-runtime" else "post_session_recovered",
            "producer": descriptor, "already_stopped": not current["alive"],
            "copy_requires_reconciliation": True, "remote_writes": 0, "restore_qualified": False}


class Producer:
    def __init__(self, context, kind):
        self.context = context
        number = int(context.output.name.removeprefix("attempt-"))
        require_quiet(context.backend, context.directory_root, number)
        state = load_state(context.directory_root)
        attempt = state["attempts"][-1]
        if (kind not in PHASES or attempt["number"] != number or attempt["status"] != "running"
                or attempt["stage"] != PHASES[kind][0] or attempt["mode"] not in PHASES[kind][1]
                or context.output != context.directory_root / attempt["stage"] / f"attempt-{number:04d}"):
            raise ValueError("Node 启动必须属于当前运行阶段")
        current_request, current_binding = producer_registration(
            context.backend, context.directory_root, attempt["stage"])
        if context.request_binding != current_binding or context.request != current_request:
            raise ValueError("Node 启动必须使用当前阶段已发布的固定登记")
        controller_path = context.directory_root / f"controller-{number:04d}.json"
        controller = read_json(controller_path)
        if (controller["attempt_sha256"] != plan_hash(attempt)
                or controller["owner"]["identity"] != process_identity(os.getpid())
                or read_json(context.directory_root / "run.lock/owner.json") != controller["owner"]):
            raise ValueError("Node 启动控制器未持当前 run 锁")
        native = native_kind(kind)
        tool = producer_tool(context.request, kind)
        external_file(tool)
        tool_key, source_key = ("tool", "source") if native else ("node", "script")
        configuration = None
        configuration_descriptor = None
        if kind == "existing":
            from restore_reference_plan import configuration_binding

            contract = read_json(context.runtime / "runtime.json")
            configuration = context.output / "existing-configuration.json"
            write_json(configuration, configuration_binding(
                context.backend, Path(contract["api_config_dir"]), "test"
            ))
            configuration_descriptor = binding(configuration)
        command = producer_command(
            context.backend, context.request, context.directory_root, number, kind,
            context.output, tool_override=tool, configuration=configuration)
        intent = {"format_version": 1, "kind": "devex-post-producer-launch", "run_manifest": binding(context.directory_root / "manifest.json"),
                  "registration": context.request_binding, "controller": binding(controller_path), "attempt": number,
                  "producer_kind": kind, "command": command, tool_key: tool,
                  source_key: binding(producer_source(context.backend, kind))}
        if configuration_descriptor is not None:
            intent["configuration"] = configuration_descriptor
        write_json(context.output / "session-launch.json", intent)
        self.log = (context.output / "session.stderr.log").open("xb")
        self.child = None
        self.identity = None
        self.identity_lock_owner = str(uuid.uuid4()) if kind.startswith("identity-") else None
        self.producer_owner = (str(uuid.uuid4())
                               if native and kind != "existing" and self.identity_lock_owner is None
                               else None)
        self.tool_guard = None
        try:
            environment = context.private["target_api"]
            if native:
                from artifact_digests import protect_binary_invocation
                from process_environment import configured

                environment = configured({"APP_ENV": "test"})
                self.tool_guard = protect_binary_invocation(tool)
                self.tool_guard.__enter__()
            self.child = subprocess.Popen(command, cwd=context.backend, env=environment,
                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            self.identity = process_identity(self.child.pid)
            if self.identity is None or Path(self.identity["executable"]).resolve() != Path(command[0]).resolve():
                raise ValueError("Node 生产者内核工具身份不符")
            receipt = {"format_version": 1, "kind": "devex-post-producer",
                       "launch": binding(context.output / "session-launch.json"), "identity": self.identity}
            if self.identity_lock_owner is not None:
                receipt["identity_lock_owner"] = self.identity_lock_owner
            if self.producer_owner is not None:
                receipt["producer_owner"] = self.producer_owner
            write_json(context.output / "session-process.json", receipt)
            if actual_argv(self.identity) != command:
                raise ValueError("Node 生产者实际 run/attempt 参数不符")
        except BaseException as original:
            try:
                self.close()
            except Exception as cleanup_error:
                cleanup_failure(context, "session-cleanup", cleanup_error, original)
            raise

    def close(self):
        try:
            if self.child is not None:
                if self.child.poll() is None:
                    identity = self.identity
                    if identity is None:
                        raise ValueError("缺少 Node 创建身份，保留现场，不按 PID 终止")
                    terminate_owned_process(identity)
                self.child.wait(timeout=10)
        finally:
            # readline 线程可能仍持有 stdout 的缓冲锁；无法回收时不能阻塞关闭其流。
            if self.child is not None and self.child.poll() is not None:
                self.child.stdin.close()
                self.child.stdout.close()
            self.log.close()
            if self.tool_guard is not None:
                guard, self.tool_guard = self.tool_guard, None
                guard.__exit__(None, None, None)

    def communicate(self, timeout):
        import json

        number = int(self.context.output.name.removeprefix("attempt-"))
        release = {"operation": "start", "run_dir": str(self.context.directory_root), "attempt": number}
        if self.identity_lock_owner is not None:
            release.update(identity_lock_owner=self.identity_lock_owner,
                           launch=binding(self.context.output / "session-launch.json"),
                           process=binding(self.context.output / "session-process.json"))
        elif self.producer_owner is not None:
            release.update(producer_owner=self.producer_owner,
                           launch=binding(self.context.output / "session-launch.json"),
                           process=binding(self.context.output / "session-process.json"))
        stdout, _ = self.child.communicate((json.dumps(release) + "\n").encode(), timeout=timeout)
        if self.child.returncode:
            raise ValueError("既有数据 Node 验证失败，保留独立日志")
        return stdout


def runtime_proof(request):
    """共享运行来源只读核验；凭据与身份数据库查询不经过此协议。"""
    from full_stack_runtime import verify_runtime
    from full_stack_process import read_process
    from process_sockets import verify_listener

    exact(request, {"backend", "directory", "scope_id", "api_url"})
    backend = Path(request["backend"])
    if (os.environ.get("APP_ENV") != "test" or not backend.is_absolute()
            or backend != backend.resolve(strict=True) or linked(backend)):
        raise ValueError("runtime_proof_configuration")
    directory = local_path(backend, request["directory"])
    contract = verify_runtime(backend, directory)
    if contract["scope_id"] != request["scope_id"]:
        raise ValueError("runtime_proof_scope")
    identity = read_process(directory, "api", request["scope_id"])
    if (process_identity(identity["pid"]) != identity
            or Path(identity["executable"]).resolve() != Path(contract["artifacts"]["api"]["path"]).resolve()):
        raise ValueError("runtime_proof_process")
    verify_listener(identity["pid"], request["api_url"])
    return {"runtime": contract, "api_process": identity}


def runtime_request(content):
    """共享来源协议拒绝重复字段、非法 UTF-8 与越界输入。"""
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("runtime_proof_duplicate_field")
            result[key] = value
        return result

    if len(content) > 32768:
        raise ValueError("runtime_proof_limit")
    return json.loads(content.decode("utf-8"), object_pairs_hook=unique)


if __name__ == "__main__":
    import sys

    try:
        if sys.argv[1:] != ["--runtime-proof"]:
            raise ValueError("runtime_proof_protocol")
        content = sys.stdin.buffer.read(32769)
        receipt = runtime_proof(runtime_request(content))
        sys.stdout.write(json.dumps({"ok": True, "receipt": receipt}) + "\n")
    except Exception:
        sys.stdout.write('{"ok":false,"receipt":null}\n')
        sys.exit(1)
