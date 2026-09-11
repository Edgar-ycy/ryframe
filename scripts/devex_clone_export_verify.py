"""复核外部已绑定的源导出，并只读观察完整源对象目录或单个已登记对象。

完整验证读取本地 SQL 和对象字节；后续绑定检查只重验小型原始证据，当前操作的
payload 仍须由 TransferSteps 单独校验。HEAD 复核不下载业务对象，不把 ETag 当 SHA。
"""
from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import subprocess
from typing import Mapping
import uuid

from devex_clone_capture import CaptureReader, inspect_response, read_json, regular_file, unique_object, verify_capture, write_json
from devex_clone_export import logical_inventory, schema_models, validate_dump_state, validate_inventory
from devex_clone_inventory import configuration
from devex_clone_model import bound_file, digest, exact, linked, local_path
from devex_clone_object_batch import ordered_batches
from devex_clone_rows import schema_catalog
from devex_clone_source_proof import bound_file as request_file, validate_request
from restore_build import file_digest
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import BUCKETS, plan_hash
from restore_source_binding import source_binding
from source_fingerprints import build_source

EVIDENCE = {"request", "generation-before", "generation-after", "inventory-before", "inventory-after", "schema-before", "schema-after"}
EXPORT_FIELDS = {"format_version", "kind", "status", "id", "scope_id", "request", "source_snapshot", "worktree_fingerprint",
                 "source", "artifact_root", "databases", "objects", "enabled_system_schedule_rows", "logical_inventory_sha256",
                 "evidence", "catalog_sha256", "quiesced_at", "completed_at", "requires_offline_plan", "operator_declared_producers_only",
                 "remote_writes", "clone_verified", "restore_qualified", "dump_semantic_digest_verified", "dump_validation"}


def no_failure(directory: Path) -> None:
    marker = directory / "failure.json"
    if marker.exists() or linked(marker):
        raise ValueError("源证据包含失败标记，不能使用残留成功文件")


def bound_export(backend: Path, binding: dict) -> tuple[Path, dict]:
    path = request_file(backend, binding)
    if path.name != "export.json":
        raise ValueError("源导出必须绑定正式 export.json")
    no_failure(path.parent)
    value = read_json(path)
    exact(value, EXPORT_FIELDS)
    if (value["format_version"] != 1 or value["kind"] != "devex-clone-source-export" or value["status"] != "source_export_captured"
            or local_path(backend, value["artifact_root"]) != path.parent
            or any(value[field] is not False for field in ("clone_verified", "restore_qualified", "dump_semantic_digest_verified"))
            or any(value[field] is not True for field in ("requires_offline_plan", "operator_declared_producers_only"))
            or type(value["remote_writes"]) is not int or value["remote_writes"] != 0):
        raise ValueError("源导出类型、目录或实际验证状态不符")
    return path.parent, value


def saved_evidence(root: Path, value: dict) -> dict:
    exact(value["evidence"], EVIDENCE)
    result = {}
    for key, binding in value["evidence"].items():
        if binding["file"] != key + ".json":
            raise ValueError("源原始证据名称与固定阶段不符")
        result[key] = read_json(bound_file(root, binding))
    return result


def verify_schema(value: dict, request: dict, models: tuple) -> None:
    exact(value, {"databases"})
    entries = value["databases"]
    declared = {item["key"]: item for item in request["source"]["databases"]}
    if not isinstance(entries, list) or len(entries) != len(declared) or {item["key"] for item in entries} != set(declared):
        raise ValueError("源 schema 必须完整覆盖所有登记数据库")
    for item in entries:
        exact(item, {"key", "columns", "sha256"})
        db = declared[item["key"]]
        known = {**(models[2] if db["kind"] == "combined" else {}), **models[3]}
        ledgers = {"seaql_tenant_data_migrations"} | ({"seaql_migrations"} if db["kind"] == "combined" else set())
        columns = item["columns"]
        if (not isinstance(columns, dict) or set(columns) != set(known) | ledgers
                or any(columns[name] != fields for name, fields in known.items())
                or any(not isinstance(fields, dict) or not fields or any(not isinstance(k, str) or not isinstance(v, str) for k, v in fields.items())
                       for fields in columns.values()) or plan_hash(columns) != digest(item["sha256"])):
            raise ValueError("源 schema 与当前完整 catalog 或原摘要不同")


def verify_generation_evidence(value: dict, request: dict, evidence: dict, models: tuple) -> dict:
    before, after = evidence["generation-before"], evidence["generation-after"]
    if (before != after or before["source"] != value["source_snapshot"]
            or before["worktree_fingerprint"] != value["worktree_fingerprint"]
            or before["request_sha256"] != plan_hash(request) or request["worktree_fingerprint"] != value["worktree_fingerprint"]
            or before["maintenance"]["source"]["snapshot"] != before["source"]
            or before["maintenance"]["source"]["worktree_fingerprint"] != value["worktree_fingerprint"]
            or value["catalog_sha256"] != plan_hash({"control": models[0], "tenant": models[1]})):
        raise ValueError("源前后代次、构建来源或当前 catalog 不一致")
    for phase in ("before", "after"):
        validate_inventory(evidence["inventory-" + phase], request, models, before["source"]["head"], value["quiesced_at"])
        verify_schema(evidence["schema-" + phase], request, models)
    if (logical_inventory(evidence["inventory-before"]) != logical_inventory(evidence["inventory-after"])
            or plan_hash(logical_inventory(evidence["inventory-before"])) != value["logical_inventory_sha256"]
            or evidence["schema-before"] != evidence["schema-after"]):
        raise ValueError("源前后完整逻辑清单或 schema 不一致")
    return before


def verify_dump_declarations(root: Path, value: dict, inventory: dict, models: tuple) -> None:
    """核对完整转储声明与来源库存；实际行数由后续同一遍状态扫描证明。"""
    declared = {item["key"]: item for item in value["source"]["databases"]}
    dumps = value["databases"]
    if not isinstance(dumps, list) or len(dumps) != len(declared) or {item["key"] for item in dumps} != set(declared):
        raise ValueError("转储文件没有精确覆盖全部数据库")
    baseline = {item["key"]: item for item in inventory["databases"]}
    for item in dumps:
        exact(item, {"key", "tables", "artifact"})
        catalog = {**(models[0] if declared[item["key"]]["kind"] == "combined" else {}), **models[1]}
        bound_file(root, item["artifact"])
        if (not isinstance(item["tables"], dict) or set(item["tables"]) != set(catalog)
                or any(type(count) is not int or count < 0 for count in item["tables"].values())):
            raise ValueError("转储缺少空表或包含保留表")
        expected = {table["table"]: table["rows"] for table in baseline[item["key"]]["tables"] if table["table"] in catalog}
        if item["tables"] != expected:
            raise ValueError("转储登记行数与源完整 inventory 不同")


def capture_proof_names() -> set[str]:
    names = {"intent.json", "capture.json"}
    for phase in ("head-before", "get", "head-after"):
        names |= {phase + ".json", phase + ".diagnostic.json"}
    for phase in ("owner-before", "owner-after"):
        for bucket in BUCKETS:
            names |= {f"{phase}-{bucket}.{suffix}" for suffix in ("bin", "json", "diagnostic.json")}
    return names


def capture_proofs(directory: Path) -> list[Path]:
    return [regular_file(directory / name) for name in sorted(capture_proof_names())]


def verify_captures(root: Path, value: dict, request: dict, inventory: dict) -> tuple[dict, dict, list[Path]]:
    buckets = value["objects"]
    if not isinstance(buckets, list) or len(buckets) != len(BUCKETS) or {item["bucket"] for item in buckets} != BUCKETS:
        raise ValueError("源对象导出没有完整覆盖五桶")
    tools = ExternalTools({"source": request["source"], "tools": request["tools"]}, root)
    baseline = {bucket["bucket"]: {item["key"]: item for item in bucket["entries"]} for bucket in inventory["objects"]}
    captures, logical, proofs = {}, {}, []
    for bucket in buckets:
        exact(bucket, {"bucket", "entries"})
        name, entries = bucket["bucket"], bucket["entries"]
        if not isinstance(entries, list) or len(entries) != len(baseline[name]) or {item["key"] for item in entries} != set(baseline[name]):
            raise ValueError("源对象导出缺失、重复或超出完整 inventory")
        captures[name] = {}
        for item in entries:
            exact(item, {"key", "artifact", "metadata", "capture", "get_header_consistent"})
            body, receipt = bound_file(root, item["artifact"]), bound_file(root, item["capture"])
            if receipt.name != "capture.json" or body != receipt.parent / "object.bin":
                raise ValueError("对象字节与采集收据不是同一明确目录")
            expected = {field: baseline[name][item["key"]][field] for field in ("bytes", "sha256")}
            capture = verify_capture(tools, "source", name, item["key"], receipt.parent,
                                     expected=expected, max_bytes=request["max_object_bytes"])["capture"]
            if capture["metadata"] != item["metadata"] or capture["get_header_consistent"] is not item["get_header_consistent"]:
                raise ValueError("对象完整存储元数据或 GET 差异标记与源证据不同")
            captures[name][item["key"]] = capture
            logical[name, item["key"].removeprefix(value["scope_id"] + "/")] = item["artifact"]
            proofs.extend(capture_proofs(receipt.parent))
    return captures, logical, proofs


def verify_source_export(backend: Path, binding: dict) -> dict:
    """完整复核已绑定 A 产物；不连接源服务，不把转储行数当作语义摘要证明。"""
    backend = backend.resolve(strict=True)
    root, value = bound_export(backend, binding)
    evidence = saved_evidence(root, value)
    request_path = request_file(backend, value["request"])
    request = read_json(request_path)
    validate_request(backend, request)
    if request != evidence["request"] or request["source"] != value["source"] or request["id"] != value["id"] or request["source"]["scope_id"] != value["scope_id"]:
        raise ValueError("源导出与外部原请求不同")
    models = schema_models(backend)
    generation = verify_generation_evidence(value, request, evidence, models)
    build = read_json(request_file(backend, request["backend_build"]))
    if (generation["source"] != build_source(build)["snapshot"]
            or generation["maintenance"] != read_json(request_file(backend, request["maintenance_build"]))
            or generation["runtime"] != read_json(request_file(backend, request["runtime"]))
            or any(generation["processes"][role] != read_json(request_file(backend, request["processes"][role]))["identity"] for role in ("api", "worker"))):
        raise ValueError("源记录代次与外部已绑定构建、运行或进程收据不同")
    inventory = evidence["inventory-before"]
    verify_dump_declarations(root, value, inventory, models)
    captures, index, proofs = verify_captures(root, value, request, inventory)
    tools = ExternalTools({"source": request["source"]}, root)
    if validate_dump_state(tools, models, value["databases"], index) != value["enabled_system_schedule_rows"]:
        raise ValueError("源实际业务关系或调度处置与导出结果不同")
    proofs += [request_path, *[root / (name + ".json") for name in sorted(EVIDENCE)]]
    result = {"export": value, "binding": copy.deepcopy(binding), "request": request, "generation": generation,
              "inventory": inventory, "captures": captures,
              "proof_files": {str(path): file_digest(regular_file(path)) for path in proofs}}
    verify_export_bindings(backend, result)
    return result


def proof_dependencies(root: Path, value: dict, selection) -> tuple[dict, set[str]]:
    """只投影已绑定完整目录的依赖路径；实际文件检查留在选中阶段，不用 mtime 缓存。"""
    selected, all_paths = {}, set()
    if selection not in ("all", "global") and (
            not isinstance(selection, tuple) or len(selection) != 2
            or any(not isinstance(part, str) for part in selection)):
        raise ValueError("原始证据选择必须为完整、全局或明确单对象")
    for bucket in value["objects"]:
        for item in bucket["entries"]:
            relative = item["capture"]["file"]
            if (not isinstance(relative, str) or "\\" in relative or ":" in relative
                    or any(part in ("", ".", "..") for part in relative.split("/"))
                    or any(ord(char) < 32 for char in relative) or Path(relative).name != "capture.json"):
                raise ValueError("对象原始证据目录格式无效")
            directory = (root / relative).parent
            paths = {str(directory / name) for name in capture_proof_names()}
            all_paths.update(paths)
            identity = bucket["bucket"], item["key"]
            if selection == "all" or selection == identity:
                selected[identity] = (item, directory, paths)
    if isinstance(selection, tuple) and set(selected) != {selection}:
        raise ValueError("原始证据选择不属于已绑定完整对象目录")
    return selected, all_paths


def verify_export_bindings(backend: Path, verified: dict, *, selection="all") -> None:
    """每次重哈希全局证据与所选对象；阶段首尾使用 all，payload 仍由步骤独立校验。"""
    root, value = bound_export(backend, verified["binding"])
    exact(verified, {"export", "binding", "request", "generation", "inventory", "captures", "proof_files"})
    if value != verified["export"]:
        raise ValueError("调用方源导出对象被修改")
    if (verified["request"] != read_json(root / "request.json")
            or verified["generation"] != read_json(root / "generation-before.json")
            or verified["inventory"] != read_json(root / "inventory-before.json") or set(verified["captures"]) != BUCKETS):
        raise ValueError("调用方源证据快照被修改")
    global_paths = {str(request_file(backend, value["request"])), *[str(root / (name + ".json")) for name in EVIDENCE]}
    selected, object_paths = proof_dependencies(root, value, selection)
    expected_paths = global_paths | object_paths
    for bucket in value["objects"]:
        if set(verified["captures"][bucket["bucket"]]) != {item["key"] for item in bucket["entries"]}:
            raise ValueError("调用方对象目录被修改")
    if set(verified["proof_files"]) != expected_paths:
        raise ValueError("调用方原始小型证据集合不完整或包含额外项")
    control, tenant = schema_catalog(backend)
    if value["catalog_sha256"] != plan_hash({"control": control, "tenant": tenant}):
        raise ValueError("当前源 catalog 变化")
    selected_paths = global_paths | {filename for _, _, paths in selected.values() for filename in paths}
    for filename in sorted(selected_paths):
        expected = verified["proof_files"][filename]
        path = regular_file(local_path(backend, filename))
        if file_digest(path) != expected:
            raise ValueError("源原始小型证据已变化")
    for (bucket, key), (item, directory, _) in selected.items():
        if bound_file(root, item["capture"]).parent != directory:
            raise ValueError("对象原始证据目录与已绑定目录不同")
        no_failure(directory)
        if read_json(directory / "capture.json") != verified["captures"][bucket][key]:
            raise ValueError("调用方对象采集快照被修改")
    request_file(backend, verified["binding"])
    no_failure(root)


class SourceObjectObservationError(RuntimeError):
    def __init__(self, output: Path):
        self.evidence_directory = str(output)
        super().__init__(f"源对象只读观察未通过；证据保留于 {output}")


class _SourceTools(ExternalTools):
    def __init__(self, session):
        super().__init__(session.plan, session.output, session.run)
        self.environment = session.aws_environment

    def aws_context(self, side: str) -> tuple[dict, dict]:
        if side != "source":
            raise ValueError("源观察不能请求目标环境")
        return self.plan["source"]["s3"], dict(self.environment)


class _SourceReader(CaptureReader):
    def __init__(self, session):
        self.tools = _SourceTools(session)
        self.side, self.output = "source", session.output


class _Observation:
    def __init__(self, backend, tools, verified, output, environment):
        self.backend, self.original, self.verified, self.output = backend, tools, verified, output
        self.original_environment, self.environment = environment, dict(environment)
        self.plan = copy.deepcopy(tools.plan)
        source, request = verified["export"]["source"], verified["request"]
        if (self.plan.get("source") != source or self.plan["tools"]["aws"] != request["tools"]["aws"]
                or any(not isinstance(k, str) or not isinstance(v, str) for k, v in self.environment.items())):
            raise ValueError("源对象工具、scope 或显式环境不同")
        s3 = source["s3"]
        access, secret = (self.environment.get(s3[field]) for field in ("access_key_env", "secret_key_env"))
        if not access or not secret:
            raise ValueError("源对象缺少明确认证环境")
        self.aws_environment = {k: v for k, v in self.environment.items()
                                if not k.upper().startswith("AWS_") and k.upper() not in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY")}
        self.aws_environment.update(AWS_ACCESS_KEY_ID=access, AWS_SECRET_ACCESS_KEY=secret, AWS_DEFAULT_REGION=s3["region"],
            AWS_EC2_METADATA_DISABLED="true", AWS_CONFIG_FILE=os.devnull, AWS_SHARED_CREDENTIALS_FILE=os.devnull,
            AWS_PAGER="", AWS_MAX_ATTEMPTS="1")
        self.initial = self.binding()
        self.reader = _SourceReader(self)

    def binding(self):
        if self.original.plan != self.plan or dict(self.original_environment) != self.environment:
            raise ValueError("源计划或显式环境在请求期间变化")
        self.original.command("aws")
        source = self.plan["source"]
        return {"plan_sha256": plan_hash(self.plan), "environment_sha256": plan_hash(self.aws_environment),
                "physical": source_binding(self.backend, {"source": source}, self.environment),
                "configuration": configuration(self.backend, self.environment, source)}

    def run(self, command, **kwargs):
        if self.binding() != self.initial or kwargs.get("env") != self.aws_environment:
            raise ValueError("源只读请求前的绑定变化")
        kwargs["env"] = dict(self.aws_environment)
        try:
            return self.original.run(command, **kwargs)
        finally:
            if self.binding() != self.initial:
                raise ValueError("源只读请求期间的绑定变化")

    def keys(self, bucket: str, phase: str) -> set[str]:
        scope = self.plan["source"]["scope_id"] + "/"
        found, cursors, cursor = set(), set(), None
        for index in range(10000):
            config, env = self.reader.tools.aws_context("source")
            command = [*self.reader.tools.command("aws"), "--endpoint-url", config["endpoint"], "--region", config["region"],
                       "--no-paginate", "s3api", "list-objects-v2", "--bucket", bucket, "--prefix", scope, "--max-keys", "1000"]
            if cursor:
                command += ["--continuation-token", cursor]
            filename = self.output / f"list-{phase}-{bucket}-{index}.json"
            diagnostic = filename.with_suffix(".diagnostic.json")
            code, stderr, error_type = None, b"", None
            with diagnostic.open("x", encoding="utf-8", newline="\n") as stream:
                try:
                    response = self.reader.tools.execute(command, env=env)
                    code, stderr = response.returncode, response.stderr
                    page = json.loads(response.stdout, object_pairs_hook=unique_object)
                    write_json(filename, page)
                except (subprocess.SubprocessError, OSError, ValueError) as error:
                    code, stderr, error_type = getattr(error, "returncode", code), getattr(error, "stderr", stderr), type(error).__name__
                    raise
                finally:
                    json.dump({"returncode": code, "error_type": error_type, "stderr": redact_object_diagnostic(stderr, env)}, stream)
                    stream.write("\n")
                    stream.flush()
                    os.fsync(stream.fileno())
            if (not isinstance(page, dict) or type(page.get("IsTruncated")) is not bool or not isinstance(page.get("Contents", []), list)
                    or page.get("Name", bucket) != bucket or page.get("Prefix", scope) != scope or page.get("CommonPrefixes")):
                raise ValueError("源对象完整分页范围或完成状态不明确")
            for item in page.get("Contents", []):
                key = item.get("Key")
                if not isinstance(key, str) or not key.startswith(scope) or key in found:
                    raise ValueError("源对象列表出现重复或越界 key")
                found.add(key)
            if not page["IsTruncated"]:
                return found
            cursor = page.get("NextContinuationToken")
            if not isinstance(cursor, str) or not cursor or cursor in cursors:
                raise ValueError("源对象列表游标重复或缺失")
            cursors.add(cursor)
        raise ValueError("源对象完整分页超过有界限制")

    def head(self, bucket: str, key: str) -> dict:
        expected = self.verified["captures"][bucket][key]
        raw = self.reader.read("head-" + uuid.uuid4().hex, "head-object", bucket, key, identity=expected["identity"])
        actual = inspect_response(raw)
        if (actual != {field: expected[field] for field in ("identity", "metadata")}
                or plan_hash(raw) != expected["response_sha256"]["head_before"]):
            raise ValueError("源当前 HEAD 身份或完整存储元数据与已采集对象不同")
        return {"bucket": bucket, "key": key, "head_sha256": plan_hash(raw), "identity": actual["identity"]}


def _observe(backend, tools, verified, output, environment, selected):
    backend = backend.resolve(strict=True)
    output = local_path(backend, str(output), new=True)
    if not output.parent.is_dir():
        raise ValueError("源观察须使用已有父目录中的新目录")
    output.mkdir()
    stage = "source_bindings"
    try:
        verify_export_bindings(backend, verified, selection=selected if selected is not None else "all")
        observation = _Observation(backend, tools, verified, output, environment)
        write_json(output / "intent.json", {"export_binding": verified["binding"], "binding": observation.initial,
                   "selected": selected, "business_get_permitted": False})
        stage = "owners_before"
        owner_buckets = tuple(sorted(BUCKETS)) if selected is None else (selected[0],)
        observation.reader.owners("owner-before", owner_buckets)
        if selected is None:
            stage = "complete_listing_before"
            before = {bucket: observation.keys(bucket, "before") for bucket in sorted(BUCKETS)}
            expected = {bucket: set(verified["captures"][bucket]) | {tools.plan["source"]["scope_id"] + "/.ryframe-owner"} for bucket in BUCKETS}
            if before != expected:
                raise ValueError("源五桶完整对象集合已变化")
            entries = [(bucket, key) for bucket in sorted(BUCKETS) for key in sorted(verified["captures"][bucket])]
        else:
            entries = [selected]
        stage = "heads"
        observed = (ordered_batches(lambda item: observation.head(*item), entries, thread_name_prefix="source-head")
                    if selected is None else [observation.head(*selected)])
        if selected is None:
            stage = "complete_listing_after"
            if {bucket: observation.keys(bucket, "after") for bucket in sorted(BUCKETS)} != before:
                raise ValueError("源 HEAD 采集期间完整 key 集合变化")
        stage = "owners_after"
        observation.reader.owners("owner-after", owner_buckets)
        verify_export_bindings(backend, verified, selection=selected if selected is not None else "all")
        if observation.binding() != observation.initial:
            raise ValueError("源观察结束时绑定变化")
        result = {"status": "source_object_identities_observed", "whole_source": selected is None,
                  "export_binding": verified["binding"], "observed": observed, "identity_sha256": plan_hash(observed),
                  "business_get_requests": 0, "business_bytes_downloaded": 0, "source_body_sha_recomputed": False,
                  "remote_writes": 0, "clone_verified": False, "restore_qualified": False}
        write_json(output / "observation.json", result)
        return result
    except Exception as error:
        write_json(output / "failure.json", {"status": "source_object_observation_failed", "stage": stage,
                   "error_type": type(error).__name__, "remote_writes": 0, "clone_verified": False, "restore_qualified": False})
        raise SourceObjectObservationError(output) from None


def observe_source_objects(backend: Path, tools: ExternalTools, verified: dict, output: Path, *, environment: Mapping[str, str]) -> dict:
    """完整五桶前后分页、owner 和所有当前 HEAD；不重新下载源业务字节。"""
    return _observe(backend, tools, verified, output, environment, None)


def observe_source_object(backend: Path, tools: ExternalTools, verified: dict, bucket: str, key: str,
                          output: Path, *, environment: Mapping[str, str]) -> dict:
    """只观察本次已验证导出中的精确单对象；404、403 和未知结果均失败，不重试。"""
    if bucket not in BUCKETS or key not in verified["captures"].get(bucket, {}):
        raise ValueError("源单对象观察必须来自已绑定导出的完整目录")
    return _observe(backend, tools, verified, output, environment, (bucket, key))
