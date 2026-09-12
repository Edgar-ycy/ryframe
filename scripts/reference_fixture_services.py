"""以统一账本启动隔离参考夹具的首代服务。"""
from __future__ import annotations

import sys
sys.dont_write_bytecode = True

import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
from urllib.parse import urlsplit

from devex_clone_capture import read_json, write_json
from devex_clone_cache_process import start as start_redis
from process_environment import configured
from devex_clone_model import linked, local_path
from devex_clone_run_state import begin, bind_controller_attempt, finish, initialize_state, load_state, run_lock
from devex_clone_source_proof import require_closed_port
from devex_clone_storage_process import start as start_rustfs
from restore_build import file_digest
from restore_reference_plan import BUCKETS, plan_hash
from reference_fixture_environment import validate_current_review_tools, validate_preflight_successor
from reference_fixture_paths import service_run
from reference_fixture_control_protocol import run_private


def bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def document(backend: Path, value: Path) -> tuple[Path, dict]:
    path = local_path(backend, str(value if value.is_absolute() else backend / value))
    if linked(path) or not path.is_file():
        raise ValueError("夹具服务输入必须是受控普通文件")
    return path, read_json(path)


def _continued_review(backend: Path, bootstrap_binding: dict, review_file: Path, review: dict) -> bool:
    preflight = review.get("preflight")
    expected = {key: bootstrap_binding.get(key) for key in ("path", "bytes", "sha256")}
    if not isinstance(preflight, dict) or preflight.get("supersedes") != expected:
        return False
    parent = Path(bootstrap_binding["path"])
    try:
        parent_file, previous = document(backend, parent)
    except (FileNotFoundError, ValueError):
        return False
    try:
        validate_preflight_successor(previous, review)
    except (KeyError, TypeError, ValueError):
        return False
    return bound(parent_file) == expected


def environment(backend: Path, review_file: Path, review: dict, bootstrap_path: Path) -> tuple[Path, dict, dict]:
    bootstrap_file, bootstrap = document(backend, bootstrap_path)
    if (bootstrap.get("kind") != "reference-fixture-environment" or bootstrap.get("status") != "prepared"
            or bootstrap.get("services_started") is not False or bootstrap.get("remote_writes") != 0):
        raise ValueError("夹具环境尚未准备完成或已经发生服务副作用")
    binding = bootstrap.get("plan", {}).get("review")
    if (not isinstance(binding, dict)
            or (binding.get("path"), binding.get("bytes"), binding.get("sha256")) != tuple(bound(review_file)[key] for key in ("path", "bytes", "sha256"))
            and not _continued_review(backend, binding, review_file, review)):
        raise ValueError("夹具环境未绑定当前审阅收据")
    environment_file = bootstrap_file.parent / "environment.json"
    values = read_json(environment_file).get("environment")
    if not isinstance(values, dict) or any(type(k) is not str or type(v) is not str for k, v in values.items()):
        raise ValueError("夹具私有环境无效")
    return bootstrap_file, bootstrap, values


def _initial_services(state: dict) -> None:
    expected = (("storage-target", "initial"), ("cache-target", "initial"))
    actual = tuple((item["stage"], item["mode"], item["status"]) for item in state["attempts"])
    if actual != tuple((*item, "passed") for item in expected):
        raise ValueError("对象桶初始化只允许接续同一账本中的完成 RustFS 与 Redis 首代")


def _bucket_environment(private: dict) -> dict:
    access, secret = private.get("APP_OBJECT_STORAGE_ACCESS_KEY"), private.get("APP_OBJECT_STORAGE_SECRET_KEY")
    if not isinstance(access, str) or not access or not isinstance(secret, str) or not secret:
        raise ValueError("对象桶初始化缺少私有 S3 凭据")
    return {**configured(private), "AWS_ACCESS_KEY_ID": access, "AWS_SECRET_ACCESS_KEY": secret}


def _ready_guard(backend: Path, review_file: Path, review: dict, bootstrap_file: Path, bootstrap: dict,
                 private: dict, credential_files: dict, sources: dict) -> None:
    current_review_file, current_review = document(backend, review_file)
    current_bootstrap_file, current_bootstrap = document(backend, bootstrap_file)
    if (current_review_file != review_file or current_review != review or bound(current_review_file) != sources["review"]
            or current_bootstrap_file != bootstrap_file or current_bootstrap != bootstrap
            or bound(current_bootstrap_file) != sources["bootstrap"]):
        raise ValueError("RustFS 就绪期间审阅或私有环境收据发生变化")
    environment_file = bootstrap_file.parent / "environment.json"
    if (bound(environment_file) != sources["environment"]
            or read_json(environment_file) != {"environment": private}):
        raise ValueError("RustFS 就绪期间私有环境发生变化")
    if any(bound(Path(item["path"])) != item for item in credential_files.values()):
        raise ValueError("RustFS 就绪期间凭据文件发生变化")
    for name in ("manifest", "request"):
        if bound(Path(sources[name]["path"])) != sources[name]:
            raise ValueError(f"RustFS 就绪期间 {name} 发生变化")
    validate_current_review_tools(review)


def buckets(backend: Path, review_path: Path, bootstrap_path: Path, *, write: bool) -> dict:
    """在空 RustFS 首代上一次性建立五个产品桶；失败账本禁止自动接管。"""
    if not write:
        raise ValueError("初始化夹具对象桶必须显式指定 --write")
    review_file, review = document(backend, review_path)
    bootstrap_file, bootstrap, private = environment(backend, review_file, review, bootstrap_path)
    execution = Path(bootstrap["execution_backend"])
    run = service_run(review)
    manifest = run / "manifest.json"
    output = run / "buckets"
    if output.exists() or not manifest.is_file():
        raise ValueError("对象桶初始化必须复用首代服务账本且不能覆盖已有证据")
    _initial_services(load_state(run))
    tool = review["tools"]["aws"]
    if file_digest(Path(tool["path"]))["sha256"] != tool["sha256"]:
        raise ValueError("对象桶初始化前 AWS 工具已变化")
    endpoint, region = review["scopes"]["seed"]["objects"]["endpoint"], review["scopes"]["seed"]["objects"]["region"]
    environment_values = _bucket_environment(private)
    output.mkdir()
    sources = {"review": bound(review_file), "bootstrap": bound(bootstrap_file), "manifest": bound(manifest),
               "aws": {"path": tool["path"], "sha256": tool["sha256"]}}

    def guard() -> None:
        current_review, current = document(backend, review_file)
        current_bootstrap, current_value = document(backend, bootstrap_file)
        if current_review != review_file or current != review or current_bootstrap != bootstrap_file or current_value != bootstrap:
            raise ValueError("对象桶初始化期间审阅或私有环境收据发生变化")
        if bound(manifest) != sources["manifest"]:
            raise ValueError("对象桶初始化期间服务账本清单发生变化")
        if file_digest(Path(tool["path"]))["sha256"] != sources["aws"]["sha256"]:
            raise ValueError("对象桶初始化期间 AWS 工具发生变化")

    with run_lock(run):
        _initial_services(load_state(run))
        number = begin(run, "fixture-buckets", "prepare", sources)
        try:
            base = [tool["path"], "--endpoint-url", endpoint, "--region", region, "--no-paginate", "s3api"]
            for bucket in sorted(BUCKETS):
                guard()
                subprocess.run([*base, "create-bucket", "--bucket", bucket], cwd=execution, env=environment_values,
                               stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, check=True, timeout=30,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                guard()
                subprocess.run([*base, "head-bucket", "--bucket", bucket], cwd=execution, env=environment_values,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True, timeout=30,
                               creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
                guard()
            guard()
            result = {"status": "fixture_buckets_ready", "buckets": sorted(BUCKETS), "remote_writes": len(BUCKETS)}
            finish(run, number, result=result)
        except BaseException as error:
            finish(run, number, error=error)
            raise
    return result


def rustfs(backend: Path, review_path: Path, bootstrap_path: Path, *, write: bool) -> dict:
    if not write:
        raise ValueError("启动夹具服务必须显式指定 --write")
    review_file, review = document(backend, review_path)
    bootstrap_file, bootstrap, private = environment(backend, review_file, review, bootstrap_path)
    service = review["services"]["rustfs"]
    execution = Path(bootstrap["execution_backend"])
    run = service_run(review)
    output = run / "rustfs"
    root = run.parent
    if root.exists():
        if linked(root) or not root.is_dir() or any(root.iterdir()):
            raise ValueError("首代服务根目录已存在且不为空、经过链接或不是目录")
    else:
        root.mkdir()
    if run.exists() or output.exists():
        raise ValueError("首代服务账本目录已经存在或父目录缺失")
    for url in (service["api"], service["console"]):
        require_closed_port(url)
    data = Path(service["data_dir"]).resolve()
    local = (backend / ".local-tests").resolve()
    if (not data.is_relative_to(local) or linked(data) or any(linked(path) for path in data.parents if path.exists())
            or data.exists() and (not data.is_dir() or any(data.iterdir()))):
        raise ValueError("RustFS 数据目录越界、经过链接或不是明确空目录")
    secrets = bootstrap["secret_files"]
    files = {"access_key": secrets["rustfs-access-key.txt"], "secret_key": secrets["rustfs-secret-key.txt"]}
    for item in files.values():
        if bound(Path(item["path"])) != item:
            raise ValueError("RustFS 凭据文件在启动前已变化")
    executable = review["tools"]["rustfs"]
    if file_digest(Path(executable["path"]))["sha256"] != executable["sha256"]:
        raise ValueError("RustFS 二进制在启动前已变化")
    validate_current_review_tools(review)
    run.mkdir(); output.mkdir(); data.parent.mkdir(exist_ok=True); data.mkdir(exist_ok=True)
    manifest = {"format_version": 1, "kind": "reference-fixture-service-run", "review": bound(review_file),
                "bootstrap": bound(bootstrap_file), "execution_backend": str(execution), "scope_id": service["scope_id"],
                "data_directory_was_empty": True}
    write_json(run / "manifest.json", manifest); initialize_state(run)
    request = {"scope_id": service["scope_id"], "executable": {**executable},
               "data_directory": {"path": str(data), "device": data.stat().st_dev, "inode": data.stat().st_ino},
               "api_url": service["api"], "console_url": service["console"], "credential_files": files,
                "timeout_seconds": 60}
    write_json(output / "request.json", request)
    sources = {"review": bound(review_file), "bootstrap": bound(bootstrap_file),
               "environment": bound(bootstrap_file.parent / "environment.json"),
               "manifest": bound(run / "manifest.json"), "request": bound(output / "request.json")}

    def guard() -> None:
        _ready_guard(backend, review_file, review, bootstrap_file, bootstrap, private, files, sources)

    guard()
    with run_lock(run) as owner:
        number = begin(run, "storage-target", "initial", {"review": bound(review_file), "bootstrap": bound(bootstrap_file),
                                                         "request": bound(output / "request.json")})
        controller = bind_controller_attempt(run, number, owner)
        try:
            result = start_rustfs(backend, request, configured(private), output, bound(run / "manifest.json"), controller,
                                  number, guard, supervised=True)
            finish(run, number, result=result)
        except BaseException as error:
            finish(run, number, error=error)
            raise
    return {"status": "rustfs_started", "storage": result, "run": str(run), "remote_writes": 0,
            "services_started": True, "request_sha256": plan_hash(request)}


def redis(backend: Path, review_path: Path, bootstrap_path: Path, *, write: bool) -> dict:
    if not write:
        raise ValueError("启动夹具服务必须显式指定 --write")
    review_file, review = document(backend, review_path)
    bootstrap_file, bootstrap, private = environment(backend, review_file, review, bootstrap_path)
    execution = Path(bootstrap["execution_backend"])
    run = service_run(review)
    output = run / "redis"
    manifest = run / "manifest.json"
    if output.exists() or not manifest.is_file():
        raise ValueError("Redis 首代必须复用已有 RustFS 服务账本且不能覆盖已有证据")
    state = load_state(run)
    if (len(state["attempts"]) != 1 or state["attempts"][0]["stage"] != "storage-target"
            or state["attempts"][0]["mode"] != "initial" or state["attempts"][0]["status"] != "passed"):
        raise ValueError("Redis 首代只允许接续已完成的 RustFS 首代账本")
    service = review["services"]["redis"]
    directory = Path(service["directory"]).resolve()
    local = (backend / ".local-tests").resolve()
    if (not directory.is_relative_to(local) or linked(directory)
            or any(linked(path) for path in directory.parents if path.exists())
            or directory.exists() and (not directory.is_dir() or any(directory.iterdir()))):
        raise ValueError("Redis 数据目录越界、经过链接或不是明确空目录")
    port = urlsplit("redis://" + service["endpoint"]).port
    if port is None:
        raise ValueError("Redis 审阅端点无端口")
    password = private.get("APP_REDIS_PASSWORD")
    if not isinstance(password, str) or not password:
        raise ValueError("Redis 首代缺少私有认证")
    tools = review["tools"]
    redis_tool, wsl, python = tools["redis_server"], tools["wsl"], tools["redis_python"]
    if (redis_tool["distribution"] != service["wsl_distribution"] or python["distribution"] != service["wsl_distribution"]
            or set(wsl) != {"path", "sha256"} or set(python) != {"distribution", "path", "resolved_path", "sha256"}):
        raise ValueError("Redis 服务与冻结监督器工具不一致")
    if not directory.exists():
        directory.mkdir()
    configuration = directory / "redis.conf"
    with configuration.open("x", encoding="utf-8", newline="\n") as stream:
        stream.write("\n".join(("bind 127.0.0.1", f"port {port}", f"dir /mnt/{directory.drive[0].lower()}/" + "/".join(directory.parts[1:]),
                                  "databases 1", "protected-mode yes", 'save ""', "appendonly no", "daemonize no",
                                  "requirepass " + shlex.quote(password))) + "\n")
    request = {"scope_id": "services-" + bootstrap["plan"]["scope_id"], "wsl": wsl,
               "distribution": service["wsl_distribution"], "launcher": "/usr/bin/redis-server",
               "executable": redis_tool["resolved_path"], "sha256": redis_tool["sha256"], "configuration": bound(configuration),
               "directory": {"path": str(directory), "device": directory.stat().st_dev, "inode": directory.stat().st_ino},
               "previous_identity": None, "previous_boot_id": None, "previous_run_id": None, "port": port,
               "password_env": "APP_REDIS_PASSWORD", "timeout_seconds": 60,
               "python": {"path": python["path"], "executable": python["resolved_path"], "sha256": python["sha256"]}}
    output.mkdir()
    write_json(output / "request.json", request)
    sources = {"review": bound(review_file), "bootstrap": bound(bootstrap_file), "manifest": bound(manifest),
               "request": bound(output / "request.json")}
    def guard() -> None:
        current_review, current = document(backend, review_file)
        current_bootstrap, current_value = document(backend, bootstrap_file)
        if current_review != review_file or current != review or current_bootstrap != bootstrap_file or current_value != bootstrap:
            raise ValueError("Redis 启动期间审阅或私有环境收据发生变化")
        if bound(manifest) != sources["manifest"]:
            raise ValueError("Redis 启动期间服务账本清单发生变化")
    with run_lock(run) as owner:
        number = begin(run, "cache-target", "initial", sources)
        controller = bind_controller_attempt(run, number, owner)
        try:
            runtime = start_redis(request, configured(private), output, guard)
            result = {"service": "redis", "runtime": runtime, "request_sha256": plan_hash(request),
                      "controller": controller, "remote_writes": 0}
            finish(run, number, result=result)
        except BaseException as error:
            finish(run, number, error=error)
            raise
    return {"status": "redis_started", "storage": runtime["redis"], "run": str(run), "remote_writes": 0,
            "services_started": True, "request_sha256": plan_hash(request)}


PROTOCOL_SCHEMAS = {
    "rustfs": (("review", "environment"), (), True),
    "redis": (("review", "environment"), (), True),
    "buckets": (("review", "environment"), (), True),
    "status": (("review", "environment"), (), False),
    "close": (("review", "environment"), (), True),
    "recover": (("review", "environment", "owner_binding"), (), True),
    "restart": (("review", "environment", "owner_binding"), (), True),
}


def main(argv: list[str]) -> None:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("operation", choices=("rustfs", "redis", "buckets", "status", "close", "recover", "restart"))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--review", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--owner-binding", type=Path,
                        help="recover 控制器时传绑定描述文件；核对外部终止时传 status 返回的 state 文件")
    options = [value.partition("=")[0] for value in argv if value.startswith("--")]
    if len(options) != len(set(options)):
        parser.error("服务控制选项不能重复")
    args = parser.parse_args(argv)
    if args.operation == "status" and (args.write or args.owner_binding is not None):
        parser.error("status 只读，不接受 --write 或 --owner-binding")
    if args.operation != "status" and not args.write:
        parser.error("服务操作必须显式指定 --write")
    if (args.operation in {"recover", "restart"}) != (args.owner_binding is not None):
        parser.error("recover 和 restart 必须指定 --owner-binding，其他操作不接受")
    backend = args.backend_dir.resolve(strict=True)
    if args.operation in {"status", "close", "recover", "restart"}:
        from reference_fixture_service_context import status
        from reference_fixture_service_lifecycle import close, recover, restart

        if args.operation == "status":
            result = status(backend, args.review, args.environment)
        elif args.operation == "close":
            result = close(backend, args.review, args.environment, write=args.write)
        elif args.operation == "recover":
            result = recover(backend, args.review, args.environment, args.owner_binding, write=args.write)
        else:
            result = restart(backend, args.review, args.environment, args.owner_binding, write=args.write)
        print(json.dumps(result, ensure_ascii=False))
        return
    action = {"rustfs": rustfs, "redis": redis, "buckets": buckets}[args.operation]
    result = action(backend, args.review, args.environment, write=args.write)
    summary = {"status": result["status"], "remote_writes": result["remote_writes"]}
    if "services_started" in result:
        summary["services_started"] = result["services_started"]
    print(json.dumps(summary, ensure_ascii=False))


if __name__ == "__main__":
    raise SystemExit(run_private("services", PROTOCOL_SCHEMAS, main, positional_operation=True))
