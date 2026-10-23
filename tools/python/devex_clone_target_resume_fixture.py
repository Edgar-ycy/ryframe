"""迁移续作的本地服务账本夹具；只发布文件，不启动服务。"""
import copy
from contextlib import contextmanager
import os
from pathlib import Path

from devex_clone_capture import read_json, write_json
from devex_clone_run_state import begin, binding, finish, initialize_state, load_state


def restart(f):
    run = f.local / "service-run"
    run.mkdir()

    def publish(path, value):
        path.parent.mkdir(parents=True, exist_ok=True)
        write_json(path, value)
        return binding(path)

    publish(run / "manifest.json", {
        "format_version": 1, "kind": "reference-fixture-service-run",
        "review": binding(f.local / "review.json"), "bootstrap": {},
        "execution_backend": str(f.root), "scope_id": f.scope,
        "data_directory_was_empty": True,
    })
    initialize_state(run)
    original = copy.deepcopy(f.request["storage"])
    directory = Path(f.review["services"]["rustfs"]["data_dir"])
    storage_request = {
        "data_directory": {"path": str(directory), "device": directory.stat().st_dev,
                           "inode": directory.stat().st_ino},
        "api_url": "http://127.0.0.1:29200", "console_url": "http://127.0.0.1:29201",
    }
    publish(run / "rustfs/request.json", storage_request)
    redis_request = {"wsl": original["redis"]["wsl"]}
    publish(run / "redis/request.json", redis_request)
    for stage, mode, result in (
            ("storage-target", "initial", original["rustfs"]),
            ("cache-target", "initial", {"runtime": {"redis": original["redis"]}}),
            ("fixture-buckets", "prepare", {})):
        number = begin(run, stage, mode, {})
        finish(run, number, result=result)
    initial = load_state(run)
    previous = {"kind": "initial", "rustfs": initial["attempts"][0]["result"],
                "redis": initial["attempts"][1]["result"]}
    tree = {"format_version": 2, "runtime_directory": str(run / "rustfs"),
            "operation_id": "b" * 32, "role": "rustfs", "scope_id": f.scope,
            "supervisor": {"pid": 100}, "monitor": {"pid": 101}, "process": {"pid": 102}}
    members = publish(run / ("rustfs/rustfs-members-" + "b" * 32 + "-stopped.json"), {
        **{key: tree[key] for key in ("operation_id", "role", "scope_id", "supervisor", "monitor")},
        "status": "stopped", "error_type": None, "directory": tree["runtime_directory"],
        "members": [tree["supervisor"], tree["monitor"], tree["process"]],
    })
    stopped = publish(run / "lifecycle-0004/stopped.json", {
        "status": "redis_process_stopped", "alive": False, "resources_deleted": False})
    rustfs_stopped = publish(run / "lifecycle-0004/rustfs-stopped.json", {
        "state": "stopped", "tree": tree, "completion": members})
    number = begin(run, "fixture-services", "close", {
        "manifest": binding(run / "manifest.json"), "state_before": binding(run / "state.json")})
    finish(run, number, result={
        "kind": "reference-fixture-service-lifecycle", "operation": "close", "run": str(run),
        "remote_writes": 0, "resources_deleted": False, "status": "services_closed",
        "services": {"redis": "stopped", "rustfs": "stopped"},
        "evidence": {"redis": stopped, "rustfs": rustfs_stopped}})
    predecessor = load_state(run)["attempts"][-1]["result"]
    sources = {"manifest": binding(run / "manifest.json"),
               "state_before": binding(run / "state.json"), "restart_predecessor": predecessor}
    number = begin(run, "fixture-services", "restart", sources)
    root = run / f"lifecycle-{number:04d}"
    rustfs_request = publish(root / "rustfs/request.json", storage_request)
    redis_request.update(previous_identity={"pid": original["redis"]["pid"]},
                         previous_boot_id="fixture-boot",
                         previous_run_id=original["redis"]["run_id"])
    redis_binding = publish(root / "redis/request.json", redis_request)
    storage = copy.deepcopy(original["rustfs"])
    for key in ("process_receipt", "launch_receipt", "tree"):
        value = read_json(Path(original["rustfs"][key]["path"])) if key != "tree" else {"fixture": key}
        filename = {"process_receipt": "process.json", "launch_receipt": "launch.json", "tree": "rustfs-tree.json"}[key]
        storage[key] = publish(root / "rustfs" / filename, value)
    redis = {**original["redis"], "run_id": "4" * 40}
    runtime = {"output": str(root / "redis"), "redis": redis}
    publish(root / "redis/runtime.json", runtime)
    controller = {"fixture": "explicit-restart-controller"}
    generation = publish(root / "restart.json", {
        "format_version": 1, "kind": "reference-fixture-service-generation", "run": str(run),
        "attempt": number, "owner": sources["state_before"], "predecessor": predecessor,
        "previous_generation": previous, "controller": controller,
        "services": {"redis": "running", "rustfs": "running"},
        "rustfs": {"request": rustfs_request, "storage": storage},
        "redis": {"request": redis_binding, "runtime": runtime},
        "remote_writes": 0, "resources_deleted": False})
    finish(run, number, result={
        "kind": "reference-fixture-service-lifecycle", "operation": "restart", "run": str(run),
        "remote_writes": 0, "resources_deleted": False, "status": "services_restarted",
        "services": {"redis": "running", "rustfs": "running"},
        "generation": generation, "controller": controller})
    result = load_state(run)["attempts"][-1]["result"]
    f.storage_restarted = True
    f.redis_values.clear()
    return run, {"storage": {key: storage[key] for key in original["rustfs"]},
                 **storage_request, "generation": result}, {"redis": redis, "generation": result}


@contextmanager
def linked_evidence(path):
    """真实 junction 或硬链接只在独立夹具内存在，退出时恢复原证据。"""
    saved = path.parent.parent / ("link-original-" + path.name)
    if saved.exists():
        raise ValueError("链接夹具前像目录已存在")
    path.rename(saved)
    created = False
    try:
        if saved.is_dir():
            if os.name == "nt":
                import _winapi
                _winapi.CreateJunction(str(saved), str(path))
            else:
                path.symlink_to(saved, target_is_directory=True)
        else:
            os.link(saved, path)
        created = True
        yield
    finally:
        if created:
            if saved.is_dir() and os.name == "nt":
                path.rmdir()
            else:
                path.unlink()
        saved.rename(path)
