"""消费已审计的启动前失败历史；不执行旧代码，不将缺树自动认定为未启动。"""
from __future__ import annotations

import hashlib
from pathlib import Path
import subprocess

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact, local_path
from devex_clone_run_state import binding, controller_record
from devex_clone_source_proof import bound_file
from full_stack_process import process_identity
from restore_reference_plan import plan_hash
from source_fingerprints import verify_execution_source, require_current_execution_source
from source_inventory import git

# b1e88466 的已审计整树是该失败位置的历史证明，清理前必须核对有效收据引用。
# execute_generation:221 仍在 preflight，222 才创建 g 目录，之后才 prepare/intent/start。
# 这里只验证原 Git 对象，不导入或执行历史实现，也不接受其他版本的失败路径。
AUDITED_TREE = "110383e2c6859b86633c2c3b65b037a23c6367e3"
FRAMES = [
    {"file": "scripts/" + file + ".py", "function": function, "line": line}
    for file, function, line in (
        ("devex_clone_run", "execute", 705), ("devex_clone_seed_runtime", "execute_seed", 341),
        ("devex_clone_seed_generation", "execute_generation", 221),
        ("devex_clone_seed_generation_runtime", "preflight", 183),
        ("devex_clone_seed_generation_runtime", "checkpoint", 192),
        ("devex_provenance", "verify_source", 174), ("source_fingerprints", "reusable_artifact_source", 349),
    )
]
START, RECOVER = "source-generation-start", "source-generation-recover"
STATUS = "seed_source_prelaunch_closed"
FIELDS = {"status", "request", "failed_start", "failure_proof", "history_length", "history_sha256",
          "source_registration", "source_rebind", "review_successor", "current_storage", "coordinator_source",
          "runtime", "before", "after", "historical_image_compared", "remote_writes", "restore_qualified"}


def proof(backend: Path, directory: Path, start: dict) -> dict:
    if ((start["stage"], start["mode"], start["status"], start["result"], start.get("error_type"))
            != ("seed-runtime", START, "failed", None, "ValueError")):
        raise ValueError("未启动证明必须绑定已收尾的原 preflight 失败")
    number = start["number"]
    local_path(backend, str(directory / f"g{number:04d}"), new=True)
    sources = start["sources"]
    verify_execution_source(sources, "原启动控制器")
    snapshot = sources["snapshot"]
    if (not snapshot["clean"] or snapshot["files"]
            or snapshot["patch_sha256"] != hashlib.sha256(b"").hexdigest()):
        raise ValueError("原启动失败没有精确干净的已审计源码")
    tree = git(backend, "rev-parse", snapshot["head"] + "^{tree}").decode().strip()
    if tree != AUDITED_TREE:
        raise ValueError("原启动源码不属于已审计的 preflight 零写入边界")
    running = {**start, "status": "running", "finished_at": None, "error_type": None}
    controller, owner = controller_record(directory, number, running)
    bound_file(backend, controller)
    if type(read_json(Path(controller["path"]))["format_version"]) is not int:
        raise ValueError("原控制器版本必须是整数")
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    exact(owner["identity"], {"pid", "started", "executable"})
    if (type(owner["format_version"]) is not int or owner["format_version"] != 1 or owner["directory"] != str(directory)
            or owner["manifest_sha256"] != binding(directory / "manifest.json")["sha256"]
            or type(owner["identity"]["pid"]) is not int or owner["identity"]["pid"] <= 1
            or not isinstance(owner["identity"]["started"], str) or not owner["identity"]["started"]
            or not isinstance(owner["identity"]["executable"], str) or not owner["identity"]["executable"]):
        raise ValueError("原启动控制器归属不完整")
    path = local_path(backend, str(directory / f"failure-{number:04d}.json"))
    failure, value = binding(path), read_json(path)
    expected = {"format_version": 1, "kind": "devex-stage-failure", "attempt": number,
                "stage": "seed-runtime", "mode": START, "error_type": "ValueError",
                "frames": FRAMES, "controller": controller}
    if value != expected or type(value["format_version"]) is not int:
        raise ValueError("原失败栈没有证明停止于创建目录和 intent 之前")
    if process_identity(owner["identity"]["pid"]) is not None:
        raise ValueError("原启动控制器仍存活或 PID 已复用")
    if binding(path) != failure or controller_record(directory, number, running) != (controller, owner):
        raise ValueError("原启动失败证明在核验期间变化")
    local_path(backend, str(directory / f"g{number:04d}"), new=True)
    return {"start": plan_hash(start), "failure": failure, "controller": controller,
            "source": sources, "tree": tree}


def closed(backend: Path, directory: Path, attempts: list) -> dict | None:
    """只排除由唯一成功恢复收据封存的未启动尝试；不丢弃或重写完整账本。"""
    records = [row for row in attempts if (row["stage"], row["mode"], row["status"])
               == ("seed-runtime", RECOVER, "passed")]
    matches = []
    for row in records:
        value = read_json(bound_file(backend, row["result"]))
        if value.get("status") == STATUS:
            matches.append((row, value))
    if not matches:
        return None
    if len(matches) != 1:
        raise ValueError("未启动尝试不能重复恢复")
    record, value = matches[0]
    exact(value, FIELDS)
    prefix = [row for row in attempts if row["number"] < record["number"]]
    starts = [row for row in prefix if (row["stage"], row["mode"]) == ("seed-runtime", START)]
    if (len(starts) != 1 or not prefix or starts[0] != prefix[-1]
            or record["number"] != starts[0]["number"] + 1
            or value["failed_start"] != starts[0]["number"] or value["failure_proof"] != proof(backend, directory, starts[0])
            or value["history_length"] != len(prefix) or value["history_sha256"] != plan_hash(prefix)
            or value["coordinator_source"] != record["sources"] or value["historical_image_compared"] is not False
            or type(value["remote_writes"]) is not int or value["remote_writes"] != 0
            or value["restore_qualified"] is not False
            or bound_file(backend, record["result"]) != directory / "results" / f"{record['number']:04d}.json"):
        raise ValueError("未启动恢复没有绑定原失败及完整历史前缀")
    request = read_json(bound_file(backend, value["request"]))
    from devex_clone_seed_generation import REQUEST_FIELDS

    exact(request, REQUEST_FIELDS)
    for field in ("source_registration", "source_rebind", "review_successor", "current_storage"):
        if value[field] != request[field]:
            raise ValueError("未启动恢复请求与已发布来源不同")
    registrations = [row for row in prefix if (row["stage"], row["mode"], row["status"])
                     == ("seed-runtime", "source-register", "passed")]
    rebinds = [row for row in prefix if (row["stage"], row["mode"], row["status"])
               == ("seed-runtime", "source-rebind", "passed")]
    if (len(registrations) != 1 or registrations[0]["result"] != value["source_registration"]
            or len(rebinds) != 1 or rebinds[0]["result"] != value["source_rebind"]):
        raise ValueError("未启动恢复改变了唯一 C52 或重绑定")
    rebound = read_json(bound_file(backend, value["source_rebind"]))
    if any(value[key] != rebound[key] for key in ("source_registration", "review_successor", "current_storage")):
        raise ValueError("未启动恢复改变了存储或后继工具关系")
    output = directory / "seed-runtime" / f"attempt-{record['number']:04d}"
    if bound_file(backend, value["runtime"]) != output / "runtime/runtime.json":
        raise ValueError("未启动恢复的采集配置不属于同一新 attempt")
    source_request = read_json(bound_file(backend, read_json(bound_file(backend, value["source_registration"]))["source_request"]))
    selected = {**source_request["source"], "runtime_dir": str(output / "runtime")}
    from devex_clone_seed_generation_images import verify_image

    images = {}
    for field in ("before", "after"):
        if bound_file(backend, value[field]) != output / field / "image.json":
            raise ValueError("未启动恢复当前像不属于同一新 attempt")
        images[field] = verify_image(backend, value[field], selected, source_request,
                                     source_registration=value["source_registration"])
    if images["before"]["image"] != images["after"]["image"]:
        raise ValueError("未启动恢复采集期间完整当前像变化")
    return {"start": starts[0], "recovery": record, "receipt": value, "image": images["after"]["image"]}


def starts(backend: Path, directory: Path, attempts: list) -> list:
    archive = closed(backend, directory, attempts)
    return [row for row in attempts if (row["stage"], row["mode"]) == ("seed-runtime", START)
            and (archive is None or row != archive["start"])]


def prepare(backend: Path, directory: Path, start: dict, request_path: Path, number: int, *, run=subprocess.run):
    from devex_clone_seed_generation import REQUEST_FIELDS, predecessor
    from devex_clone_seed_generation_runtime import GenerationRuntime
    from devex_clone_run_state import load_state
    from reference_fixture_successor_generation import rebuild

    evidence = proof(backend, directory, start)
    descriptor = binding(local_path(backend, str(request_path)))
    request = read_json(request_path)
    exact(request, REQUEST_FIELDS)
    source = predecessor(backend, directory, load_state(directory))
    expected = {"source_registration": source["review_successor"]["source_result"],
                "review_successor": source["review_successor_binding"], "source_rebind": source["source_rebind"],
                "current_storage": source["storage"]["storage"]}
    if any(request[key] != value for key, value in expected.items()) or rebuild(backend, request) != request:
        raise ValueError("未启动恢复必须使用同一 C52 的当前正式请求")
    output = local_path(backend, str(directory / "seed-runtime" / f"attempt-{number:04d}"), new=True)
    runtime = GenerationRuntime(backend, directory, output, request, source, run)
    runtime.preflight()
    if proof(backend, directory, start) != evidence or binding(request_path) != descriptor:
        raise ValueError("未启动恢复准备期间原失败或当前请求变化")
    return evidence, descriptor, request, source, runtime


def execute(backend: Path, directory: Path, request_path: Path, number: int, prefix: list, *, run=subprocess.run) -> dict:
    from devex_clone_seed_generation_control import _active
    from devex_clone_seed_generation_images import capture_image, verify_image
    from devex_clone_run_state import load_state
    from devex_clone_seed_rebind import quiet_producers
    from full_stack_runtime import register_runtime, verify_runtime

    start = prefix[-1]
    evidence, descriptor, request, source, runtime = prepare(backend, directory, start, request_path, number, run=run)
    coordinator = require_current_execution_source(backend, load_state(directory)["attempts"][-1]["sources"])

    def checkpoint():
        with runtime.control_environment():
            require_current_execution_source(backend, coordinator)
            if (_active(directory, number, RECOVER) != prefix or proof(backend, directory, start) != evidence
                    or binding(request_path) != descriptor):
                raise ValueError("未启动恢复期间控制阶段、原失败或当前请求变化")
        runtime.checkpoint()
        with runtime.environment():
            quiet_producers(backend, source)

    checkpoint()
    runtime.output.mkdir()
    runtime.runtime.mkdir()
    binaries = {"ryframe": runtime.build["artifacts"]["api"]["executable"],
                "ryframe-worker": runtime.build["artifacts"]["worker"]["executable"]}
    maintenance = read_json(bound_file(runtime.execution, request["maintenance_build"]))
    binaries.update({"ryframe-" + role: maintenance["artifacts"][role]["executable"] for role in ("reset", "migrate")})
    write_json(runtime.runtime / "binaries.json", binaries)
    with runtime.environment() as environment:
        register_runtime(runtime.execution, runtime.runtime)
        images = {}
        for field in ("before", "after"):
            checkpoint()
            images[field] = capture_image(backend, runtime.execution, runtime.selected, request, source,
                                          environment, runtime.output / field, run, control_environment=runtime.control_environment)
            verify_runtime(runtime.execution, runtime.runtime)
        values = [verify_image(backend, images[field], runtime.selected, source["request"],
                               source_registration=request["source_registration"]) for field in ("before", "after")]
        if values[0]["image"] != values[1]["image"]:
            raise ValueError("未启动恢复采集期间完整当前像变化，不允许后续启动")
        checkpoint()
    return {"status": STATUS, "request": descriptor, "failed_start": start["number"], "failure_proof": evidence,
            "history_length": len(prefix), "history_sha256": plan_hash(prefix), "coordinator_source": coordinator,
            **{key: request[key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage")},
            "runtime": binding(runtime.runtime / "runtime.json"), **images, "historical_image_compared": False,
            "remote_writes": 0, "restore_qualified": False}
