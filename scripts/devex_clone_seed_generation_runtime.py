"""复用完整进程树监督器，在复制 run 锁内启动并静止源 API/Worker。"""
from __future__ import annotations

import copy
from datetime import datetime, timezone
import os
from pathlib import Path
import time
import uuid

from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import Environments, configured
from devex_clone_model import exact, linked, local_path
from devex_clone_run_state import binding
from devex_clone_seed_rebind import quiet_producers
from devex_clone_source_proof import bound_file, require_closed_port, verify_api_address
from devex_clone_storage import current_storage_binding
from devex_clone_tools import verify as verify_tools, verify_evidence
from devex_provenance import verify_source
from full_stack_process import process_identity, read_process
from full_stack_process_monitor import completion_binding
from full_stack_process_tree import (launch_supervised_process, read_process_tree,
                                    terminate_owned_process_tree, validate_process_tree_directory)
from full_stack_runtime import register_runtime, verify_runtime
from process_sockets import verify_listener
from restore_build import registered_source, verify_build, verify_build_artifacts
from restore_source_binding import source_binding
from restore_runtime_evidence import timestamp

ROLES = ("api", "worker")
EXTRA_FILES = ("binaries.json", "runtime.json")


def process_record(runtime: Path, role: str, scope: str) -> dict:
    value = read_json(runtime / f"{role}.json")
    exact(value, {"format_version", "role", "scope_id", "identity"})
    exact(value["identity"], {"pid", "started", "executable"})
    if type(value["format_version"]) is not int:
        raise ValueError("源进程收据版本必须是整数")
    return read_process(runtime, role, scope)


def generation_directory(output: Path) -> None:
    files = {"request.json", "intent.json", "producers.json", "source-request.json",
             "generation-verified.json", "runtime-evidence.json", "running-evidence.json", "dataset-lineage.json"}
    directories = {"runtime", "before", "running", "after", "stop-before", "verification", "recover-before", "recover-after"}
    if linked(output) or not output.is_dir():
        raise ValueError("source-generation 必须是普通代次目录")
    for item in output.iterdir():
        if (linked(item) or item.name not in files | directories
                or item.name in files and not item.is_file() or item.name in directories and not item.is_dir()):
            raise ValueError("source-generation 目录存在未知、链接或类型不符的项")


def registered_inputs(backend: Path, request: dict, *, reconstruct: bool) -> tuple[Path, dict]:
    product = request["product_backend"]
    execution, inventory, _ = registered_source(backend, Path(request["execution_backend"]), request["expected_backend_sha"],
        adapter_contract=request["adapter_contract"], product_backend=None if product is None else Path(product), reconstruct=reconstruct)
    build = read_json(bound_file(backend, request["backend_build"]))
    if build["sources"]["full"] != inventory:
        raise ValueError("后继源 v2 构建与完整注册来源不同")
    verify_build_artifacts(build)
    verify_source(execution, build, inventory["source"]["worktree_fingerprint"])
    maintenance = verify_evidence(execution, bound_file(execution, request["maintenance_build"]))
    if maintenance["source"] != inventory["source"]:
        raise ValueError("后继源维护工具没有绑定同一完整执行源码")
    bound_file(backend, request["source_environment"])
    return execution, build


def verify_running_evidence(backend: Path, output: Path, request: dict, selected: dict, physical: dict, *, live: bool) -> dict:
    from devex_clone_runtime import _ready

    generation_directory(output)
    execution, build = registered_inputs(backend, request, reconstruct=False)
    evidence = read_json(output / "running-evidence.json")
    exact(evidence, {"format_version", "kind", "intent", "runtime", "roles", "physical_binding"})
    intent = read_json(bound_file(backend, evidence["intent"]))
    exact(intent, {"format_version", "kind", "request", "runtime_directory", "operations"})
    runtime = output / "runtime"
    if (type(evidence["format_version"]) is not int or evidence["format_version"] != 1 or evidence["kind"] != "seed-source-runtime-running"
            or evidence["intent"] != binding(output / "intent.json")
            or evidence["runtime"] != binding(runtime / "runtime.json") or evidence["physical_binding"] != physical
            or type(intent["format_version"]) is not int or intent["format_version"] != 1 or intent["kind"] != "seed-source-runtime-intent"
            or intent["request"] != binding(output / "request.json") or intent["runtime_directory"] != str(runtime)
            or set(intent["operations"]) != set(ROLES) or set(evidence["roles"]) != set(ROLES)):
        raise ValueError("运行中源证据不属于同一完整代次、请求或物理来源")
    validate_process_tree_directory(runtime, intent["operations"], extra_files=EXTRA_FILES)
    private = read_json(bound_file(backend, request["source_environment"]))
    exact(private, {"environment"})
    with Environments(configured(private["environment"]), {}).use("source"):
        contract = verify_runtime(execution, runtime)
        if (contract["backend_root"] != str(execution)
                or contract["artifacts"] != {role: {"path": item["executable"], "sha256": item["sha256"]}
                                             for role, item in build["artifacts"].items()}
                or source_binding(execution, {"source": selected}, evidence_root=backend) != physical):
            raise ValueError("运行中源配置、物理来源或产物变化")
    urls = {"api": selected["api_url"].rstrip("/") + "/readyz", "worker": contract["worker_ready_url"]}
    for role in ROLES:
        row = evidence["roles"][role]
        exact(row, {"process", "tree", "ready"})
        tree = read_process_tree(runtime, role, selected["scope_id"])
        identity = process_record(runtime, role, selected["scope_id"])
        if (row["process"] != binding(runtime / f"{role}.json") or row["tree"] != binding(runtime / f"{role}-tree.json")
                or row["ready"] is not True or tree["operation_id"] != intent["operations"][role]
                or tree["process"] != identity):
            raise ValueError("运行中源树、产品创建身份或启动意图变化")
        if live:
            for key in ("process", "supervisor", "monitor"):
                if process_identity(tree[key]["pid"]) != tree[key]:
                    raise ValueError("运行中源完整进程树已退出或 PID 复用")
            verify_listener(identity["pid"], urls[role])
            if not _ready(urls[role]):
                raise ValueError("运行中源未保持双角色 ready")
    return contract


def verify_runtime_evidence(backend: Path, output: Path, request: dict, effective: dict, *, live: bool) -> dict:
    generation_directory(output)
    execution, build = registered_inputs(backend, request, reconstruct=False)
    value = read_json(output / "runtime-evidence.json")
    exact(value, {"format_version", "kind", "intent", "runtime", "roles", "physical_binding", "ports_idle", "observed_stopped_at"})
    observed = datetime.fromisoformat(timestamp(value["observed_stopped_at"], "源停止观察").replace("Z", "+00:00"))
    if observed > datetime.now(timezone.utc):
        raise ValueError("源停止观察不能来自未来")
    intent = read_json(bound_file(backend, value["intent"]))
    exact(intent, {"format_version", "kind", "request", "runtime_directory", "operations"})
    runtime = output / "runtime"
    if (type(value["format_version"]) is not int or value["format_version"] != 1 or value["kind"] != "seed-source-runtime-quiescence"
            or type(intent["format_version"]) is not int or intent["format_version"] != 1 or intent["kind"] != "seed-source-runtime-intent"
            or intent["request"] != binding(output / "request.json") or intent["runtime_directory"] != str(runtime)
            or set(intent["operations"]) != set(ROLES) or set(value["roles"]) != set(ROLES)
            or value["runtime"] != effective["runtime"] or value["ports_idle"] != list(ROLES)):
        raise ValueError("后继源运行证据缺少同一请求、启动意图或完整静止角色")
    validate_process_tree_directory(runtime, intent["operations"], extra_files=EXTRA_FILES)
    contract = read_json(bound_file(backend, effective["runtime"]))
    if (contract["backend_root"] != str(execution)
            or contract["artifacts"] != {role: {"path": item["executable"], "sha256": item["sha256"]} for role, item in build["artifacts"].items()}):
        raise ValueError("后继源 runtime 没有绑定同一 v2 产物")
    for role in ROLES:
        item = value["roles"][role]
        exact(item, {"process", "tree", "completion", "ready"})
        tree = read_process_tree(runtime, role, effective["source"]["scope_id"])
        if (tree["operation_id"] != intent["operations"][role] or item["tree"] != binding(runtime / f"{role}-tree.json")
                or item["process"] != effective["processes"][role] or item["ready"] is not True
                or tree["process"] != process_record(runtime, role, effective["source"]["scope_id"])
                or item["completion"] != completion_binding(tree)):
            raise ValueError("后继源进程树、创建身份、ready 或完整退出证明变化")
    if live:
        require_closed_port(effective["source"]["api_url"])
        require_closed_port(read_json(bound_file(backend, effective["runtime"]))["worker_ready_url"])
    return value


class GenerationRuntime:
    def __init__(self, backend, directory, output, request, source, run):
        self.backend, self.directory, self.output, self.request, self.source, self.run = backend, directory, output, request, source, run
        self.execution = Path(request["execution_backend"])
        self.runtime = output / "runtime"
        self.selected = {**copy.deepcopy(source["request"]["source"]), "runtime_dir": str(self.runtime)}
        self.build = read_json(bound_file(backend, request["backend_build"]))
        self.private = read_json(bound_file(backend, request["source_environment"]))
        exact(self.private, {"environment"})
        self.environments = Environments(configured(self.private["environment"]), {})
        self.operations = {role: uuid.uuid4().hex for role in ROLES}
        self.started, self.ready, self.contract = {}, {}, None

    def environment(self):
        return self.environments.use("source")

    def preflight(self):
        registered_inputs(self.backend, self.request, reconstruct=True)
        verify_build(self.execution, self.build, self.request["expected_backend_sha"], self.run)
        if self.output.exists():
            raise ValueError("源后继运行目录已存在，不能覆盖或重放")
        with self.environment():
            # C52 物理绑定包含配置解析出的连接，不能仅比较数据库名或 server UUID。
            if source_binding(self.execution, {"source": self.selected}, evidence_root=self.backend) != self.source["generation"]["physical_binding"]:
                raise ValueError("后继运行改变了 C52 的完整物理来源配置")
            if any(key in os.environ for key in ("RYFRAME_E2E_FIXTURE", "RYFRAME_FULL_STACK_SOURCE")):
                raise ValueError("源后继不能混入独立 fixture 运行环境")
            quiet_producers(self.backend, self.source)
            self.checkpoint()

    def checkpoint(self):
        from devex_clone_run import _require_owned_run

        _require_owned_run(self.directory)
        for field in ("backend_build", "source_environment"):
            bound_file(self.backend, self.request[field])
        fingerprint = self.build["sources"]["full"]["source"]["worktree_fingerprint"]
        verify_source(self.execution, self.build, fingerprint)
        verify_build_artifacts(self.build)
        maintenance = verify_tools(self.execution, bound_file(self.execution, self.request["maintenance_build"]), self.run)
        if maintenance["source"] != self.build["sources"]["full"]["source"]:
            raise ValueError("源后继维护工具与 v2 产品构建必须绑定同一完整执行来源")
        if (current_storage_binding(self.backend, self.directory, "target") != self.request["current_storage"]
                or source_binding(self.execution, {"source": self.selected}, evidence_root=self.backend) != self.source["generation"]["physical_binding"]):
            raise ValueError("源后继运行期间存储代次或物理配置发生变化")
        verify_api_address(self.execution, self.selected["api_url"])
        if self.contract is not None:
            generation_directory(self.output)
            if verify_runtime(self.execution, self.runtime) != self.contract:
                raise ValueError("源后继运行配置发生变化")
            validate_process_tree_directory(self.runtime, self.operations, extra_files=EXTRA_FILES)

    def prepare(self):
        self.runtime.mkdir()
        write_json(self.output / "intent.json", {"format_version": 1, "kind": "seed-source-runtime-intent",
                   "request": binding(self.output / "request.json"), "runtime_directory": str(self.runtime),
                   "operations": self.operations})
        maintenance = read_json(bound_file(self.execution, self.request["maintenance_build"]))
        binaries = {"ryframe": self.build["artifacts"]["api"]["executable"],
                    "ryframe-worker": self.build["artifacts"]["worker"]["executable"],
                    **{"ryframe-" + role: maintenance["artifacts"][role]["executable"] for role in ("reset", "migrate")}}
        write_json(self.runtime / "binaries.json", binaries)
        self.contract = register_runtime(self.execution, self.runtime)
        self.urls = {"api": self.selected["api_url"].rstrip("/") + "/readyz", "worker": self.contract["worker_ready_url"]}
        for url in self.urls.values():
            require_closed_port(url)

    def start(self, checkpoint):
        from devex_clone_runtime import _ready

        for role in ROLES:
            checkpoint()
            require_closed_port(self.urls[role])
            with (self.runtime / f"{role}.log").open("xb") as log:
                self.started[role] = launch_supervised_process(self.runtime, role, self.selected["scope_id"],
                    [self.build["artifacts"][role]["executable"]], self.execution, dict(os.environ), log,
                    operation_id=self.operations[role])
            deadline = time.monotonic() + 90
            while time.monotonic() < deadline:
                process = self.started[role]
                if process.poll() is not None:
                    raise ValueError("源后继产品在 ready 前退出")
                identity = read_process(self.runtime, role, self.selected["scope_id"])
                if process_identity(identity["pid"]) != identity:
                    raise ValueError("源后继产品创建身份变化")
                if _ready(self.urls[role]):
                    verify_listener(identity["pid"], self.urls[role])
                    self.ready[role] = True
                    break
                time.sleep(0.1)
            if role not in self.ready:
                raise TimeoutError("源后继产品 ready 超时")
        checkpoint()
        for role in ROLES:
            identity = read_process(self.runtime, role, self.selected["scope_id"])
            if process_identity(identity["pid"]) != identity or not _ready(self.urls[role]):
                raise ValueError("源 API/Worker 未同时保持同代 ready")
            verify_listener(identity["pid"], self.urls[role])

    def stop(self):
        from devex_clone_run import _require_owned_run

        validate_process_tree_directory(self.runtime, self.operations, extra_files=EXTRA_FILES)
        failures = []
        for role in reversed(ROLES):
            try:
                _require_owned_run(self.directory)
                path = self.runtime / f"{role}-tree.json"
                if path.exists():
                    tree = read_process_tree(self.runtime, role, self.selected["scope_id"])
                    if tree["operation_id"] != self.operations[role]:
                        raise ValueError("源后继停止意图与进程树 operation 不同")
                    terminate_owned_process_tree(tree)
                    completion_binding(tree)
                    if role in self.started:
                        self.started[role].release_controller_handle()
                require_closed_port(self.urls[role])
            except BaseException as error:
                failures.append(error)
        if failures:
            for other in failures[1:]:
                failures[0].add_note(type(other).__name__)
            raise failures[0]

    def retain(self):
        """产品运行权交给已发布树监督器，启动 CLI 的退出不代表产品停止。"""
        for role in ROLES:
            self.started[role].release_controller_handle()

    def running_evidence(self):
        roles = {role: {"process": binding(self.runtime / f"{role}.json"),
                        "tree": binding(self.runtime / f"{role}-tree.json"), "ready": self.ready[role]}
                 for role in ROLES}
        write_json(self.output / "running-evidence.json", {"format_version": 1, "kind": "seed-source-runtime-running",
                   "intent": binding(self.output / "intent.json"), "runtime": binding(self.runtime / "runtime.json"),
                   "roles": roles, "physical_binding": self.source["generation"]["physical_binding"]})

    def finish(self):
        roles = {}
        for role in ROLES:
            tree = read_process_tree(self.runtime, role, self.selected["scope_id"])
            roles[role] = {"process": binding(self.runtime / f"{role}.json"),
                           "tree": binding(self.runtime / f"{role}-tree.json"),
                           "completion": completion_binding(tree), "ready": self.ready[role]}
        self.producers()
        write_json(self.output / "runtime-evidence.json", {"format_version": 1, "kind": "seed-source-runtime-quiescence",
                   "intent": binding(self.output / "intent.json"), "runtime": binding(self.runtime / "runtime.json"),
                   "roles": roles, "physical_binding": self.source["generation"]["physical_binding"], "ports_idle": list(ROLES),
                   "observed_stopped_at": datetime.now(timezone.utc).isoformat()})

    def producers(self):
        original = read_json(bound_file(self.backend, self.source["request"]["producers_registry"]))
        producers = [{"name": f"predecessor-{index}", "identity": item["identity"]}
                     for index, item in enumerate(original["processes"])]
        producers += [{"name": role, "identity": read_process(self.runtime, role, self.selected["scope_id"])} for role in ROLES]
        value = {"format_version": 1, "scope_id": self.selected["scope_id"],
                 "runtime_sha256": binding(self.runtime / "runtime.json")["sha256"], "processes": producers}
        path = self.output / "producers.json"
        if path.exists():
            if read_json(path) != value:
                raise ValueError("源后继生产者登记变化")
        else:
            write_json(path, value)
