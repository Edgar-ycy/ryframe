"""内部开发复制步骤引擎，由真实源导出和新目标初始化工厂提供当前观察。

LiveGuard 是集成边界，不是可从 JSON 构造的执行许可。组合入口必须以真实
源导出及 fresh-target 创建历史实现它；本模块的离线替身测试不证明资源可写。
"""
from __future__ import annotations

import copy
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol
import uuid

from devex_clone import VerifiedPlan, read_json, reuse_plan, verify_plan_result
from devex_clone_capture import capture_object, verify_capture
from devex_clone_ledger import CloneLedger, image, resource_digest
from devex_clone_model import bound_file, digest, local_path
from devex_clone_object_batch import WORKERS
from devex_clone_rows import EXCLUDED
from restore_build import file_digest
from restore_reference_io import ExternalTools, validate_dump
from restore_reference_plan import plan_hash

PRESERVED = EXCLUDED | {"seaql_tenant_data_migrations"}


@dataclass(frozen=True)
class GenerationObservation:
    plan_sha256: str
    generation_sha256: str
    input_sha256: str
    tools_binding_sha256: str
    source_export_sha256: str
    fresh_target_sha256: str
    source_stopped: bool
    target_stopped: bool
    target_never_started: bool


@dataclass(frozen=True)
class DatabaseObservation:
    resource: dict
    schema_sha256: str
    tables: dict
    preserved: dict
    all_tables: tuple[str, ...]
    ownership: tuple[dict, ...]


@dataclass(frozen=True)
class DatabaseBasis:
    source: DatabaseObservation
    initialized: DatabaseObservation
    artifact_sha256: str


@dataclass(frozen=True)
class ObjectObservation:
    resource: dict
    capture_directory: Path | None
    absence_evidence_sha256: str | None


class LiveGuard(Protocol):
    """实现须重新核对真实进程、资源、owner 和证据；不能只返回已存 JSON 的 SHA。

    generation 核对当前源停止与目标从未启动的代次、全部连接文件与环境秘密。
    具体实现开会话及 verify_complete_snapshot 收尾核对完整清单；逐资源观察另核
    相应源当前行像或对象身份。database_basis 绑定 dump 导出前后像及初始表像。
    observe_object 仅精确 404 可返回缺失，否则须先完成 HEAD/GET/HEAD capture。
    """
    def generation(self, plan: dict, declaration: dict, tools: ExternalTools, *,
                   source_object: tuple[str, str] | None = None) -> GenerationObservation: ...
    def database_basis(self, key: str) -> DatabaseBasis: ...
    def observe_database(self, key: str) -> DatabaseObservation: ...
    def observe_databases(self) -> dict[str, DatabaseObservation]: ...
    def observe_object(self, bucket: str, key: str) -> ObjectObservation: ...
    def observe_objects(self, targets: list[tuple[str, str]]) -> list[ObjectObservation]: ...
    def verify_object_bindings(self, targets: list[tuple[str, str]]) -> None: ...
    def verify_complete_snapshot(self) -> None: ...


def table_images(values: dict) -> dict:
    if not isinstance(values, dict):
        raise ValueError("数据库观察缺少逐表完整行像")
    for table, item in values.items():
        if (not isinstance(table, str) or not isinstance(item, dict) or set(item) != {"rows", "sha256"}
                or type(item["rows"]) is not int or item["rows"] < 0):
            raise ValueError("逐表观察必须包含明确行数和完整内容 SHA")
        digest(item["sha256"])
    return copy.deepcopy(values)


class TransferSteps:
    """只供已核验 live factory 组合使用；构造不会授予公开执行权限。

    调用方管理 CloneLedger 上下文，异常必须离开上下文；本实例异常后不再可用。
    finish 仅要求账本收尾，数据摘要须在排他锁释放与收据成功发布后读取。
    """
    def __init__(self, backend: Path, plan_file: Path, tools: ExternalTools, ledger: CloneLedger, guard: LiveGuard, *,
                 verified_plan: VerifiedPlan | None = None):
        self.backend = backend.resolve()
        self.plan_file = local_path(self.backend, str(plan_file))
        verified = verify_plan_result(self.backend, str(self.plan_file)) if verified_plan is None else verified_plan
        wrapper, self.declaration, self.bindings = reuse_plan(self.backend, str(self.plan_file), verified)
        self.plan = wrapper["plan"]
        self.input_file = local_path(self.backend, wrapper["input_path"])
        self.root = local_path(self.backend, self.declaration["artifact_root"])
        if guard is None or not isinstance(ledger, CloneLedger) or not ledger.active:
            raise ValueError("必须提供真实 guard 与当前持锁账本，离线计划不是执行许可")
        self.tools, self.ledger, self.guard = tools, ledger, guard
        self.work = local_path(self.backend, str(tools.work))
        self.tools_binding = plan_hash(tools.plan)
        self.generation = None
        self.failed = self.finished = False
        self.noops = {}
        self.bases = {}
        self.databases = {self.db_step(item): item for item in self.plan["databases"]}
        self.objects = {self.object_step(item): item for item in self.plan["objects"]}
        if len(self.databases) != len(self.plan["databases"]) or len(self.objects) != len(self.plan["objects"]):
            raise ValueError("数据步骤标识重复，不能省略计划资源")
        self._check()

    @staticmethod
    def db_step(item: dict) -> str:
        return "database-" + plan_hash({"key": item["key"]})[:32]

    @staticmethod
    def object_step(item: dict) -> str:
        return "object-" + plan_hash({"bucket": item["bucket"], "key": item["target_key"]})[:32]

    def _bindings(self) -> None:
        if local_path(self.backend, str(self.tools.work)) != self.work or not self.work.is_dir():
            raise ValueError("工具证据目录在步骤之间变化")
        if any(file_digest(local_path(self.backend, str(path))) != value for path, value in self.bindings.items()):
            raise ValueError("计划或输入在步骤之间变化")
        if (plan_hash({key: value for key, value in self.plan.items() if key != "plan_sha256"}) != self.plan["plan_sha256"]
                or plan_hash(self.declaration) != self.plan["input_sha256"] or plan_hash(self.tools.plan) != self.tools_binding):
            raise ValueError("输入或外部工具绑定变化")
        identities = {}
        for side in ("source", "target"):
            declared, actual = self.declaration[side], self.tools.plan[side]
            if declared["scope_id"] != actual["scope_id"] or declared["object_endpoint"] != actual["s3"]["endpoint"]:
                raise ValueError("工具 scope 或对象端点不属于离线计划")
            expected = {item["key"]: item for item in declared["databases"]}
            observed = {item["key"]: item for item in actual["databases"]}
            if set(expected) != set(observed) or len(observed) != len(actual["databases"]):
                raise ValueError("工具目标集合重复或不完整")
            for key, item in expected.items():
                if any(item[field] != observed[key][field] for field in ("key", "kind", "mode", "server_uuid", "database")):
                    raise ValueError("工具物理数据库不属于计划")
            identities[side] = {(item["server_uuid"], item["database"]) for item in observed.values()}
        if identities["source"] & identities["target"] or self.plan["source_scope"] == self.plan["target_scope"]:
            raise ValueError("源目标物理资源重叠")

    def _check(self, step: str | None = None) -> None:
        if self.failed or self.finished or not self.ledger.active or self.ledger.poisoned:
            raise ValueError("失败、已收尾或无锁步骤引擎不能继续")
        self._bindings()
        if step is not None and step not in self.databases | self.objects:
            raise ValueError("源码对象证明只能选择当前完整计划的数据步骤")
        item = self.objects.get(step)
        source_object = (item["bucket"], item["source_key"]) if item is not None else None
        observed = self.guard.generation(copy.deepcopy(self.plan), copy.deepcopy(self.declaration), self.tools,
                                         source_object=source_object)
        if not isinstance(observed, GenerationObservation):
            raise ValueError("缺少真实初始化代次观察，不能接收 ready 字典")
        for value in (observed.plan_sha256, observed.generation_sha256, observed.input_sha256,
                      observed.tools_binding_sha256, observed.source_export_sha256, observed.fresh_target_sha256):
            digest(value)
        if (observed.plan_sha256 != self.plan["plan_sha256"] or observed.input_sha256 != self.plan["input_sha256"]
                or observed.tools_binding_sha256 != self.tools_binding
                or self.ledger.binding != {"plan_sha256": observed.plan_sha256, "generation_sha256": observed.generation_sha256}
                or any(value is not True for value in (observed.source_stopped, observed.target_stopped, observed.target_never_started))):
            raise ValueError("真实来源、初始化代次、停止状态或工具绑定不完整")
        if self.generation is not None and observed != self.generation:
            raise ValueError("初始化代次或源导出证据变化")
        self.generation = observed
        self._bindings()

    def _resource(self, side: str, item: dict) -> dict:
        config = self.declaration[side]
        if "bucket" in item:
            return {"kind": "object", "scope_id": config["scope_id"], "endpoint": config["object_endpoint"],
                    "bucket": item["bucket"], "key": item[f"{side}_key"]}
        database = next(entry for entry in config["databases"] if entry["key"] == item["key"])
        return {"kind": "database", "scope_id": config["scope_id"],
                "server_uuid": database["server_uuid"], "database": database["database"]}

    def _database_image(self, observed: DatabaseObservation, side: str, item: dict) -> dict:
        if not isinstance(observed, DatabaseObservation) or observed.resource != self._resource(side, item):
            raise ValueError("数据库观察物理身份不匹配")
        declared = next(entry for entry in self.declaration[side]["databases"] if entry["key"] == item["key"])
        tables, preserved = table_images(observed.tables), table_images(observed.preserved)
        if (observed.schema_sha256 != declared["schema_sha256"] or set(tables) != set(item["tables"])
                or set(tables) & PRESERVED or not set(preserved).issubset(PRESERVED)
                or "ryframe_resource_ownership" not in preserved
                or len(observed.all_tables) != len(set(observed.all_tables))
                or set(observed.all_tables) != set(tables) | set(preserved)
                or sorted(observed.ownership, key=plan_hash) != sorted(declared["ownership"], key=plan_hash)):
            raise ValueError("数据库完整表集、schema、排除表或 ownership 不匹配")
        return image({"kind": "database", "resource_sha256": resource_digest(self.generation.generation_sha256, observed.resource),
                      "schema_sha256": observed.schema_sha256, "tables_sha256": plan_hash(tables),
                      "preserved_sha256": plan_hash(preserved)})

    def _database_basis(self, item: dict) -> tuple[dict, dict]:
        basis = self.guard.database_basis(item["key"])
        if not isinstance(basis, DatabaseBasis) or basis.artifact_sha256 != item["artifact"]["sha256"]:
            raise ValueError("数据库导出前后像未绑定本计划 SQL artifact")
        source = self._database_image(basis.source, "source", item)
        before = self._database_image(basis.initialized, "target", item)
        if {name: value["rows"] for name, value in basis.source.tables.items()} != item["tables"]:
            raise ValueError("源完整观察行数不匹配离线 SQL")
        if item["key"] in self.bases and basis != self.bases[item["key"]]:
            raise ValueError("绑定同代次的源或初始化完整前像变化")
        self.bases[item["key"]] = copy.deepcopy(basis)
        return before, {**before, "tables_sha256": source["tables_sha256"]}

    def _object_image(self, observed: ObjectObservation, item: dict) -> dict:
        if not isinstance(observed, ObjectObservation) or observed.resource != self._resource("target", item):
            raise ValueError("对象观察身份不匹配")
        result = {"kind": "object", "resource_sha256": resource_digest(self.generation.generation_sha256, observed.resource),
                  "exists": False, "bytes": None, "sha256": None, "metadata_sha256": None}
        if observed.capture_directory is None:
            digest(observed.absence_evidence_sha256)
            return image(result)
        if observed.absence_evidence_sha256 is not None:
            raise ValueError("对象不能同时声明不存在及采集成功")
        directory = local_path(self.backend, str(observed.capture_directory))
        request = read_json(directory / "intent.json")
        verified = verify_capture(self.tools, "target", item["bucket"], item["target_key"], directory,
                                  expected=request["expected"], max_bytes=request["max_bytes"], owner_buckets=(item["bucket"],))["capture"]
        metadata = verified["metadata"]
        if verified["get_header_differences"]:
            metadata = {"stored": metadata, "get_header_differences": verified["get_header_differences"]}
        return image({**result, "exists": True, "bytes": verified["artifact"]["bytes"],
                      "sha256": verified["artifact"]["sha256"], "metadata_sha256": plan_hash(metadata)})

    def _object_after(self, item: dict) -> dict:
        return image({"kind": "object", "resource_sha256": resource_digest(self.generation.generation_sha256, self._resource("target", item)),
                      "exists": True, "bytes": item["artifact"]["bytes"], "sha256": item["artifact"]["sha256"],
                      "metadata_sha256": plan_hash(item["metadata"])})

    def _artifact(self, item: dict) -> Path:
        filename = bound_file(self.root, item["artifact"])
        if "tables" in item:
            if set(item["tables"]) & PRESERVED or item["preserve_target_ownership"] is not True:
                raise ValueError("复制 SQL 包含排除表或未声明保留目标 owner")
            validate_dump(filename, set(item["tables"]))
        return filename

    def _observe(self, step: str) -> dict:
        if step in self.databases:
            item = self.databases[step]
            return self._database_image(self.guard.observe_database(item["key"]), "target", item)
        item = self.objects[step]
        return self._object_image(self.guard.observe_object(item["bucket"], item["target_key"]), item)

    def _images(self, step: str, item: dict) -> tuple[dict, dict]:
        if step in self.databases:
            return self._database_basis(item)
        after = self._object_after(item)
        return {**after, "exists": False, "bytes": None, "sha256": None, "metadata_sha256": None}, after

    def _existing(self, step: str, item: dict, before: dict, after: dict) -> dict:
        existing = self.ledger.snapshot()["steps"].get(step)
        if existing is None or (existing["before"], existing["after"], existing["artifact_sha256"]) != (before, after, item["artifact"]["sha256"]):
            raise ValueError("已有步骤不匹配本计划原 intent 与真实前后像")
        return existing

    def _write(self, step: str, item: dict, artifact: Path) -> dict:
        # 没有通用 write callback：SQL 和对象参数全部从完整离线计划派生。
        if step in self.databases:
            target = next(entry for entry in self.tools.plan["target"]["databases"] if entry["key"] == item["key"])
            self.tools.restore_database(copy.deepcopy(target), sorted(item["tables"]), artifact)
            return self._observe(step)
        expected = {key: item["artifact"][key] for key in ("bytes", "sha256")}
        self.tools.create_object_if_absent(item["bucket"], item["target_key"], artifact, copy.deepcopy(item["metadata"]), expected)
        output = local_path(self.backend, str(self.tools.work / ("transfer-capture-" + uuid.uuid4().hex)), new=True)
        capture_object(self.tools, "target", item["bucket"], item["target_key"], output,
                       expected=expected, max_bytes=expected["bytes"], owner_buckets=(item["bucket"],))
        return self._object_image(ObjectObservation(self._resource("target", item), output, None), item)

    def _apply(self, step: str, *, resume: bool) -> str:
        try:
            self._check(step)
            item = (self.databases | self.objects)[step]
            artifact = self._artifact(item)
            before, after = self._images(step, item)
            observed = self._observe(step)
            if observed != before:
                raise ValueError("当前资源不是完整初始前像，禁止接管或重放")
            existing = self.ledger.snapshot()["steps"].get(step)
            if before == after and not resume and existing is None:
                self._check(step)
                self.noops[step] = before
                return "unchanged_verified"
            if resume:
                self._existing(step, item, before, after)
                self.ledger.resume(step, observed)
            else:
                self.ledger.intent(step, before, after, item["artifact"]["sha256"])
            self._check(step)
            self._artifact(item)
            result = self._write(step, item, artifact)
            self._check(step)
            self._artifact(item)
            self.ledger.confirm(step, result)
            return "confirmed"
        except BaseException:
            self.failed = True
            raise

    def apply_database(self, key: str) -> str:
        item = next((item for item in self.databases.values() if item["key"] == key), None)
        if item is None:
            raise ValueError("数据库不在完整离线计划中")
        return self._apply(self.db_step(item), resume=False)

    def apply_object(self, bucket: str, target_key: str) -> str:
        item = next((item for item in self.objects.values() if (item["bucket"], item["target_key"]) == (bucket, target_key)), None)
        if item is None:
            raise ValueError("对象不在完整离线计划中")
        return self._apply(self.object_step(item), resume=False)

    def reconcile(self, step: str) -> str:
        try:
            self._check(step)
            if step not in self.databases | self.objects or step not in self.ledger.snapshot()["steps"]:
                raise ValueError("核对只能使用本完整计划已有 intent")
            item = (self.databases | self.objects)[step]
            self._artifact(item)
            self._existing(step, item, *self._images(step, item))
            observed = self._observe(step)
            self._check(step)
            return self.ledger.reconcile(step, observed)
        except BaseException:
            self.failed = True
            raise

    def resume(self, step: str) -> str:
        if step not in self.databases | self.objects:
            raise ValueError("续跑步骤不属于完整计划")
        return self._apply(step, resume=True)

    def verify_existing(self, step: str) -> None:
        """已确认步骤仍核对当前完整后像，不能因历史成功而跳过观察或重写。"""
        try:
            self._check(step)
            item = (self.databases | self.objects)[step]
            self._artifact(item)
            existing = self._existing(step, item, *self._images(step, item))
            if existing["phase"] != "confirmed" or self._observe(step) != existing["after"]:
                raise ValueError("已确认步骤当前后像不同，必须停止并显式核对")
            self._check(step)
        except BaseException:
            self.failed = True
            raise

    def verify_pending(self, step: str) -> None:
        """核对未登记 intent 的资源仍处于初始化前像；只读，不推断写入成功。"""
        try:
            self._check(step)
            if step in self.ledger.snapshot()["steps"]:
                raise ValueError("已有 intent 只能通过账本步骤核对")
            item = (self.databases | self.objects)[step]
            self._artifact(item)
            before, _ = self._images(step, item)
            if self._observe(step) != before:
                raise ValueError("未登记写入的资源已偏离初始化前像，禁止接管")
            self._check(step)
        except BaseException:
            self.failed = True
            raise

    def _batch_ledger(self) -> dict:
        if (not self.ledger.active or self.ledger.poisoned
                or self.ledger.path("clone.lock").stat().st_ino != self.ledger.lock_identity
                or self.ledger._read()[0] != self.ledger.frames):
            raise ValueError("只读批次的当前账本或排他锁发生变化")
        return {"snapshot": self.ledger.snapshot(), "files": {
            name: file_digest(self.ledger.path(name)) for name in ("head.json", "events.ndjson")}}

    def check_databases(self, mode: str, selected: list[str]) -> dict:
        """完整只读向量通过绑定核验后才分类；数据库写入仍独立观察。"""
        try:
            if (mode not in {"reconcile", "confirmed", "finish"} or not isinstance(selected, list)
                    or selected != [step for step in self.databases if step in selected]
                    or mode in {"reconcile", "finish"} and selected != list(self.databases)):
                raise ValueError("数据库只读批次必须选择按计划顺序排列的明确步骤")
            if not selected:
                return {}
            self._check()
            ledger = self._batch_ledger()
            states = ledger["snapshot"]["steps"]
            noops = copy.deepcopy(self.noops) if mode == "finish" else None
            if mode == "finish" and set(noops) != set(self.databases) - set(states):
                raise ValueError("收尾数据库 no-op 必须精确对应未登记 intent 的计划数据库")
            expected, bases = {}, {}
            for step in selected:
                item = self.databases[step]
                self._artifact(item)
                before, after = self._images(step, item)
                bases[step] = before, after
                if step in states:
                    existing = self._existing(step, item, before, after)
                    if mode in {"confirmed", "finish"} and existing["phase"] != "confirmed":
                        raise ValueError("已确认或收尾数据库批次不能接收未知或未确认步骤")
                elif mode == "confirmed":
                    raise ValueError("已确认数据库缺少本计划 intent")
                elif mode == "finish" and (noops.get(step) != before or before != after):
                    raise ValueError("只读核验的初始/来源像不再一致")
                expected[step] = before if mode == "reconcile" and step not in states else after
            observed = self.guard.observe_databases()
            images = self._database_vector(observed)
            self._check()
            for step in selected:
                self._artifact(self.databases[step])
                if self._images(step, self.databases[step]) != bases[step]:
                    raise ValueError("数据库批次的原始前后像绑定变化")
            if self._database_vector(observed) != images or self._batch_ledger() != ledger:
                raise ValueError("只读数据库批次期间观察或账本变化，不能分类或记账")
            if mode == "finish" and self.noops != noops:
                raise ValueError("只读数据库批次期间 no-op 绑定变化")
            for step in selected:
                if mode == "reconcile" and step in states:
                    continue
                if images[step] != expected[step]:
                    message = "未登记写入的资源已偏离初始化前像，禁止接管" if mode == "reconcile" else "已确认步骤当前后像不同，必须停止并显式核对"
                    raise ValueError(message)
            return {step: self.ledger.reconcile(step, images[step]) for step in selected
                    if mode == "reconcile" and step in states}
        except BaseException:
            self.failed = True
            raise

    def _database_vector(self, observed: dict) -> dict:
        if not isinstance(observed, dict) or set(observed) != {item["key"] for item in self.databases.values()}:
            raise ValueError("数据库只读向量缺少或包含额外的计划数据库")
        return {step: self._database_image(observed[item["key"]], "target", item)
                for step, item in self.databases.items()}

    def _object_batch(self, selected: list[str], mode: str) -> dict:
        self._check()
        ledger = self._batch_ledger()
        states = ledger["snapshot"]["steps"]
        expected = {}
        for step in selected:
            item = self.objects[step]
            self._artifact(item)
            before, after = self._images(step, item)
            if step in states:
                existing = self._existing(step, item, before, after)
                if mode == "confirmed" and existing["phase"] != "confirmed":
                    raise ValueError("已确认对象批次不能接收未知或未确认步骤")
            elif mode == "confirmed" or mode == "finish" and (self.noops.get(step) != before or before != after):
                raise ValueError("收尾或已确认对象缺少当前计划的有效前后像")
            expected[step] = before if mode == "reconcile" and step not in states else after
        targets = [(self.objects[step]["bucket"], self.objects[step]["target_key"]) for step in selected]
        observed = self.guard.observe_objects(targets)
        if not isinstance(observed, list) or len(observed) != len(selected):
            raise ValueError("对象批次观察缺失、重复或不是明确的有序结果")
        images = {step: self._object_image(value, self.objects[step])
                  for step, value in zip(selected, observed, strict=True)}
        self._check()
        self.guard.verify_object_bindings(targets)
        for step in selected:
            self._artifact(self.objects[step])
        if self._batch_ledger() != ledger:
            raise ValueError("只读对象批次期间账本发生变化，不能分类或记账")
        for step in selected:
            if mode == "reconcile" and step in states:
                continue
            if images[step] != expected[step]:
                message = "未登记写入的资源已偏离初始化前像，禁止接管" if mode == "reconcile" else "已确认步骤当前后像不同，必须停止并显式核对"
                raise ValueError(message)
        return {step: self.ledger.reconcile(step, images[step]) for step in selected if mode == "reconcile" and step in states}

    def check_objects(self, mode: str, selected: list[str]) -> dict:
        """只读对象批次；每批完整检查后在父控制器按计划顺序分类及记账。"""
        try:
            if (mode not in {"reconcile", "confirmed", "finish"} or not isinstance(selected, list)
                    or selected != [step for step in self.objects if step in selected]):
                raise ValueError("只读对象检查必须使用唯一、按计划顺序排列的明确步骤")
            result = {}
            for offset in range(0, len(selected), WORKERS):
                result.update(self._object_batch(selected[offset:offset + WORKERS], mode))
            return result
        except BaseException:
            self.failed = True
            raise

    def finish(self) -> None:
        try:
            self._check()
            steps = self.ledger.snapshot()["steps"]
            if set(steps) | set(self.noops) != set(self.databases) | set(self.objects):
                raise ValueError("完整计划仍有未执行或未只读核验的数据步骤")
            noops = copy.deepcopy(self.noops)
            self.check_databases("finish", list(self.databases))
            self.check_objects("finish", list(self.objects))
            self.guard.verify_complete_snapshot()
            self._check()
            if self.noops != noops:
                raise ValueError("完整收尾期间 no-op 绑定变化")
            if steps:
                self.ledger.finish()
            self.finished = True
        except BaseException:
            self.failed = True
            raise

    def result(self) -> dict:
        if self.failed or not self.finished or self.ledger.active:
            raise ValueError("数据步骤或排他锁收尾尚未完成")
        state = self.ledger.inspect()
        if state["steps"] and state["status"] != "ledger_evidence_complete":
            raise ValueError("账本成功收据未发布，不能单独发布数据步骤成功")
        if not state["steps"] and state["status"] != "partial":
            raise ValueError("只读步骤账本收尾失败")
        return {"status": "data_steps_verified", "plan_sha256": self.plan["plan_sha256"],
                "generation_sha256": self.generation.generation_sha256,
                "source_export_sha256": self.generation.source_export_sha256,
                "fresh_target_sha256": self.generation.fresh_target_sha256,
                "written_steps": len(state["steps"]), "unchanged_databases": len(self.noops),
                "unchanged_database_images": copy.deepcopy(self.noops),
                "worker_stopped": True, "target_ready": False, "restore_success": False,
                "pending_target_actions": copy.deepcopy(self.plan["pending_target_actions"]),
                "requires_final_live_verification": True}
