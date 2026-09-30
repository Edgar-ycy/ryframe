"""复制后持锁交接的真实观察，数据库和对象只读，启动复用现有 runtime 工具。"""
from dataclasses import asdict
import hashlib
import json
import os
from pathlib import Path
import subprocess
import uuid

from devex_clone import reuse_plan
from devex_clone_capture import read_json, write_json
from devex_clone_export_verify import verify_source_export, verify_export_bindings, observe_source_objects
from devex_clone_factory_context import initialization_history
from process_environment import Environments
from devex_clone_inventory import capture_side_inventory
from devex_clone_ledger import CloneLedger
from devex_clone_live_object import observe_target_object
from devex_clone_model import linked
from devex_clone_object_batch import ordered_batches
from devex_clone_post import registration, registered, validate_registration, pacing_binding
from devex_clone_post_model import action_input, full_row_sql, exact_directory
from devex_clone_post_process import require_quiet
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file, verify_generation, require_closed_port, verify_api_address
from devex_clone_target_binding import request_binding, external_file
from devex_clone_target_resources import Resources
from devex_clone_target_storage import actual_windows_argv
from devex_clone_tools import verify as verify_tools
from devex_provenance import verify_source
from full_stack_process import process_identity, read_process
from full_stack_runtime import verify_runtime, register_runtime, worker_ready_url
from process_sockets import verify_listener
from restore_build import verify_build_artifacts
from restore_reference_io import ExternalTools, redact_object_diagnostic, verification_stdout
from restore_reference_plan import plan_hash, BUCKETS
from restore_source_binding import source_binding, defaults_connection
from source_fingerprints import source_check


def absent(path):
    if path.exists() or linked(path):
        raise ValueError("未登记的生产者或锁仍存在，拒绝接管")


class Context:
    def __init__(self, backend, directory, value, number, run=subprocess.run, *, stage="post-copy"):
        if stage not in {"post-copy", "seed-runtime"}:
            raise ValueError("未知受控交接阶段")
        with source_check(backend):
            self.backend, self.directory_root, self.value, self.runner = backend, directory, value, run
            active = registration(backend, directory)
            self.request_binding = active.descriptor
            self.request = active.request
            self.verified_plan = validate_registration(backend, directory, value, self.request)
            self.copy_root = Path(value["copy_directory"])
            self.copy = {"plan": reuse_plan(backend, str(self.copy_root / "plan.json"), self.verified_plan)[0]["plan"],
                         "result": read_json(self.copy_root / "result.json"), "session": read_json(self.copy_root / "session.json")}
            self.files = {name: binding(self.copy_root / name) for name in ("plan.json", "input.json", "session.json", "result.json", "ledger/head.json")}
            self.ledger = CloneLedger(backend, self.copy_root / "ledger", self.copy["plan"]["plan_sha256"], self.copy["result"]["generation_sha256"])
            self.ledger_state = self.ledger.inspect()
            self.ledger_frames = plan_hash(self.ledger.frames)
            self.initial, self.target = initialization_history(backend, bound_file(backend, value["initialized"]))
            self.review, self.selected = request_binding(backend, self.target)
            self.private = {"source": read_json(bound_file(backend, value["source_environment"]))["environment"],
                            "target_initial": read_json(bound_file(backend, value["target_environment"]))["environment"],
                            "target_api": read_json(bound_file(backend, self.request["api_environment"]))["environment"]}
            self.environments = Environments(self.private["source"], self.private["target_api"])
            self.old = Environments(self.private["source"], self.private["target_initial"])
            self.source = verify_source_export(backend, self.copy["session"]["source_export"])
            self.source_config = self.source["request"]["source"]
            self.base = directory / "post-copy"
            self.base.mkdir(exist_ok=True)
            exact_directory(self.base)
            self.runtime = active.runtime
            output_base = directory / stage
            output_base.mkdir(exist_ok=True)
            exact_directory(output_base)
            self.output = output_base / f"attempt-{number:04d}"
            self.output.mkdir()
            self.target_config = {**self.target["target"], "runtime_dir": str(self.runtime), "api_url": self.selected["api_url"]}
            self.tool_plan = {"source": self.source_config, "target": self.target_config, "tools": self.target["tools"]}
            self.build = read_json(bound_file(backend, self.request["backend_build"]))
            self.redaction = {}
            for side, config in (("source", self.source_config), ("target", self.target_config)):
                env = self.environments.values[side]
                for field in ("access_key_env", "secret_key_env"):
                    self.redaction[f"SECRET_{side}_{field}"] = env[config["s3"][field]]
                for db in config["databases"]:
                    self.redaction["PASSWORD_" + side + db["key"]] = defaults_connection(backend, db)["password"]

    def bindings(self):
        with source_check(self.backend):
            require_quiet(self.backend, self.directory_root, int(self.output.name.removeprefix("attempt-")))
            reuse_plan(self.backend, str(self.copy_root / "plan.json"), self.verified_plan)
            if registered(self.backend, self.directory_root) != self.request:
                raise ValueError("固定 post-copy 登记变化")
            for value in self.files.values():
                bound_file(self.backend, value)
            for field in ("api_environment", "backend_build", "reference_plan", "dataset", "copy_stage_receipt", "ledger_head"):
                bound_file(self.backend, self.request[field])
            external_file(self.request["node"])
            pacing_binding(self.backend, self.directory_root, self.request)
            if self.ledger.inspect() != self.ledger_state or plan_hash(self.ledger.frames) != self.ledger_frames:
                raise ValueError("复制账本在交接后变化")
            if initialization_history(self.backend, bound_file(self.backend, self.value["initialized"])) != (self.initial, self.target):
                raise ValueError("原初始化证据变化")
            verify_export_bindings(self.backend, self.source)
            verify_build_artifacts(self.build)
            verify_source(self.backend, self.build, self.source["request"]["worktree_fingerprint"])
            return verify_tools(self.backend, bound_file(self.backend, self.target["maintenance_build"]), self.run)

    def target_guard(self, *, api):
        from devex_clone_run import target_storage_run

        with self.environments.use("target"):
            if source_binding(self.backend, {"source": self.target_config}) != self.initial["generation"]["physical"]:
                raise ValueError("派生 API 物理目标不是原复制目标")
            verify_api_address(self.backend, self.selected["api_url"])
            if worker_ready_url() != self.selected["worker_ready_url"]:
                raise ValueError("Worker 地址变化")
            require_closed_port(self.selected["worker_ready_url"])
            absent(self.runtime / "worker.json")
            absent(Path(self.selected["runtime_dir"]))
            storage_run = target_storage_run(self.backend, self.value) or self.directory_root
            resources = Resources(self.backend, self.target, self.output, self.selected, self.review, self.run,
                                  storage_run=storage_run)
            resources.storage_identity()
            resources.tools.verify_databases("target")
            resources.tools.verify_objects("target")
            if api:
                self.api_guard(resources)
            else:
                require_closed_port(self.selected["api_url"])
        return resources

    def api_guard(self, resources):
        from devex_clone_post import runtime_inputs
        runtime, _ = runtime_inputs(self.backend, self.directory_root)
        if Path(runtime["runtime_dir"]) != self.runtime or runtime["api_url"] != self.selected["api_url"]:
            raise ValueError("API runtime 交接地址变化")
        contract = verify_runtime(self.backend, self.runtime)
        identity = read_process(self.runtime, "api", self.target_config["scope_id"])
        if (process_identity(identity["pid"]) != identity
                or identity["executable"] != self.build["artifacts"]["api"]["executable"]
                or actual_windows_argv(resources, identity) != [identity["executable"]]):
            raise ValueError("API 内核身份、产物或实际命令发生变化")
        for role in ("api", "worker"):
            artifact = self.build["artifacts"][role]
            if contract["artifacts"][role] != {"path": artifact["executable"], "sha256": artifact["sha256"]}:
                raise ValueError("API runtime 使用其他产物")
        verify_listener(identity["pid"], self.selected["api_url"])

    def prepare(self):
        self.before_api()
        maintenance = self.bindings()
        if self.runtime.exists():
            exact_directory(self.runtime)
            if {p.name for p in self.runtime.iterdir()} - {"binaries.json", "runtime.json", "handoff.json"}:
                raise ValueError("已有运行历史不能重新准备或覆盖")
        else:
            self.runtime.mkdir()
        binaries = {"ryframe": self.build["artifacts"]["api"]["executable"],
                    "ryframe-worker": self.build["artifacts"]["worker"]["executable"],
                    "ryframe-reset": maintenance["artifacts"]["reset"]["executable"],
                    "ryframe-migrate": maintenance["artifacts"]["migrate"]["executable"]}
        filename = self.runtime / "binaries.json"
        if filename.exists():
            if read_json(filename) != binaries:
                raise ValueError("准备中的 runtime 产物不同，拒绝覆盖")
        else:
            write_json(filename, binaries)
        with self.environments.use("target"):
            register_runtime(self.backend, self.runtime)
        handoff = {"registration": self.request_binding, "runtime": binding(self.runtime / "runtime.json"),
                   "binaries": binding(filename), "api_url": self.selected["api_url"], "worker_started": False}
        filename = self.runtime / "handoff.json"
        if filename.exists():
            if read_json(filename) != handoff:
                raise ValueError("准备中的 runtime 交接不同，拒绝覆盖")
        else:
            write_json(filename, handoff)
        self.bindings()
        self.target_guard(api=False)
        return {"status": "post_copy_api_prepared", "handoff": binding(filename), "worker_started": False,
                "restore_qualified": False}

    def business_binding(self):
        reference = read_json(bound_file(self.backend, self.request["reference_plan"]))
        result = self.copy["result"]
        return {"format_version": 1, "kind": "devex-copy-business-target", "source_plan_sha256": plan_hash(reference),
                "source_scope_id": reference["source"]["scope_id"],
                "target": {"scope_id": self.target_config["scope_id"], "api_url": self.selected["api_url"],
                           "frontend_url": self.selected["frontend_url"]},
                "copy": {**{key: result[key] for key in ("plan_sha256", "generation_sha256", "source_export_sha256", "fresh_target_sha256")},
                         "stage_receipt_sha256": self.request["copy_stage_receipt"]["sha256"],
                         "ledger_head_sha256": self.request["ledger_head"]["sha256"]}}

    def business_objects(self, *, remaining=False):
        dataset = read_json(bound_file(self.backend, self.request["dataset"]))
        items = {(item["bucket"], item["target_key"]): item for item in self.copy["plan"]["objects"]}
        covered = set()
        for tenant in dataset["tenants"]:
            for file in tenant["files"]:
                key = ("uploads", self.target_config["scope_id"] + "/" + file["file_path"])
                if key in covered or key not in items or any(file[field] != items[key]["artifact"][field] for field in ("bytes", "sha256")):
                    raise ValueError("原业务对象数据集未精确匹配复制对象计划")
                covered.add(key)
        if remaining:
            with self.environments.use("target"):
                resources = self.target_guard(api=True)
                tools = ExternalTools(self.tool_plan, self.output, self.run)
                for bucket in sorted(BUCKETS):
                    expected = {self.target_config["scope_id"] + "/.ryframe-owner"} | {key for b, key in items if b == bucket}
                    if resources.object_keys(bucket) != expected:
                        raise ValueError("业务读取后目标完整对象范围不同")
                for key in sorted(set(items) - covered):
                    item = items[key]
                    expected = {field: item["artifact"][field] for field in ("bytes", "sha256")}
                    observe_target_object(self.backend, tools, item["bucket"], item["target_key"],
                                          self.directory("remaining-object"), expected=expected, max_bytes=expected["bytes"])
        return {"business_objects": len(covered), "additional_objects": len(items) - len(covered),
                "all_scoped_keys_verified": remaining, "additional_objects_downloaded": remaining}


    def directory(self, label):
        return self.output / (label + '-' + uuid.uuid4().hex)

    def run(self, command, **kwargs):
        filename = self.directory('command').with_suffix('.json')
        stdout, stderr, code, failure = b'', b'', None, None
        try:
            result = self.runner(command, **kwargs)
            stdout, stderr, code = result.stdout, result.stderr, result.returncode
            return result
        except BaseException as error:
            stdout, stderr = getattr(error, 'stdout', b''), getattr(error, 'stderr', b'')
            code, failure = getattr(error, 'returncode', None), type(error).__name__
            raise
        finally:
            stdout, output_evidence = verification_stdout(kwargs.get("stdout"), stdout)
            redaction = dict(os.environ) | (kwargs.get('env') or {}) | self.redaction
            private = Path(command[0]).name.lower() in {'powershell.exe', 'pwsh.exe'}
            def diagnostic(value):
                if not private:
                    return redact_object_diagnostic(value, redaction)
                raw = value if isinstance(value, bytes) else (value or '').encode()
                return {'bytes': len(raw), 'sha256': hashlib.sha256(raw).hexdigest()}
            write_json(filename, {'command': command, 'returncode': code, 'error_type': failure,
                                 'stdout': None if 'stdout_capture_error' in output_evidence else diagnostic(stdout),
                                 **output_evidence, 'stderr': diagnostic(stderr)})

    def source_guard(self, *, complete=False):
        with self.environments.use('source') as environment:
            if verify_generation(self.backend, self.source['request'], self.run) != self.source['generation']:
                raise ValueError('停止的来源代次变化')
            tools = ExternalTools(self.tool_plan, self.output, self.run)
            tools.verify_databases('source')
            if complete:
                inventory = capture_side_inventory(self.backend, 'source', tools,
                    bound_file(self.backend, self.source['request']['maintenance_build']), self.directory('source-inventory'), environment=environment)
                observed = {db['key']: obs for db, obs in zip(sorted(self.source_config['databases'], key=lambda x: x['key']),
                                                            sorted(inventory.observations, key=lambda x: next(d['key'] for d in self.source_config['databases'] if d['database'] == x.resource['database'])), strict=True)}
                actual = {key: {'tables': value.tables, 'preserved': value.preserved} for key, value in observed.items()}
                if actual != self.copy['session']['source_observation']:
                    raise ValueError('来源完整库存不再等于原复制数据来源')
                observe_source_objects(self.backend, tools, self.source, self.directory('source-objects'), environment=environment)

    def before_api(self):
        with source_check(self.backend):
            self.bindings()
            self.source_guard(complete=True)
            with self.old.use('target') as environment:
                tools = ExternalTools(self.tool_plan, self.output, self.run)
                actual = capture_side_inventory(self.backend, 'target', tools, bound_file(self.backend, self.target['maintenance_build']),
                                                 self.directory('target-copy-handoff'), environment=environment)
            for observed in actual.observations:
                key = next(db['key'] for db in self.target_config['databases'] if db['database'] == observed.resource['database'])
                original = self.initial['inventory']['observations'][observed.resource['database']]
                expected = {**original, 'tables': self.copy['session']['source_observation'][key]['tables']}
                if json.loads(json.dumps(asdict(observed))) != expected:
                    raise ValueError('API 启动前目标完整复制后像变化')
            self.bindings()
            self.target_guard(api=False)
            with self.environments.use("target"):
                tools = ExternalTools(self.tool_plan, self.output, self.run)
                resources = self.target_guard(api=False)
                for bucket in sorted(BUCKETS):
                    expected = {self.target_config["scope_id"] + "/.ryframe-owner"} | {
                        item["target_key"] for item in self.copy["plan"]["objects"] if item["bucket"] == bucket}
                    if resources.object_keys(bucket) != expected:
                        raise ValueError("API 启动前完整对象范围不同")
                tasks = [(item, self.directory("target-object")) for item in self.copy["plan"]["objects"]]
                def observe(task):
                    item, output = task
                    expected = {key: item["artifact"][key] for key in ("bytes", "sha256")}
                    return observe_target_object(self.backend, tools, item["bucket"], item["target_key"],
                                                 output, expected=expected, max_bytes=expected["bytes"])
                ordered_batches(observe, tasks, thread_name_prefix="post-target-object")


    def guard(self, *, complete=False):
        with source_check(self.backend):
            self.bindings()
            self.source_guard(complete=complete)
            self.target_guard(api=True)
            self.bindings()

    def row(self, action):
        action_input(action)
        with self.environments.use('target'):
            tools = ExternalTools(self.tool_plan, self.output, self.run)
            tools.verify_databases('target')
            db = next(db for db in self.target_config['databases'] if db['key'] == 'shared-control')
            return json.loads(tools.mysql(db, full_row_sql(self.backend, action)))

    def administrator(self):
        admin = self.request['source_admin']
        sql = "SELECT id,username,status,del_flag FROM sys_user WHERE tenant_id='system' AND id=" + admin['subject_id'] + ';'
        for side, config in (('source', self.source_config), ('target', self.target_config)):
            with self.environments.use(side):
                tools = ExternalTools(self.tool_plan, self.output, self.run)
                db = next(db for db in config['databases'] if db['key'] == 'shared-control')
                if tools.mysql(db, sql).split('\t') != [admin['subject_id'], admin['username'], '1', '0']:
                    raise ValueError('源实际管理员与复制后目标账号不匹配')
