"""解析同一 run 内不可变 post-copy 登记代次；不生成可变当前指针。"""
from dataclasses import dataclass
from pathlib import Path
import copy
import re

from devex_clone_capture import read_json
from devex_clone_model import exact, linked
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash

AMEND_FIELDS = {"format_version", "kind", "sequence", "run_manifest", "predecessor", "request_sha256",
                "api_environment", "invalidated_runtime"}
RESULT_FIELDS = {"status", "registration", "sequence", "predecessor", "runtime_directory", "remote_writes",
                 "worker_must_remain_stopped", "restore_qualified"}


@dataclass(frozen=True)
class PostRegistration:
    request: dict
    descriptor: dict
    sequence: int
    runtime: Path


def _base(backend: Path, directory: Path, state: dict) -> PostRegistration:
    path = directory / "post-copy.json"
    records = [item for item in state["attempts"] if item["stage"] == "post-copy" and item["mode"] == "register"]
    passed = [item for item in records if item["status"] == "passed"]
    if len(passed) != 1:
        raise ValueError("post-copy 原始登记必须恰有一个发布成功结果")
    result = read_json(bound_file(backend, passed[0]["result"]))
    descriptor = binding(path)
    if result.get("status") != "post_copy_registered" or result.get("registration") != descriptor:
        raise ValueError("post-copy 原始登记与发布结果不同")
    return PostRegistration(read_json(path), descriptor, 0, directory / "post-copy/runtime")


def _invalidation(backend: Path, directory: Path, state: dict, previous: PostRegistration, value: dict) -> None:
    exact(value, {"prepare", "handoff", "runtime", "binaries", "api_process", "producer_history",
                  "failed_start", "stopped"})
    paths = {"handoff": previous.runtime / "handoff.json", "runtime": previous.runtime / "runtime.json",
             "binaries": previous.runtime / "binaries.json", "api_process": previous.runtime / "api.json",
             "producer_history": previous.runtime / "producer-history.json"}
    for field, path in paths.items():
        if bound_file(backend, value[field]) != path:
            raise ValueError("post-copy amendment 的旧 runtime 证据路径无效")
    exact(value["failed_start"], {"attempt", "controller", "failure"})
    exact(value["stopped"], {"attempt", "result"})
    failed, stopped = value["failed_start"]["attempt"], value["stopped"]["attempt"]
    attempts = {item["number"]: item for item in state["attempts"]}
    prepares = [item for item in state["attempts"] if item["stage"] == "post-copy" and item["mode"] == "prepare"
                and item["status"] == "passed" and item["result"] == value["prepare"]]
    if (type(failed) is not int or type(stopped) is not int or failed <= 0 or stopped <= failed
            or len(prepares) != 1 or attempts.get(failed, {}).get("stage") != "runtime-target"
            or attempts.get(failed, {}).get("mode") != "start" or attempts.get(failed, {}).get("status") != "failed"
            or attempts.get(stopped, {}).get("stage") != "runtime-target"
            or attempts.get(stopped, {}).get("mode") != "stop" or attempts.get(stopped, {}).get("status") != "passed"
            or attempts[stopped]["result"] != value["stopped"]["result"]
            or bound_file(backend, value["prepare"]).parent != directory / "results"
            or bound_file(backend, value["failed_start"]["controller"]) != directory / f"controller-{failed:04d}.json"
            or bound_file(backend, value["failed_start"]["failure"]) != directory / f"failure-{failed:04d}.json"
            or bound_file(backend, value["stopped"]["result"]) != directory / "results" / f"{stopped:04d}.json"):
        raise ValueError("post-copy amendment 的旧阶段证据链无效")


def _amendments(backend: Path, directory: Path, state: dict, base: PostRegistration, *, cleanup: bool) -> list[PostRegistration]:
    records = [item for item in state["attempts"] if item["stage"] == "post-copy" and item["mode"] == "amend"
               and item["status"] == "passed"]
    values, previous = [], base
    for expected, record in enumerate(records, 1):
        result = read_json(bound_file(backend, record["result"]))
        exact(result, RESULT_FIELDS)
        descriptor = result.get("registration")
        if not isinstance(descriptor, dict):
            raise ValueError("post-copy amendment 发布结果缺少登记绑定")
        path = bound_file(backend, descriptor)
        amendment = read_json(path)
        exact(amendment, AMEND_FIELDS)
        expected_path = directory / "post-copy/amendments" / f"{expected:04d}.json"
        if (path != expected_path or linked(path) or amendment["format_version"] != 1
                or amendment["kind"] != "devex-clone-post-copy-amendment" or amendment["sequence"] != expected
                or amendment["run_manifest"] != binding(directory / "manifest.json")
                or amendment["predecessor"] != previous.descriptor
                or result.get("status") != "post_copy_amended" or result.get("sequence") != expected
                or result.get("predecessor") != previous.descriptor
                or result.get("runtime_directory") != str(directory / "post-copy" / f"runtime-{expected:04d}")
                or result.get("remote_writes") != 0 or result.get("worker_must_remain_stopped") is not True
                or result.get("restore_qualified") is not False):
            raise ValueError("post-copy amendment 顺序、来源或发布结果无效")
        if not cleanup:
            _invalidation(backend, directory, state, previous, amendment["invalidated_runtime"])
            bound_file(backend, amendment["api_environment"])
        request = copy.deepcopy(previous.request)
        request["api_environment"] = amendment["api_environment"]
        if plan_hash(request) != amendment["request_sha256"]:
            raise ValueError("post-copy amendment 不能还原完整修正请求")
        previous = PostRegistration(request, descriptor, expected,
                                    directory / "post-copy" / f"runtime-{expected:04d}")
        values.append(previous)
    return values


def resolve_post_registration(backend: Path, directory: Path, *, descriptor: dict | None = None,
                              current: int | None = None, cleanup: bool = False) -> PostRegistration:
    state = load_state(directory, verify_results=not cleanup)
    base = _base(backend, directory, state)
    amendments = _amendments(backend, directory, state, base, cleanup=cleanup)
    candidates = [base, *amendments]
    attempts = [item for item in state["attempts"] if item["stage"] == "post-copy"
                and item["mode"] in {"register", "amend"} and item["number"] != current]
    if descriptor is not None:
        found = [item for item in candidates if item.descriptor == descriptor]
        if len(found) != 1:
            raise ValueError("历史 post-copy 登记不属于本 run 的已发布代次")
        if not cleanup and (not attempts or attempts[-1]["status"] != "passed" or found[0] != candidates[-1]):
            raise ValueError("普通操作只能使用最新通过的 post-copy 登记")
        return found[0]
    if not cleanup and (not attempts or attempts[-1]["status"] != "passed"):
        raise ValueError("最新 post-copy 登记或 amendment 未通过，禁止回退")
    if len(amendments) > 9999 or any(not re.fullmatch(r"[0-9]{4}\.json", Path(item.descriptor["path"]).name)
                                     for item in amendments):
        raise ValueError("post-copy amendment 代次无效")
    return candidates[-1]
