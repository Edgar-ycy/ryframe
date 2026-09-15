"""在原复制账本内发布冻结 seed 的只读运行后继；原 C52 文件保持不可变。"""
from __future__ import annotations

import copy
from contextlib import contextmanager
import os
from pathlib import Path
import subprocess

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, local_path, name
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file, validate_request
from restore_reference_plan import plan_hash
from source_fingerprints import require_current_execution_source

START = "source-generation-start"
STOP = "source-generation-stop"
RECOVER = "source-generation-recover"
STATUS = "source-generation-status"
REQUEST_FIELDS = {"format_version", "kind", "id", "source_registration", "review_successor",
                  "source_rebind", "current_storage", "execution_backend", "expected_backend_sha",
                  "adapter_contract", "product_backend", "backend_build", "maintenance_build", "source_environment"}
RESULT_FIELDS = {"status", "request", "source_registration", "source_rebind", "review_successor",
                 "current_storage", "history_length", "history_sha256", "source_request",
                 "generation_verified", "runtime_evidence", "before", "after", "remote_writes", "restore_qualified"}
RESULT_FIELDS.add("dataset_lineage")
RESULT_FIELDS.update({"start", "source_runtime", "running", "stop_before"})
START_FIELDS = {"status", "request", "source_registration", "source_rebind", "review_successor", "current_storage",
                "history_length", "history_sha256", "generation_id", "intent", "runtime_evidence", "before", "running",
                "dataset_lineage", "coordinator_source", "remote_writes", "restore_qualified"}


def preflight(directory: Path, *, backend: Path | None = None) -> None:
    from devex_clone_seed_generation_prelaunch import closed

    attempts = load_state(directory)["attempts"]
    archived = closed(backend or Path(__file__).resolve().parents[1], directory, attempts)
    ignored = () if archived is None else archived["records"]
    if any(item["stage"] == "seed-runtime" and item["mode"] in {START, STOP, RECOVER, "source-export", "arm-input"}
           and item not in ignored for item in attempts):
        raise ValueError("source-generation 只执行一次；失败必须核对完整前后像，不能重放启动")
    rebound = [item for item in attempts if (item["stage"], item["mode"]) == ("seed-runtime", "source-rebind")]
    if len(rebound) != 1 or rebound[0]["status"] != "passed":
        raise ValueError("source-generation 必须继承唯一已发布 storage rebind")


def inputs(backend: Path, directory: Path, request_path: Path, number: int) -> tuple[dict, dict, list]:
    from devex_clone_run import _require_owned_run
    from devex_clone_seed_export import _source

    _require_owned_run(directory)
    request = read_json(local_path(backend, str(request_path)))
    exact(request, REQUEST_FIELDS)
    if type(request["format_version"]) is not int or request["format_version"] != 1 or request["kind"] != "devex-clone-seed-source-generation":
        raise ValueError("source-generation 请求类型无效")
    name(request["id"])
    state = load_state(directory)
    if not state["attempts"] or tuple(state["attempts"][-1][key] for key in ("number", "stage", "mode", "status")) != (
            number, "seed-runtime", START, "running"):
        raise ValueError("source-generation 不是同一账本的当前持锁阶段")
    require_current_execution_source(backend, state["attempts"][-1]["sources"])
    source = _source(backend, directory, live=False)
    expected = {"source_registration": source["review_successor"]["source_result"],
                "review_successor": source["review_successor_binding"], "source_rebind": source["source_rebind"],
                "current_storage": source["storage"]["storage"]}
    if source.get("source_generation") is not None or any(request[key] != value for key, value in expected.items()):
        raise ValueError("source-generation 没有精确继承同一 C52、successor 与当前存储")
    for field in ("source_registration", "source_rebind", "review_successor", "backend_build",
                  "source_environment"):
        bound_file(backend, request[field])
    from reference_fixture_successor_generation import rebuild

    if rebuild(backend, request) != request:
        raise ValueError("source-generation 请求与正式生产者重算结果不同")
    return request, source, state["attempts"][:-1]


def source_request(source: dict, request: dict, runtime: Path) -> dict:
    value = copy.deepcopy(source["request"])
    build = read_json(Path(request["backend_build"]["path"]))
    value.update(id=request["id"], backend_build=request["backend_build"], maintenance_build=request["maintenance_build"],
                 worktree_fingerprint=build["sources"]["full"]["source"]["worktree_fingerprint"],
                 runtime=binding(runtime / "runtime.json"),
                 processes={role: binding(runtime / f"{role}.json") for role in ("api", "worker")},
                 producers_registry=binding(runtime.parent / "producers.json"))
    value["source"]["runtime_dir"] = str(runtime)
    return value


def predecessor(backend: Path, directory: Path, state: dict) -> dict:
    from devex_clone_seed_source import _registered_source
    from devex_clone_seed_rebind import resolve_storage
    from reference_fixture_successor import _source_with_loader

    records = [item for item in state["attempts"] if (item["stage"], item["mode"]) == ("seed-runtime", "source-rebind")]
    if len(records) != 1 or records[0]["status"] != "passed":
        raise ValueError("源运行代次缺少唯一已发布重绑定")
    rebound = read_json(bound_file(backend, records[0]["result"]))
    source = _source_with_loader(backend, rebound["review_successor"], live_storage=False, loader=_registered_source)
    if source["directory"] != directory:
        raise ValueError("源运行代次不属于同一原复制账本")
    return resolve_storage(backend, rebound["source_registration"], source, state, live_storage=False)


def verify_running_source(backend: Path, start_descriptor: dict, *, live: bool) -> dict:
    from devex_clone_seed_generation_images import verify_image
    from devex_clone_seed_generation_runtime import verify_running_evidence
    from restore_source_lineage import verify_dataset_lineage

    path = bound_file(backend, start_descriptor)
    directory = path.parent.parent
    state_binding = binding(directory / "state.json")
    state = load_state(directory)
    from devex_clone_seed_generation_prelaunch import starts

    records = starts(backend, directory, state["attempts"])
    if (len(records) != 1 or records[0]["status"] != "passed" or records[0]["result"] != start_descriptor
            or path != directory / "results" / f"{records[0]['number']:04d}.json"):
        raise ValueError("source verify 必须绑定同一账本唯一成功 start 的外层收据")
    coordinator_source = require_current_execution_source(backend, records[0]["sources"])
    if live and any(row["number"] > records[0]["number"] and row["stage"] == "seed-runtime" and row["mode"] in {STOP, RECOVER}
                    and row["status"] != "running" for row in state["attempts"]):
        raise ValueError("源运行代次已执行停止或恢复，不能再次验证或重放")
    value = read_json(path)
    exact(value, START_FIELDS)
    output = directory / f"g{records[0]['number']:04d}"
    source = predecessor(backend, directory, state)
    request = read_json(bound_file(backend, value["request"]))
    exact(request, REQUEST_FIELDS)
    prefix = [row for row in state["attempts"] if row["number"] < records[0]["number"]]
    expected = {"status": "seed_source_generation_running", "generation_id": output.name,
                "coordinator_source": coordinator_source,
                "history_length": len(prefix), "history_sha256": plan_hash(prefix),
                "source_registration": source["review_successor"]["source_result"],
                "source_rebind": source["source_rebind"], "review_successor": source["review_successor_binding"],
                "current_storage": source["storage"]["storage"], "remote_writes": 0, "restore_qualified": False}
    if (any(value[key] != item for key, item in expected.items())
            or any(request[key] != value[key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage"))
            or type(request["format_version"]) is not int or request["format_version"] != 1
            or request["kind"] != "devex-clone-seed-source-generation"
            or type(value["remote_writes"]) is not int or value["restore_qualified"] is not False
            or read_json(output / "request.json") != request):
        raise ValueError("运行中源未冻结同一请求、C52、存储或完整前缀")
    name(request["id"])
    for key, filename in (("intent", "intent.json"), ("runtime_evidence", "running-evidence.json"),
                          ("before", "before/image.json"), ("running", "running/image.json"),
                          ("dataset_lineage", "dataset-lineage.json")):
        if bound_file(backend, value[key]) != output / filename:
            raise ValueError("运行中源证据不属于同一固定代次目录")
    selected = {**copy.deepcopy(source["request"]["source"]), "runtime_dir": str(output / "runtime")}
    images = {key: verify_image(backend, value[key], selected, source["request"],
              source_registration=value["source_registration"]) for key in ("before", "running")}
    if images["before"]["image"] != images["running"]["image"]:
        raise ValueError("运行中源启动前后存在数据库、对象或 Redis 漂移")
    lineage = verify_dataset_lineage(backend, source, value["dataset_lineage"], value["before"], images["before"])
    runtime = verify_running_evidence(backend, output, request, selected, source["generation"]["physical_binding"], live=live)
    if live:
        from devex_clone_storage import current_storage_binding

        if current_storage_binding(backend, directory, "target") != value["current_storage"]:
            raise ValueError("源运行中的存储代次变化")
    if binding(path) != start_descriptor or binding(directory / "state.json") != state_binding:
        raise ValueError("源运行收据或账本在复核期间变化")
    require_current_execution_source(backend, coordinator_source)
    return {"receipt": value, "start_descriptor": start_descriptor, "directory": directory, "output": output,
            "request": request, "source": source, "execution": Path(request["execution_backend"]), "selected": selected,
            "runtime": runtime, **images, "lineage": lineage, "coordinator_source": coordinator_source,
            "environment": read_json(bound_file(backend, request["source_environment"]))}


@contextmanager
def registered_running_source(backend: Path, start_descriptor: dict):
    from devex_clone_run import _require_owned_run
    from devex_clone_run_state import run_lock
    from process_environment import Environments, configured

    initial = verify_running_source(backend, start_descriptor, live=True)
    environments = Environments(dict(os.environ), configured(initial["environment"]["environment"]))
    directory = initial["directory"]
    active = True
    with run_lock(directory):
        lock = directory / "run.lock"
        identity = (lock.stat().st_dev, lock.stat().st_ino)

        def checkpoint():
            from devex_clone_model import linked

            if not active or linked(lock) or (lock.stat().st_dev, lock.stat().st_ino) != identity:
                raise ValueError("源验证控制锁身份变化或 checkpoint 已离开控制区间")
            _require_owned_run(directory)
            if dict(os.environ) not in environments.values.values():
                raise ValueError("源验证控制或服务环境发生变化")
            with environments.use("source"):
                current = verify_running_source(backend, start_descriptor, live=True)
            if current != initial:
                raise ValueError("源验证期间运行代次、来源或前像绑定变化")
            return current

        try:
            checkpoint()
            yield checkpoint
        except BaseException as error:
            try:
                checkpoint()
            except BaseException as failed:
                error.add_note("源验证异常后的代次复核失败：" + type(failed).__name__)
            raise
        else:
            checkpoint()
        finally:
            active = False


def _archived_request(backend: Path, archive: dict, request: dict, segment: dict | None) -> None:
    previous = read_json(bound_file(backend, archive["receipt"]["request"]))
    expected = copy.deepcopy(previous)
    if segment is not None:
        if segment["phase"] != "ready" or segment["storage"] is None or segment["cache"] is None:
            raise ValueError("分段资源尚未全部恢复，不能执行实际 START")
        expected["current_storage"] = segment["storage"]
    if request != expected:
        raise ValueError("后续实际启动只允许更新同一请求的当前存储代次")


def _archived_image(archive: dict, current: dict, segment: dict | None) -> None:
    expected = copy.deepcopy(archive["image"])
    if segment is not None:
        rustfs = segment["storage"]["storage"]
        redis = segment["cache"]["redis"]
        old_redis = expected["storage"]["redis"]
        mutable = {"pid", "started", "run_id"}
        if (set(redis) != set(old_redis) or any(redis[key] != value for key, value in old_redis.items()
                                               if key not in mutable)):
            raise ValueError("分段缓存重启改变了配置、端点、二进制或物理目标")
        expected["storage"] = {"rustfs": copy.deepcopy(rustfs), "redis": copy.deepcopy(redis)}
    if current != expected:
        raise ValueError("未启动恢复的新完整逻辑基线之后存在未知写入；禁止启动")


def execute_generation(backend: Path, directory: Path, request_path: Path, number: int, *, run=subprocess.run) -> dict:
    from devex_clone_seed_generation_runtime import GenerationRuntime
    from devex_clone_seed_generation_images import capture_image, verify_image
    from restore_source_lineage import derive_dataset_lineage
    from devex_clone_seed_generation_prelaunch import closed

    original = inputs(backend, directory, request_path, number)
    request, source, prefix = original
    archive = closed(backend, directory, prefix)
    segment = None
    if archive is not None:
        from devex_clone_seed_segment import segmented_resume

        segment = segmented_resume(backend, directory, prefix, archive, request["source_registration"])
        _archived_request(backend, archive, request, segment)
    coordinator_source = require_current_execution_source(backend, load_state(directory)["attempts"][-1]["sources"])
    descriptor = binding(request_path)
    output = local_path(backend, str(directory / f"g{number:04d}"), new=True)
    runtime = GenerationRuntime(backend, directory, output, request, source, run)

    def checkpoint():
        with runtime.control_environment():
            require_current_execution_source(backend, coordinator_source)
            if inputs(backend, directory, request_path, number) != original or binding(request_path) != descriptor:
                raise ValueError("source-generation 执行期间来源、请求或账本前缀发生变化")
        runtime.checkpoint()

    runtime.preflight()
    output.mkdir()
    write_json(output / "request.json", request)
    with runtime.environment() as environment:
        runtime.prepare()
        checkpoint()
        before = capture_image(backend, runtime.execution, runtime.selected, request, source,
                               environment, output / "before", run, control_environment=runtime.control_environment)
        if archive is not None:
            _archived_image(archive, read_json(Path(before["path"]))["image"], segment)
        lineage = derive_dataset_lineage(backend, source, before,
            verify_image(backend, before, runtime.selected, source["request"], source_registration=request["source_registration"]))
        write_json(output / "dataset-lineage.json", lineage)
        try:
            checkpoint()
            runtime.start(checkpoint)
            checkpoint()
            running = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                    environment, output / "running", run, control_environment=runtime.control_environment)
            if read_json(Path(before["path"]))["image"] != read_json(Path(running["path"]))["image"]:
                raise ValueError("源启动完整数据库、保留表、placement、对象或 Redis 前后像变化；禁止发布或重放")
            runtime.running_evidence()
            checkpoint()
            runtime.retain()
        except BaseException as failure:
            try:
                runtime.stop()
                checkpoint()
                after = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                      environment, output / "after", run, control_environment=runtime.control_environment)
                if read_json(Path(before["path"]))["image"] != read_json(Path(after["path"]))["image"]:
                    failure.add_note("失败启动完整后像发生变化，禁止重放")
            except BaseException as error:
                failure.add_note("源代次停止或失败后像复核失败：" + type(error).__name__)
            raise
    return {"status": "seed_source_generation_running", "request": descriptor,
            "coordinator_source": coordinator_source,
            **{key: request[key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage")},
            "history_length": len(prefix), "history_sha256": plan_hash(prefix),
            "generation_id": output.name, "intent": binding(output / "intent.json"),
            "runtime_evidence": binding(output / "running-evidence.json"), "before": before, "running": running,
            "dataset_lineage": binding(output / "dataset-lineage.json"),
            "remote_writes": 0, "restore_qualified": False}


def resolve_generation(backend: Path, descriptor: dict, source: dict, state: dict, *, live: bool) -> dict:
    """只采用唯一成功外层发布；预览验证全部文件绑定，不启动进程或新建证据。"""
    from devex_clone_seed_source import _validate_evidence_bindings

    result = copy.deepcopy(source)
    result.update(source_request=source["registration"]["source_request"],
                  source_environment=source["registration"]["source_environment"],
                  generation_verified=source["registration"]["generation_verified"], source_generation=None)
    records = [item for item in state["attempts"] if (item["stage"], item["mode"]) == ("seed-runtime", STOP)]
    published = [item for item in records if item["status"] == "passed"]
    if not published:
        return result
    from devex_clone_seed_generation_runtime import verify_runtime_evidence
    if len(records) != 1 or len(published) != 1:
        raise ValueError("source-generation 存在重复或失败重放")
    record = published[0]
    path = bound_file(backend, record["result"])
    directory = source["directory"]
    if path != directory / "results" / f"{record['number']:04d}.json":
        raise ValueError("source-generation 外层结果不属于固定 attempt")
    value = read_json(path)
    exact(value, RESULT_FIELDS)
    _validate_evidence_bindings(backend, value)
    facts = verify_running_source(backend, value["start"], live=False)
    request = facts["request"]
    if (value["request"] != facts["receipt"]["request"] or any(facts["source"].get(key) != item for key, item in source.items())
            or value["dataset_lineage"] != facts["receipt"]["dataset_lineage"]):
        raise ValueError("source-generation 请求或原 dataset/C29 来源与同一 start 不同")
    prefix = [item for item in state["attempts"] if item["number"] < record["number"]]
    expected = {"status": "seed_source_generation_published", "source_registration": descriptor,
                "source_rebind": source["source_rebind"], "current_storage": source["storage"]["storage"],
                "history_length": len(prefix), "history_sha256": plan_hash(prefix), "remote_writes": 0,
                "restore_qualified": False}
    if (type(value["remote_writes"]) is not int or value["restore_qualified"] is not False
            or any(value[key] != item for key, item in expected.items())
            or any(value[key] != request[key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage"))):
        raise ValueError("source-generation 未冻结同一 C52、重绑定和完整历史前缀")
    output = facts["output"]
    for key, filename in (("source_request", "source-request.json"), ("generation_verified", "generation-verified.json"),
                          ("runtime_evidence", "runtime-evidence.json"), ("before", "before/image.json"),
                          ("running", "running/image.json"), ("stop_before", "stop-before/image.json"), ("after", "after/image.json")):
        if bound_file(backend, value[key]) != output / filename:
            raise ValueError("source-generation 证据不属于同一固定代次目录")
    if read_json(output / "request.json") != request:
        raise ValueError("source-generation 请求副本与原绑定不同")
    effective = read_json(bound_file(backend, value["source_request"]))
    validate_request(backend, effective)
    if effective != source_request(source, request, output / "runtime"):
        raise ValueError("后继 source request 改变了同一物理 seed 来源或构建绑定")
    from devex_clone_seed_generation_images import verify_image

    from restore_source_runtime import verify_source_runtime

    verified = verify_source_runtime(backend, value["source_runtime"], live=False)
    if (verified["receipt"]["source_generation"] != value["start"]
            or verified["receipt"]["dataset_lineage"] != value["dataset_lineage"]
            or value["before"] != facts["receipt"]["before"] or value["running"] != facts["receipt"]["running"]):
        raise ValueError("源验证收据未绑定同一 start、原始前像和 dataset lineage")
    stop_before, after = (verify_image(backend, value[key], effective["source"], source["request"],
                         source_registration=descriptor) for key in ("stop_before", "after"))
    if stop_before["image"] != after["image"] or stop_before["image"] != verified["after"]["image"]:
        raise ValueError("source verify 后或停止期间完整前后像发生未知变化")
    runtime = verify_runtime_evidence(backend, output, request, effective, live=live)
    generation = read_json(bound_file(backend, value["generation_verified"]))
    expected_generation = {
        "request_sha256": plan_hash(effective), "runtime": read_json(bound_file(backend, effective["runtime"])),
        "source": read_json(bound_file(backend, effective["backend_build"]))["sources"]["full"]["source"]["snapshot"],
        "worktree_fingerprint": effective["worktree_fingerprint"],
        "maintenance": read_json(bound_file(facts["execution"], effective["maintenance_build"])),
        "processes": {role: read_json(bound_file(backend, effective["processes"][role]))["identity"] for role in ("api", "worker")},
        "operator_declared_producers": sorted(read_json(bound_file(backend, effective["producers_registry"]))["processes"], key=lambda item: item["name"]),
        "physical_binding": source["generation"]["physical_binding"],
        "external_writers_discovered": False, "clone_verified": False, "restore_qualified": False}
    if generation != expected_generation or runtime["physical_binding"] != expected_generation["physical_binding"]:
        raise ValueError("后继 generation 证明与有效请求、源码或 runtime 不同")
    result.update(request=effective, source_request=value["source_request"], generation=generation,
                  generation_verified=value["generation_verified"], source_generation=record["result"],
                  source_environment=request["source_environment"],
                  environment=read_json(bound_file(backend, request["source_environment"])))
    return result
