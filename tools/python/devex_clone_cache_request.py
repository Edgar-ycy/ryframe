"""缓存请求所需的配置、历史与工具 I/O；纯模型统一在 Rust 缓存协议校验。"""
from __future__ import annotations
from pathlib import Path
import shlex

from devex_clone_cache_native import invoke
from devex_clone_capture import read_json
from devex_clone_factory_context import initialization_history
from devex_clone_model import local_path
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_storage_request import directory_identity
from devex_clone_target_binding import external_file, pending_request_binding, request_binding
from rate_limit_client import load_app_table


def environment(backend, request):
    from devex_clone_cache_process import validate
    private = read_json(bound_file(backend, request["target_environment"]))
    value = invoke("environment-select", private=private)
    validate(request["process"], files=False)
    redis = load_app_table(backend, value).get("redis")
    invoke("environment-model", request=request, redis=redis)
    return value


def process_binding(backend, request, review, private, *, effective=None, observe_effective_files=True):
    from devex_clone_cache_process import validate
    previous, process = request["previous"], request["process"]
    validate(process, files=effective is None)
    if effective is not None:
        validate(effective, files=observe_effective_files)
    receipt = read_json(bound_file(backend, request["process_receipt"]))
    directory = directory_identity(backend, process["directory"])
    local_path(backend, review["services"]["redis"]["directory"])
    config = bound_file(backend, previous["configuration"])
    if effective is None:
        external_file(previous["wsl"])
    options = [shlex.split(line) for line in config.read_text(encoding="utf-8").splitlines() if line.strip()]
    invoke("process-model", request=request, review=review, receipt=receipt,
           directory=str(directory), config=str(config), options=options)


def _validate_request(backend, directory, value, request, bind_request, *, owned_lock_identity=None,
                      effective=None, initialization=None, observe_effective_files=True):
    invoke("request-head", request=request, manifest=binding(directory / "manifest.json"), value=value)
    filename = bound_file(backend, request["initialized"])
    initial, original = (initialization_history(backend, filename, owned_lock_identity)
                         if initialization is None else initialization)
    review, selected = bind_request(backend, original)
    invoke("request-model", request=request, initial=initial, original=original, selected=selected)
    private = environment(backend, request)
    process_binding(backend, request, review, private, effective=effective,
                    observe_effective_files=observe_effective_files)
    return private


def validate_request(backend: Path, directory: Path, value: dict, request: dict, *, owned_lock_identity=None) -> dict:
    return _validate_request(backend, directory, value, request, request_binding,
                             owned_lock_identity=owned_lock_identity)


def successor_process(backend, directory, value, request, successor, *, owned_lock_identity=None,
                      observe_current_tools=True):
    from devex_clone_seed_source import _registered_source
    from reference_fixture_environment import validate_current_review_tools, validated_recorded_review_tools
    from reference_fixture_successor import _source_with_loader
    source = _source_with_loader(backend, successor, live_storage=False, loader=_registered_source)
    relationship = source["review_successor"]
    review = read_json(bound_file(backend, {key: item for key, item in relationship["successor_review"].items()
                                          if key != "canonical_sha256"}))
    tools = (validate_current_review_tools(review) if observe_current_tools
             else validated_recorded_review_tools(review))
    effective = invoke("successor-model", request=request, directory=str(directory), value=value,
        source_directory=str(source["directory"]), source_manifest=source["manifest"], tools=tools,
        tool_wsl_path=str(Path(tools["wsl"]["path"]).resolve()),
        original_wsl_path=str(Path(request["process"]["wsl"]["path"]).resolve()))
    def bind_pending(root, target):
        invoke("pending-target", target=target, seed_target=source["seed_target"])
        return pending_request_binding(root, target, relationship["predecessor_review"])
    private = _validate_request(backend, directory, value, request, bind_pending,
                                 owned_lock_identity=owned_lock_identity, effective=effective,
                                 initialization=(source["initialization"], source["seed_target"]),
                                 observe_effective_files=observe_current_tools)
    return effective, private, source
