"""从固定 C69 最终复核失败只读重算一次 C70 启动授权。"""

from __future__ import annotations

import hashlib
from pathlib import Path
import subprocess

from devex_clone_capture import read_json
from devex_clone_model import exact, linked, local_path
from devex_clone_run_state import binding, controller_record, load_state
from devex_clone_source_proof import bound_file, require_recorded_producer_stopped
from devex_clone_storage_request import identity
from source_fingerprints import verify_execution_source
from source_inventory import git


ATTEMPT = 69
HEAD = "e57703a1ccd1282d703ec9ee79f22c235093d4eb"
TREE = "dfd6ecf6b041f51a5c3ea0f0e9d286f38349b9e4"
REQUEST_RELATIVE = Path(
    ".local-tests/stable-readiness/source-r26-current/"
    "generation-request-observed-b9af49bd-current-20260916-r3.json"
)
REQUEST_BYTES = 3_724
REQUEST_SHA256 = "93be10307c83a794362c74dad7d2be84ed8e6138aff92b6e282127de2a514c4c"
MANIFEST = {
    "files": 10_231,
    "bytes": 1_081_072_371,
    "sha256": "fe2100c6190188e5a66ada28f8ce0359786c798aa4ea8a75aab43152b26d8635",
}
FRAMES = [
    {"file": "scripts/" + file + ".py", "function": function, "line": line}
    for file, function, line in (
        ("devex_clone_run", "execute", 717),
        ("devex_clone_seed_runtime", "execute_seed", 341),
        ("devex_clone_seed_generation_control", "execute_recover", 220),
        ("devex_clone_seed_generation_lineage_recovery", "authorize", 475),
        ("devex_clone_seed_generation_lineage_recovery", "lineage_failure", 384),
        ("devex_clone_seed_generation_lineage_recovery", "_verify_request", 280),
        ("devex_clone_seed_generation_runtime", "registered_inputs", 63),
        ("devex_provenance", "verify_source", 174),
        ("source_fingerprints", "reusable_artifact_source", 349),
    )
]
STABLE_FACTS = (
    "proof",
    "failed",
    "request",
    "historical_request",
    "request_descriptor",
    "source",
    "contract",
    "before",
    "image",
    "records",
)


def pending(attempts: list) -> bool:
    """识别已经收尾的固定 C69 失败；完整授权仍由 authority 重算。"""
    return bool(
        attempts
        and attempts[-1].get("number") == ATTEMPT
        and tuple(
            attempts[-1].get(key)
            for key in ("stage", "mode", "status", "result", "error_type")
        )
        == ("seed-runtime", "source-generation-recover", "failed", None, "ValueError")
    )


def completed(backend: Path, directory: Path, attempts: list) -> dict | None:
    """C70 已收尾后只采用它绑定的 r3；后续生命周期不重复扫描 1 GiB C69。"""
    matches = [
        (index, row)
        for index, row in enumerate(attempts)
        if row.get("number") == ATTEMPT
        and (row.get("stage"), row.get("mode"))
        == ("seed-runtime", "source-generation-recover")
    ]
    if len(matches) != 1 or matches[0][0] + 1 >= len(attempts):
        return None
    position, record = matches[0]
    previous, successor = attempts[position - 1], attempts[position + 1]
    if (
        tuple(record.get(key) for key in ("status", "result", "error_type"))
        != ("failed", None, "ValueError")
        or type(previous.get("number")) is not int
        or previous["number"] != ATTEMPT - 1
        or tuple(
            previous.get(key)
            for key in ("stage", "mode", "status", "result", "error_type")
        )
        != ("seed-runtime", "source-generation-start", "failed", None, "ValueError")
        or type(successor.get("number")) is not int
        or tuple(successor.get(key) for key in ("number", "stage", "mode"))
        != (ATTEMPT + 1, "seed-runtime", "source-generation-start")
    ):
        raise ValueError("C70 已收尾历史没有继承固定 C68/C69")
    if successor.get("status") == "running":
        return None
    result = successor.get("result")
    if (
        not isinstance(successor.get("finished_at"), str)
        or not successor["finished_at"]
        or successor.get("status") == "passed"
        and (not isinstance(result, dict) or successor.get("error_type") is not None)
        or successor.get("status") == "failed"
        and (result is not None or not isinstance(successor.get("error_type"), str))
        or successor.get("status") not in {"passed", "failed"}
    ):
        raise ValueError("C70 已收尾状态、结果或失败类型无效")
    state_descriptor = binding(directory / "state.json")
    if load_state(directory)["attempts"] != attempts:
        raise ValueError("C70 已收尾历史不是当前完整账本")
    request_path, request_descriptor = _fixed_request(backend)
    result_descriptor = None
    if successor["status"] == "passed":
        result_path = bound_file(backend, result)
        result_descriptor = binding(result_path)
        if read_json(result_path).get("request") != request_descriptor:
            raise ValueError("C70 成功结果没有绑定 C69 授权的同一请求")
    else:
        request_copy = directory / "g0070/request.json"
        if request_copy.exists() and read_json(request_copy) != read_json(request_path):
            raise ValueError("C70 失败目录没有绑定 C69 授权的同一请求")
    if (
        binding(directory / "state.json") != state_descriptor
        or load_state(directory)["attempts"] != attempts
        or _fixed_request(backend)[1] != request_descriptor
        or result_descriptor is not None
        and binding(Path(result_descriptor["path"])) != result_descriptor
    ):
        raise ValueError("C70 已收尾历史或请求在核验期间变化")
    return {
        "record": record,
        "failed": previous,
        "records": (previous, record),
        "request": request_descriptor,
        "receipt": None,
        "completed_successor": successor,
    }


def _incident(attempts: list) -> tuple[list, dict, dict] | None:
    matches = [
        (index, row)
        for index, row in enumerate(attempts)
        if row.get("number") == ATTEMPT
        and (row.get("stage"), row.get("mode"))
        == ("seed-runtime", "source-generation-recover")
    ]
    if not matches:
        return None
    if len(matches) != 1:
        raise ValueError("C69 谱系恢复不能重复")
    position, record = matches[0]
    if tuple(record.get(key) for key in ("status", "result", "error_type")) != (
        "failed",
        None,
        "ValueError",
    ):
        return None
    if (
        type(record.get("number")) is not int
        or position == 0
        or type(attempts[position - 1].get("number")) is not int
        or record["number"] != attempts[position - 1]["number"] + 1
    ):
        raise ValueError("C69 失败授权不是 C68 后的唯一相邻阶段")
    suffix = attempts[position + 1 :]
    if suffix and (
        len(suffix) != 1
        or type(suffix[0].get("number")) is not int
        or tuple(
            suffix[0].get(key)
            for key in (
                "number",
                "stage",
                "mode",
                "status",
                "finished_at",
                "result",
                "error_type",
            )
        )
        != (70, "seed-runtime", "source-generation-start", "running", None, None, None)
    ):
        raise ValueError("C69 后只允许唯一未收尾的 C70 START")
    return attempts[: position + 1], record, attempts[position - 1]


def _state(directory: Path, prefix: list) -> tuple[dict, dict]:
    descriptor = binding(directory / "state.json")
    current = load_state(directory)
    rows = current["attempts"]
    if rows == prefix:
        return descriptor, current
    successor = rows[-1] if len(rows) == len(prefix) + 1 and rows[:-1] == prefix else None
    if (
        successor is None
        or type(successor.get("number")) is not int
        or tuple(
            successor.get(key)
            for key in (
                "number",
                "stage",
                "mode",
                "status",
                "finished_at",
                "result",
                "error_type",
            )
        )
        != (70, "seed-runtime", "source-generation-start", "running", None, None, None)
    ):
        raise ValueError("C69 只读授权没有绑定当前账本或相邻 C70 START")
    from devex_clone_run import _require_owned_run

    _require_owned_run(directory)
    return descriptor, current


def _fixed_request(backend: Path) -> tuple[Path, dict]:
    request_path = local_path(backend, str(backend / REQUEST_RELATIVE))
    descriptor = binding(request_path)
    if descriptor != {
        "path": str(request_path),
        "bytes": REQUEST_BYTES,
        "sha256": REQUEST_SHA256,
    }:
        raise ValueError("C69 失败授权绑定的正式请求变化")
    return request_path, descriptor


def _source(backend: Path, record: dict, previous: dict, facts: dict) -> None:
    if previous != facts["failed"]:
        raise ValueError("C69 失败授权没有绑定固定 C68")
    verify_execution_source(record["sources"], "C69 失败控制器")
    snapshot = record["sources"]["snapshot"]
    if (
        snapshot["head"] != HEAD
        or git(backend, "rev-parse", HEAD + "^{tree}").decode().strip() != TREE
        or not snapshot["clean"]
        or snapshot["files"]
        or snapshot["patch_sha256"] != hashlib.sha256(b"").hexdigest()
        or record["sources"]["fingerprints"]["product"]
        != previous["sources"]["fingerprints"]["product"]
    ):
        raise ValueError("C69 失败授权改变了产品来源或不是固定干净提交")


def _controller(directory: Path, record: dict) -> tuple[dict, dict, dict]:
    running = {
        **record,
        "status": "running",
        "finished_at": None,
        "error_type": None,
    }
    controller, owner = controller_record(directory, ATTEMPT, running)
    exact(owner, {"format_version", "identity", "directory", "manifest_sha256"})
    owner_identity = identity(owner["identity"])
    if (
        type(owner["format_version"]) is not int
        or owner["format_version"] != 1
        or owner["directory"] != str(directory)
        or owner["manifest_sha256"] != binding(directory / "manifest.json")["sha256"]
    ):
        raise ValueError("C69 失败控制器不属于固定复制运行")
    require_recorded_producer_stopped(owner_identity)
    return controller, owner, running


def _failure(backend: Path, directory: Path, controller: dict) -> tuple[Path, dict]:
    failure_path = local_path(backend, str(directory / "failure-0069.json"))
    descriptor = binding(failure_path)
    value = read_json(failure_path)
    expected = {
        "format_version": 1,
        "kind": "devex-stage-failure",
        "attempt": ATTEMPT,
        "stage": "seed-runtime",
        "mode": "source-generation-recover",
        "error_type": "ValueError",
        "frames": FRAMES,
        "controller": controller,
    }
    if (
        type(value.get("format_version")) is not int
        or type(value.get("attempt")) is not int
        or value != expected
    ):
        raise ValueError("C69 失败栈不是完整后像后的固定环境复核位置")
    for absent in (directory / "results/0069.json", directory / "g0069"):
        local_path(backend, str(absent), new=True)
    return failure_path, descriptor


def _binaries(backend: Path, request: dict) -> dict:
    build = read_json(bound_file(backend, request["backend_build"]))
    execution = local_path(backend, request["execution_backend"])
    maintenance = read_json(bound_file(execution, request["maintenance_build"]))
    return {
        "ryframe": build["artifacts"]["api"]["executable"],
        "ryframe-worker": build["artifacts"]["worker"]["executable"],
        **{
            "ryframe-" + role: maintenance["artifacts"][role]["executable"]
            for role in ("reset", "migrate")
        },
    }


def _layout(output: Path, runtime_directory: Path) -> None:
    if linked(output) or not output.is_dir():
        raise ValueError("C69 失败目录包含启动证据、未知文件或不同内容")
    children = list(output.iterdir())
    if (
        {item.name for item in children} != {"runtime", "after"}
        or any(linked(item) or not item.is_dir() for item in children)
        or linked(runtime_directory)
        or not runtime_directory.is_dir()
        or tuple(sorted(item.name for item in runtime_directory.iterdir()))
        != ("binaries.json", "runtime.json")
    ):
        raise ValueError("C69 失败目录包含启动证据、未知文件或不同内容")


def _output(backend: Path, directory: Path, facts: dict, *, run) -> dict:
    from devex_clone_seed_generation_images import verify_image
    from devex_clone_seed_generation_lineage_recovery import _manifest_summary
    from devex_clone_seed_generation_runtime import GenerationRuntime
    from full_stack_runtime import verify_runtime

    output = local_path(backend, str(directory / "seed-runtime/attempt-0069"))
    runtime_directory = output / "runtime"
    after_path = output / "after/image.json"
    _layout(output, runtime_directory)
    if (
        read_json(runtime_directory / "binaries.json") != _binaries(backend, facts["request"])
        or _manifest_summary(output) != MANIFEST
    ):
        raise ValueError("C69 失败目录包含启动证据、未知文件或不同内容")
    runtime = GenerationRuntime(
        backend,
        directory,
        output,
        facts["request"],
        facts["source"],
        run,
    )
    runtime.contract = read_json(runtime_directory / "runtime.json")
    runtime.operations = {"api": "0" * 32, "worker": "1" * 32}
    with runtime.environment():
        if verify_runtime(runtime.execution, runtime_directory) != facts["contract"]:
            raise ValueError("C69 失败运行配置与 C68 不同")
    after = verify_image(
        backend,
        binding(after_path),
        runtime.selected,
        facts["source"]["request"],
        source_registration=facts["request"]["source_registration"],
    )
    if after["image"] != facts["image"]:
        raise ValueError("C69 完整后像与 C68 启动前像不同")
    return {
        "directory": output,
        "runtime_path": runtime_directory / "runtime.json",
        "after_path": after_path,
        "runtime": binding(runtime_directory / "runtime.json"),
        "after": binding(after_path),
        "image": after["image"],
    }


def _stable_facts(facts: dict) -> dict:
    return {key: facts[key] for key in STABLE_FACTS}


def _unchanged(
    backend: Path,
    directory: Path,
    prefix: list,
    state_evidence: tuple[dict, dict],
    request_evidence: tuple[Path, dict],
    control_evidence: tuple[dict, dict, dict],
    failure_evidence: tuple[Path, dict],
    output: dict,
) -> None:
    from devex_clone_seed_generation_lineage_recovery import _manifest_summary

    request_path, request_descriptor = request_evidence
    controller, owner, running = control_evidence
    failure_path, failure = failure_evidence
    current_state = _state(directory, prefix)
    _layout(output["directory"], output["runtime_path"].parent)
    for absent in (directory / "results/0069.json", directory / "g0069"):
        local_path(backend, str(absent), new=True)
    if (
        current_state != state_evidence
        or binding(request_path) != request_descriptor
        or controller_record(directory, ATTEMPT, running) != (controller, owner)
        or binding(failure_path) != failure
        or _manifest_summary(output["directory"]) != MANIFEST
        or binding(output["runtime_path"]) != output["runtime"]
        or binding(output["after_path"]) != output["after"]
    ):
        raise ValueError("C69 失败授权的账本、请求或完整证据在核验期间变化")


def authority(
    backend: Path, directory: Path, attempts: list, *, run=subprocess.run
) -> dict | None:
    """精确证据不完整时失败关闭；不创建、改写或删除任何文件。"""
    incident = _incident(attempts)
    if incident is None:
        return None
    prefix, record, previous = incident
    state_evidence = _state(directory, prefix)
    request_evidence = _fixed_request(backend)
    request_path, request_descriptor = request_evidence
    from devex_clone_seed_generation_lineage_recovery import lineage_failure

    facts = lineage_failure(
        backend,
        directory,
        prefix,
        request_path=request_path,
        run=run,
        require_idle=False,
    )
    _source(backend, record, previous, facts)
    control_evidence = _controller(directory, record)
    controller, _, _ = control_evidence
    failure_evidence = _failure(backend, directory, controller)
    output = _output(backend, directory, facts, run=run)

    repeated = lineage_failure(
        backend,
        directory,
        prefix,
        request_path=request_path,
        run=run,
        require_idle=False,
    )
    if _stable_facts(repeated) != _stable_facts(facts):
        raise ValueError("C69 核验期间 C68 失败、来源或完整前像变化")
    _, failure = failure_evidence
    proof = {
        "status": "seed_source_generation_replay_recovered",
        "request": request_descriptor,
        "failed_start": facts["failed"],
        "failed_authorization": record,
        "failure": failure,
        "controller": controller,
        "runtime": output["runtime"],
        "after": output["after"],
        "evidence_manifest": MANIFEST,
        "remote_writes": 0,
        "restore_qualified": False,
    }
    _unchanged(
        backend,
        directory,
        prefix,
        state_evidence,
        request_evidence,
        control_evidence,
        failure_evidence,
        output,
    )
    return {
        "record": record,
        "failed": facts["failed"],
        "records": (facts["failed"], record),
        "request": request_descriptor,
        "receipt": None,
        "image": output["image"],
        "failure": facts,
        "recovered_failure": proof,
    }
