"""把实际源导出、新目标初始化和步骤账本接成一次显式开发复制；不启动服务。"""
from __future__ import annotations

import copy
from dataclasses import replace
import json
import hashlib
import os
from pathlib import Path
import subprocess
import uuid

from devex_clone import write_plan_result
from devex_clone_capture import write_json
from devex_clone_export_verify import (observe_source_object, observe_source_objects, verify_export_bindings,
                                       verify_source_export)
from devex_clone_factory_context import database_observation, initialization_history, target_history
from process_environment import Environments
from devex_clone_factory_plan import declaration
from devex_clone_inventory import capture_side_inventory
from devex_clone_ledger import CloneLedger
from devex_clone_copy_locks import register_copy_owner
from devex_clone_live_object import observe_target_object, verify_target_heads
from devex_clone_object_batch import WORKERS, ordered_batches
from devex_clone_model import local_path, name, schema_fingerprints
from devex_clone_source_proof import bound_file, verify_generation
from devex_clone_target import verify_target
from devex_clone_target_state import generation_lock
from devex_clone_transfer import DatabaseBasis, GenerationObservation, TransferSteps
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic, verification_stdout
from restore_reference_plan import BUCKETS, plan_hash
from restore_source_binding import defaults_connection
from source_fingerprints import source_check


class CloneSession:
    """只由 prepare 取得真实初像；不从 ready JSON 构造写入许可。"""
    def __init__(self, backend: Path, source_export: dict, initialized: dict, source_environment: dict,
                 target_environment: dict, output: Path, run, *, existing: bool = False,
                 source_storage_run: Path | None = None, target_storage_run: Path | None = None):
        self.backend = backend.resolve(strict=True)
        self.output = local_path(self.backend, str(output), new=not existing)
        if not self.output.parent.is_dir():
            raise ValueError("复制执行目录须有已存在的明确父目录")
        self.environment = Environments(source_environment, target_environment)
        self.export_binding, self.target_binding = copy.deepcopy(source_export), copy.deepcopy(initialized)
        self.runner, self.run, self.source_initial = run, self.command, None
        self.owned_lock_identity = None
        self.copy_owner = None
        self.tools = None
        self.source_storage_run = source_storage_run
        self.copy_stage = None
        self.target_storage_run = target_storage_run
        self.latest_objects = {}
        self.secret_redaction = None
        if existing:
            if not self.output.is_dir():
                raise ValueError("续跑必须使用已有复制执行目录")
        else:
            self.output.mkdir()

    def bind_credentials(self) -> None:
        values = set()
        for side, config in (("source", self.exported["source"]), ("target", self.target_request["target"])):
            environment = self.environment.values[side]
            for field in ("access_key_env", "secret_key_env"):
                value = environment.get(config["s3"][field])
                if not value:
                    raise ValueError("复制日志缺少已登记对象凭据的脱敏绑定")
                values.add(value)
            values.update(defaults_connection(self.backend, database)["password"] for database in config["databases"])
            values.update(environment[key] for key in ("RYFRAME_RESET_ADMIN_PASSWORD", "RYFRAME_RESET_USER_PASSWORD") if environment.get(key))
        self.secret_redaction = {f"COPY_PASSWORD_{index}": value for index, value in enumerate(sorted(values))}

    def command(self, command, **kwargs):
        # 写请求前就占有诊断文件；不记录 stdin 中的业务 SQL 或明文环境。
        if self.secret_redaction is None:
            raise ValueError("外部命令必须先绑定两侧文件与配置中的实际凭据")
        prefix = "command-" + (self.copy_owner.session_id + "-" if self.copy_owner else "")
        path = self.output / (prefix + uuid.uuid4().hex + ".json")
        redaction = dict(os.environ) | (kwargs.get("env") or {}) | self.secret_redaction
        private_probe = Path(command[0]).name.lower() in {"powershell.exe", "pwsh.exe"}

        def diagnostic(value):
            if not private_probe:
                return redact_object_diagnostic(value, redaction)
            raw = value if isinstance(value, bytes) else (value or "").encode("utf-8")
            return {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}

        stdout, stderr, code, error_type = b"", b"", None, None
        with path.open("x", encoding="utf-8", newline="\n") as stream:
            try:
                result = self.runner(command, **kwargs)
                stdout, stderr, code = result.stdout, result.stderr, result.returncode
                return result
            except BaseException as error:
                stdout, stderr = getattr(error, "stdout", b""), getattr(error, "stderr", b"")
                code, error_type = getattr(error, "returncode", None), type(error).__name__
                raise
            finally:
                stdout, output_evidence = verification_stdout(kwargs.get("stdout"), stdout)
                json.dump({"command": command, "returncode": code, "error_type": error_type,
                           "output_policy": "digest_only" if private_probe else "redacted",
                           "stdout": None if "stdout_capture_error" in output_evidence else diagnostic(stdout),
                           **output_evidence, "stderr": diagnostic(stderr)}, stream, ensure_ascii=False, indent=2)
                stream.write("\n")
                stream.flush()
                os.fsync(stream.fileno())

    def directory(self, purpose: str) -> Path:
        return self.output / (name(purpose) + "-" + uuid.uuid4().hex)

    def _source_generation(self, selection: str | tuple[str, str] = "all") -> dict:
        with source_check(self.backend), self.environment.use("source"):
            if self.source_storage_run is not None:
                from devex_clone_run import source_storage_binding

                source_storage_binding(
                    self.backend, self.source_storage_run, copy_stage=self.copy_stage,
                )
            verify_export_bindings(self.backend, self.source_verified, selection=selection)
            current = verify_generation(self.backend, self.source_request, self.run)
        if current != self.source_verified["generation"]:
            raise ValueError("源实际运行、停止声明、配置或构建代次与导出不同")
        return current

    def _target_generation(self):
        with source_check(self.backend), self.environment.use("target"):
            return target_history(self.backend, self.target_binding, self.output, self.run,
                                  owned_lock_identity=self.owned_lock_identity,
                                  storage_run=self.target_storage_run)

    def _inventory(self, side: str):
        config = self.tools.plan[side]
        receipt = self.source_request["maintenance_build"] if side == "source" else self.target_request["maintenance_build"]
        with source_check(self.backend), self.environment.use(side) as environment:
            captured = capture_side_inventory(self.backend, side, self.tools, bound_file(self.backend, receipt),
                                              self.directory(side + "-inventory"), environment=environment)
        by_name = {db["database"]: db["key"] for db in config["databases"]}
        return {by_name[item.resource["database"]]: item for item in captured.observations}

    def _source_inventory(self) -> dict:
        observed = self._inventory("source")
        inventory = self.source_verified["inventory"]
        fingerprints = schema_fingerprints(inventory)
        for database in inventory["databases"]:
            item = observed[database["key"]]
            actual = item.tables | item.preserved
            expected = {row["table"]: {field: row[field] for field in ("rows", "sha256")} for row in database["tables"]}
            schema = dict(fingerprints)
            if database["kind"] != "combined":
                schema["control_schema_fingerprint"] = None
            if item.schema_sha256 != plan_hash(schema) or any(actual.get(table) != value for table, value in expected.items()):
                raise ValueError("源当前完整行像或 schema 与导出前后库存不同")
        if self.source_initial is not None and observed != self.source_initial:
            raise ValueError("复制期间源业务或保留表发生变化")
        return observed

    def prepare(self, copy_id: str, stage: str) -> None:
        self.copy_stage = stage
        self.source_verified = verify_source_export(self.backend, self.export_binding)
        self.exported = self.source_verified["export"]
        self.source_request = self.source_verified["request"]
        self.target_directory = bound_file(self.backend, self.target_binding).parent
        _, self.target_request = initialization_history(self.backend, self.target_directory / "initialized.json", None)
        self.bind_credentials()
        source_generation = self._source_generation()
        initial, self.target_request, _ = self._target_generation()
        with source_check(self.backend), self.environment.use("target"):
            target_proof = verify_target(
                self.backend, self.target_directory, self.directory("target-precopy"), self.run,
                storage_run=self.target_storage_run,
            )
        if target_proof["initialized"] != self.target_binding:
            raise ValueError("实际新目标复验不属于外部绑定的初始化代次")
        target = initial["generation"]["selected"]
        target_config = {**self.target_request["target"], "runtime_dir": target["runtime_dir"], "api_url": target["api_url"]}
        tools = {role: self.source_request["tools"][role] for role in ("mysql", "aws")}
        if tools != self.target_request["tools"]:
            raise ValueError("两侧复制必须绑定同一 MySQL 与 S3 工具")
        self.tools = ExternalTools({"source": self.exported["source"], "target": target_config, "tools": tools}, self.output, self.run)
        self.source_initial = self._source_inventory()
        by_name = initial["inventory"]["observations"]
        self.initialized = {db["key"]: database_observation(by_name[db["database"]]) for db in target_config["databases"]}
        self.initial_target = initial
        with self.environment.use("source") as environment:
            observe_source_objects(self.backend, self.tools, self.source_verified, self.directory("source-objects-initial"), environment=environment)
        # 仅增加独立的小型目标证明；源导出已经绑定的文件与远端数据均不修改。
        root = local_path(self.backend, self.exported["artifact_root"])
        proofs = root / ("target-proof-" + uuid.uuid4().hex)
        proofs.mkdir()
        write_json(proofs / "initialized.json", initial)
        write_json(proofs / "stopped.json", target_proof)
        evidence = {"source_stopped": self.exported["evidence"]["generation-after"],
                    "target_initialized": {"file": (proofs / "initialized.json").relative_to(root).as_posix(), **file_digest(proofs / "initialized.json")},
                    "target_stopped": {"file": (proofs / "stopped.json").relative_to(root).as_posix(), **file_digest(proofs / "stopped.json")}}
        self.declaration = declaration(self.backend, self.exported, self.source_initial, target_config, self.initialized,
                                       evidence, copy_id=copy_id, stage=stage)
        write_json(self.output / "input.json", self.declaration)
        self.plan_file = self.output / "plan.json"
        self.verified_plan = write_plan_result(self.backend, str(self.output / "input.json"), str(self.plan_file))
        self.plan = self.verified_plan.plan
        self.generation_sha256 = plan_hash({"source": self.export_binding, "target": self.target_binding,
                                            "source_generation": source_generation, "target_generation": initial["generation"],
                                            "environments": self.environment.bindings, "plan": self.plan["plan_sha256"]})
        write_json(self.output / "session.json", {"source_export": self.export_binding, "initialized": self.target_binding,
                   "generation_sha256": self.generation_sha256, "plan_sha256": self.plan["plan_sha256"],
                   "source_observation": {key: {"tables": item.tables, "preserved": item.preserved} for key, item in self.source_initial.items()},
                   "clone_verified": False, "restore_qualified": False})

    def generation(self, plan: dict, declared: dict, tools: ExternalTools, *,
                   source_object: tuple[str, str] | None = None) -> GenerationObservation:
        with source_check(self.backend):
            if plan != self.plan or declared != self.declaration or tools is not self.tools:
                raise ValueError("步骤引擎不属于本次实际准备的完整复制计划")
            if source_object is not None and source_object not in {
                    (item["bucket"], item["source_key"]) for item in self.plan["objects"]}:
                raise ValueError("当次源对象证明不属于当前复制计划")
            self._source_generation(source_object if source_object is not None else "global")
            current, request, _ = self._target_generation()
            if current != self.initial_target or request != self.target_request:
                raise ValueError("目标初始化历史变化")
            return GenerationObservation(self.plan["plan_sha256"], self.generation_sha256, self.plan["input_sha256"],
                                         plan_hash(tools.plan), self.export_binding["sha256"], self.target_binding["sha256"], True, True, True)

    def verify_target_owners(self) -> None:
        """每个显式复制阶段仅一次完整五桶核验；失败保留原始 owner 与命令诊断。"""
        directory = self.directory("target-owners")
        directory.mkdir()
        tools_plan = copy.deepcopy(self.tools.plan)
        tool_plan_sha256 = plan_hash(tools_plan)
        try:
            with self.environment.use("target"):
                tools = ExternalTools(tools_plan, directory, self.run)
                tools.verify_objects("target")
                if plan_hash(tools.plan) != tool_plan_sha256 or plan_hash(self.tools.plan) != tool_plan_sha256:
                    raise ValueError("阶段 ownership 核验期间复制工具计划发生变化")
                owners = [{"file": path.name, **file_digest(path)} for path in sorted(directory.iterdir())]
            write_json(directory / "verified.json", {"status": "target_owners_verified",
                       "scope_id": tools_plan["target"]["scope_id"], "copy_plan_sha256": self.plan["plan_sha256"],
                       "tool_plan_sha256": tool_plan_sha256, "environment_sha256": self.environment.bindings["target"],
                       "owners": owners, "remote_writes": 0})
        except BaseException as error:
            write_json(directory / "failure.json", {"status": "target_owners_failed", "error_type": type(error).__name__,
                       "remote_writes": 0})
            raise

    def database_basis(self, key: str) -> DatabaseBasis:
        if key not in self.initialized:
            raise ValueError("数据库不属于本复制完整清单")
        artifact = next(item["artifact"]["sha256"] for item in self.exported["databases"] if item["key"] == key)
        return DatabaseBasis(copy.deepcopy(self.source_initial[key]), copy.deepcopy(self.initialized[key]), artifact)

    def observe_databases(self) -> dict:
        """只读批次取得完整两侧向量；每次重采，不跨步骤或写入复用。"""
        source = self._source_inventory()
        observed = self._inventory("target")
        expected = set(self.initialized)
        if set(source) != expected or set(observed) != expected:
            raise ValueError("数据库批次必须完整覆盖同一初始化的源目标集合")
        return copy.deepcopy(observed)

    def observe_database(self, key: str):
        if key not in self.initialized:
            raise ValueError("不能观察未登记的数据库")
        self._source_inventory()
        return self._inventory("target")[key]

    def observe_object(self, bucket: str, key: str):
        item = next((item for item in self.plan["objects"] if (item["bucket"], item["target_key"]) == (bucket, key)), None)
        if item is None:
            raise ValueError("不能观察未登记的对象")
        with self.environment.use("source") as environment:
            observe_source_object(self.backend, self.tools, self.source_verified, bucket, item["source_key"],
                                  self.directory("source-object"), environment=environment)
        expected = {field: item["artifact"][field] for field in ("bytes", "sha256")}
        with self.environment.use("target"):
            result = observe_target_object(self.backend, self.tools, bucket, key, self.directory("target-object"),
                                           expected=expected, max_bytes=expected["bytes"])
        self.latest_objects[bucket, key] = result
        return result

    def _object_items(self, targets: list[tuple[str, str]]) -> list[dict]:
        planned = {(item["bucket"], item["target_key"]): item for item in self.plan["objects"]}
        if (not isinstance(targets, list) or not 1 <= len(targets) <= WORKERS
                or any(not isinstance(key, tuple) or key not in planned for key in targets)
                or len(set(targets)) != len(targets)):
            raise ValueError("对象观察批次必须精确属于当前计划且最多四项")
        return [planned[key] for key in targets]

    def verify_object_bindings(self, targets: list[tuple[str, str]]) -> None:
        """批后代次检查完成后，重新核对本批原始对象证明。"""
        items = self._object_items(targets)
        with self.environment.use("source"):
            for item in items:
                verify_export_bindings(self.backend, self.source_verified, selection=(item["bucket"], item["source_key"]))

    def observe_objects(self, targets: list[tuple[str, str]]) -> list:
        """单批只读；环境切换、原始证明及结果登记只在父控制器执行。"""
        items = self._object_items(targets)
        source_outputs = [self.directory("source-object") for _ in items]
        target_outputs = [self.directory("target-object") for _ in items]

        def proof_bindings():
            for item in items:
                verify_export_bindings(self.backend, self.source_verified, selection=(item["bucket"], item["source_key"]))

        with self.environment.use("source") as environment:
            proof_bindings()
            ordered_batches(lambda task: observe_source_object(
                self.backend, self.tools, self.source_verified, task[0]["bucket"], task[0]["source_key"], task[1],
                environment=environment), list(zip(items, source_outputs, strict=True)), thread_name_prefix="copy-source-read")
            proof_bindings()
        with self.environment.use("target"):
            results = ordered_batches(lambda task: observe_target_object(
                self.backend, self.tools, task[0]["bucket"], task[0]["target_key"], task[1],
                expected={field: task[0]["artifact"][field] for field in ("bytes", "sha256")},
                max_bytes=task[0]["artifact"]["bytes"]), list(zip(items, target_outputs, strict=True)),
                thread_name_prefix="copy-target-read")
        with self.environment.use("source"):
            proof_bindings()
        self.latest_objects.update(zip(targets, results, strict=True))
        return results

    def _target_snapshot(self) -> None:
        current, _, resources = self._target_generation()
        with self.environment.use("target"):
            verify_target_heads(self.backend, self.tools, self.plan["objects"], self.latest_objects, self.directory("target-heads-final"))
            scope = self.target_request["target"]["scope_id"]
            for bucket in sorted(BUCKETS):
                expected = {scope + "/.ryframe-owner"} | {item["target_key"] for item in self.plan["objects"] if item["bucket"] == bucket}
                if resources.object_keys(bucket) != expected:
                    raise ValueError("目标完整对象范围包含遗漏、额外对象或 owner 缺失")
            resources.tools.verify_objects("target")
            if resources.redis_state(initialized=True, sentinel=True) != current["redis"]:
                raise ValueError("未启动目标的 Redis 已偏离初始化状态")
        actual = self._inventory("target")
        expected = {key: replace(value, tables=copy.deepcopy(self.source_initial[key].tables)) for key, value in self.initialized.items()}
        if actual != expected:
            raise ValueError("最终目标完整业务、保留表或 ownership 与来源及初始化像不符")

    def verify_complete_snapshot(self) -> None:
        self._source_generation()
        self._source_inventory()
        with self.environment.use("source") as environment:
            observe_source_objects(self.backend, self.tools, self.source_verified, self.directory("source-objects-final"), environment=environment)
        self._source_inventory()
        self._target_snapshot()
        self._source_generation()


def copy_to_fresh_target(backend: Path, source_export: dict, initialized: dict, source_environment: dict,
                         target_environment: dict, output: Path, *, copy_id: str, stage: str, run=subprocess.run,
                         source_storage_run: Path | None = None,
                         target_storage_run: Path | None = None) -> dict:
    """首次显式复制；失败保留账本与资源，不自动清理、重试或接管已有目标。"""
    session = None
    try:
        session = CloneSession(
            backend, source_export, initialized, source_environment, target_environment, output, run,
            source_storage_run=source_storage_run, target_storage_run=target_storage_run,
        )
        session.prepare(copy_id, stage)
        owner = register_copy_owner(session.backend, session.output)
        session.copy_owner = owner
        with session.environment.use("target"), generation_lock(session.target_directory, copy_owner=owner):
            session.owned_lock_identity = (session.target_directory / "initialize.lock").stat().st_ino
            session.verify_target_owners()
            ledger = CloneLedger(session.backend, session.output / "ledger", session.plan["plan_sha256"],
                                 session.generation_sha256, create=True, copy_owner=owner)
            with ledger:
                steps = TransferSteps(session.backend, session.plan_file, session.tools, ledger, session, verified_plan=session.verified_plan)
                for item in session.plan["databases"]:
                    steps.apply_database(item["key"])
                for item in session.plan["objects"]:
                    steps.apply_object(item["bucket"], item["target_key"])
                steps.finish()
            result = steps.result()
        session.owned_lock_identity = None
        # 仅数据步骤已复核；调度处置、业务消费者和性能来源证明仍由后续显式流程完成。
        write_json(session.output / "result.json", result)
        return result
    except BaseException as error:
        if session is not None:
            session.owned_lock_identity = None
            write_json(session.output / "failure.json", {"status": "needs_reconciliation", "error_type": type(error).__name__,
                       "automatic_retry": False, "automatic_resource_cleanup": False, "target_ready": False, "restore_success": False})
        raise
