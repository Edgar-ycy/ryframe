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
          "runtime", "before", "after", "historical_image_compared", "remote_writes", "restore_qualified",
          "collection_failure"}

# 已审计 d781c20f 的唯一采集失败：control verify 返回后即退出，尚未采集完整像。
# 原文件摘要是这段历史的必要证据；不接受其他失败、改写的局部树或历史代码执行。
COLLECTION_TREE = "f6312c497c5863c64d53eadfb30ed6d30e0119db"
COLLECTION_FRAMES = [
    {"file": "scripts/" + file + ".py", "function": function, "line": line}
    for file, function, line in (
        ("devex_clone_run", "execute", 717), ("devex_clone_seed_runtime", "execute_seed", 341),
        ("devex_clone_seed_generation_control", "execute_recover", 208),
        ("devex_clone_seed_generation_prelaunch", "execute", 207),
        ("devex_clone_seed_generation_images", "capture_image", 45),
        ("devex_clone_export", "verify_migrations", 70), ("devex_clone_export", "run_cli", 55),
    )
]
COLLECTION_FILES = {
    "before/migrations-control.command.json": "a301844382215c36cf56ae0e6c1359780b2d7e2996f0bd4493152c7d7584f193",
    "before/migrations-control.diagnostic.json": "57b26e9f805c0d4868b088f722e7082b7ca139ec2b2e8c206054e0803edd45b1",
    "before/redis-kernel-092424efb7a3421b9b1bfe1b65fd6263.command.json": "6eafe6904d152d0f4f3f2a386195e2fcd8570ad21c0d54998cd19fb916fefa77",
    "before/redis-kernel-5487f322f1ae4bcfa15b3b5763f4a671.command.json": "5f8e7d6fcc981566200de0ec1cb6f7b8633b347850cbd18671ced250eb4c2740",
    "before/redis-kernel-6e408b07957047fc8654dfc90055e857.command.json": "b940b84cb4b520c8c527cc8a5a5af6b80c58932e080b0a6f3a7ff96a1a7ce3db",
    "before/redis-kernel-f3894f4f3eb94497938d9c85b1393cbd.command.json": "a53d6a73166255c487eb107ef4d5eaf4e39d01c137329fba126b94c2d86575e8",
    "before/redis-kernel-f4a244167c2949d2b189b4091dafee9f.command.json": "ccbd72c2212d39cf1c3efb1ecbba5d101ac5b85661a6e4e6945b54cfc4dab6bb",
    "before/storage-layout-4478cbf34ce04c4ca0eeed96443b42bd.json": "b43d520f943aef98b56ea749f2ae343c0040defd0c4d11edaf19995641ba2a74",
    "runtime/binaries.json": "86fea2999e3c4323569b672c1b7af6310af0aa09d9a83e7c5dafa753de091d3c",
    "runtime/runtime.json": "b292edafceefad1924651fb526026aa63b4d8ee776a3df714963f1cb1db00567",
}
COLLECTION_MAINTENANCE = "86fb5ff5fca2a83a4588f5f52bdf53a2986d7725900f453936ffadeccb401556"
OBSERVATION_TREE = "277a6d8db15e5db1fc4c58645e7885d841dd9080"
OBSERVATION_PRODUCT_TREE = "dad947d8d622264c975aca8fea5fe58e6ae05190"
OBSERVATION_FILES = "68df30b793b0953bbb3bedd4c42df3b8653ed3c73065f5ead29de2892f37e9ed"
OBSERVATION_MAINTENANCE = "f7c39f0e2939bc130a542485066b624362441b3324336dfe7f5197a449f40a5d"
# 后续唯一失败在已登记历史二进制的参数解析中，配置加载、连接和库存输出均未执行。
OBSERVATION_FRAMES = [
    {"file": "scripts/" + file + ".py", "function": function, "line": line}
    for file, function, line in (
        ("devex_clone_run", "execute", 717), ("devex_clone_seed_runtime", "execute_seed", 341),
        ("devex_clone_seed_generation_control", "execute_recover", 209),
        ("devex_clone_seed_generation_prelaunch", "execute", 321),
        ("devex_clone_seed_generation_images", "capture_image", 51),
        ("devex_clone_export", "capture_inventory", 146), ("devex_clone_export", "run_cli", 55),
    )
]


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


def collection_failure(backend: Path, directory: Path, start: dict, failed: dict,
                       previous: dict | None = None) -> dict:
    from restore_source_runtime import _manifest
    from restore_build import file_digest

    original = proof(backend, directory, start)
    prior = None if previous is None else collection_failure(backend, directory, start, previous)
    observed = prior is not None
    tree, frames = (OBSERVATION_TREE, OBSERVATION_FRAMES) if observed else (COLLECTION_TREE, COLLECTION_FRAMES)
    if (failed["number"] != start["number"] + (2 if observed else 1)
            or tuple(failed.get(key) for key in ("stage", "mode", "status", "result", "error_type"))
            != ("seed-runtime", RECOVER, "failed", None, "CalledProcessError")):
        raise ValueError("只允许原未启动失败后的唯一已审计采集失败")
    sources = failed["sources"]
    verify_execution_source(sources, "原采集控制器")
    snapshot = sources["snapshot"]
    if (not snapshot["clean"] or snapshot["files"]
            or snapshot["patch_sha256"] != hashlib.sha256(b"").hexdigest()
            or git(backend, "rev-parse", snapshot["head"] + "^{tree}").decode().strip() != tree):
        raise ValueError("原采集失败没有精确干净的已审计源码")
    number = failed["number"]
    local_path(backend, str(directory / f"g{number:04d}"), new=True)
    running = {**failed, "status": "running", "finished_at": None, "error_type": None}
    controller, owner = controller_record(directory, number, running)
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    exact(owner["identity"], {"pid", "started", "executable"})
    if (type(owner["format_version"]) is not int or owner["format_version"] != 1
            or owner["directory"] != str(directory) or owner["manifest_sha256"] != binding(directory / "manifest.json")["sha256"]
            or type(owner["identity"]["pid"]) is not int or owner["identity"]["pid"] <= 1
            or not isinstance(owner["identity"]["started"], str) or not owner["identity"]["started"]
            or not isinstance(owner["identity"]["executable"], str) or not owner["identity"]["executable"]
            or type(read_json(bound_file(backend, controller))["format_version"]) is not int):
        raise ValueError("原采集控制器归属不完整")
    path = local_path(backend, str(directory / f"failure-{number:04d}.json"))
    failure, value = binding(path), read_json(path)
    if (value != {"format_version": 1, "kind": "devex-stage-failure", "attempt": number,
                  "stage": "seed-runtime", "mode": RECOVER, "error_type": "CalledProcessError",
                  "frames": frames, "controller": controller}
            or type(value["format_version"]) is not int):
        raise ValueError("原采集失败栈不属于已审计只读边界")
    output = local_path(backend, str(directory / "seed-runtime" / f"attempt-{number:04d}"))
    files = _manifest(output)
    files_match = (plan_hash(files) == OBSERVATION_FILES if observed
                   else {item["path"]: item["sha256"] for item in files} == COLLECTION_FILES)
    if not files_match:
        raise ValueError("原采集局部目录不是完整的已审计文件集合")
    maintenance, artifact = _collection_binary(backend, output, sources, observed)
    binary = Path(artifact["executable"])
    if process_identity(owner["identity"]["pid"]) is not None:
        raise ValueError("原采集控制器仍存活或 PID 已复用")
    if (binding(path) != failure or controller_record(directory, number, running) != (controller, owner)
            or _manifest(output) != files or binding(Path(maintenance["path"])) != maintenance
            or file_digest(binary) != {key: artifact[key] for key in ("bytes", "sha256")}
            or proof(backend, directory, start) != original
            or observed and collection_failure(backend, directory, start, previous) != prior):
        raise ValueError("原采集失败证明在核验期间变化")
    local_path(backend, str(directory / f"g{number:04d}"), new=True)
    return {"attempt": plan_hash(failed), "original_start": original, "failure": failure,
            "controller": controller, "source": sources, "tree": tree, "previous": prior,
            "files": files, "maintenance": maintenance, "tenant-data" if observed else "migrate": binding(binary)}


def _collection_binary(backend: Path, output: Path, sources: dict, observed: bool) -> tuple[dict, dict]:
    from restore_build import file_digest

    binaries = read_json(output / "runtime/binaries.json")
    command = read_json(output / ("before/inventory-before.command.json" if observed
                                 else "before/migrations-control.command.json"))
    binary = local_path(backend, command["command"][0] if observed else binaries["ryframe-migrate"])
    maintenance_path = local_path(backend, str(binary.parent / "build.json"))
    maintenance = binding(maintenance_path)
    receipt = read_json(maintenance_path)
    artifact = receipt["artifacts"]["tenant-data" if observed else "migrate"]
    expected_sha = OBSERVATION_MAINTENANCE if observed else COLLECTION_MAINTENANCE
    if (maintenance["sha256"] != expected_sha or artifact["executable"] != str(binary)
            or file_digest(binary) != {key: artifact[key] for key in ("bytes", "sha256")}):
        raise ValueError("原采集命令没有绑定同源只读维护二进制")
    if not observed:
        if (receipt["source"] != {key: sources[key] for key in ("snapshot", "worktree_fingerprint")}
                or command != {"command": [str(binary), "control", "verify"], "cwd": str(backend), "remote_operations": "read_only"}):
            raise ValueError("原 control verify 不属于同一来源和命令")
    else:
        snapshot = receipt["source"]["snapshot"]
        diagnostic = read_json(output / "before/inventory-before.diagnostic.json")
        if (not snapshot["clean"] or snapshot["files"] or snapshot["patch_sha256"] != hashlib.sha256(b"").hexdigest()
                or git(backend, "rev-parse", snapshot["head"] + "^{tree}").decode().strip() != OBSERVATION_PRODUCT_TREE
                or command != {"command": [str(binary), "backup-inventory", "--output", str(output / "before/inventory-before.json"),
                    "--source-sha", snapshot["head"], "--observed-at", command["command"][-1]],
                    "cwd": receipt["backend_root"], "remote_operations": "read_only"}
                or diagnostic["error_type"] != "CalledProcessError" or diagnostic["returncode"] != 1
                or diagnostic["stdout"] != "" or not diagnostic["stderr"].startswith('Error: Validation("缺少 --quiesced-at\\n')):
            raise ValueError("原库存失败没有证明停止于已审计二进制的参数解析")
    return maintenance, artifact


def origin(backend: Path, directory: Path, prefix: list) -> tuple[dict, dict | None]:
    records = [row for row in prefix if (row["stage"], row["mode"]) == ("seed-runtime", START)]
    if len(records) != 1:
        raise ValueError("未启动恢复必须继承唯一原 START")
    start = records[0]
    suffix = prefix[prefix.index(start):]
    if suffix == [start]:
        proof(backend, directory, start)
        return start, None
    if any(row.get("number") != start["number"] + index
           or tuple(row.get(key) for key in ("stage", "mode", "status", "result", "error_type"))
           != ("seed-runtime", RECOVER, "failed", None, "CalledProcessError")
           for index, row in enumerate(suffix[1:], 1)):
        raise ValueError("未启动恢复历史包含其他阶段或不明失败")
    if len(suffix) == 2:
        return start, collection_failure(backend, directory, start, suffix[1])
    if len(suffix) == 3:
        return start, collection_failure(backend, directory, start, suffix[2], previous=suffix[1])
    raise ValueError("未启动恢复历史包含其他阶段或重复采集")


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
    start, collection = origin(backend, directory, prefix)
    if (record["number"] != prefix[-1]["number"] + 1 or value["collection_failure"] != collection
            or value["failed_start"] != start["number"] or value["failure_proof"] != proof(backend, directory, start)
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
    records = (*prefix[prefix.index(start):], record)
    return {"start": start, "recovery": record, "records": records, "receipt": value, "image": images["after"]["image"]}


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
    state = load_state(directory)
    prefix = state["attempts"]
    if prefix[-1]["number"] == number and prefix[-1]["status"] == "running":
        prefix = prefix[:-1]
    if origin(backend, directory, prefix)[0] != start or number != prefix[-1]["number"] + 1:
        raise ValueError("未启动恢复没有绑定原失败和唯一当前采集阶段")
    source = predecessor(backend, directory, state)
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

    start, collection = origin(backend, directory, prefix)
    evidence, descriptor, request, source, runtime = prepare(backend, directory, start, request_path, number, run=run)
    coordinator = require_current_execution_source(backend, load_state(directory)["attempts"][-1]["sources"])

    def checkpoint():
        with runtime.control_environment():
            require_current_execution_source(backend, coordinator)
            if (_active(directory, number, RECOVER) != prefix or proof(backend, directory, start) != evidence
                    or binding(request_path) != descriptor or origin(backend, directory, prefix)[1] != collection):
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
            "collection_failure": collection,
            "history_length": len(prefix), "history_sha256": plan_hash(prefix), "coordinator_source": coordinator,
            **{key: request[key] for key in ("source_registration", "source_rebind", "review_successor", "current_storage")},
            "runtime": binding(runtime.runtime / "runtime.json"), **images, "historical_image_compared": False,
            "remote_writes": 0, "restore_qualified": False}
