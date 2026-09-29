"""复用四目标、正式库存和对象采集器，保留源运行前后完整只读像。"""
from __future__ import annotations

from contextlib import nullcontext
from pathlib import Path
import datetime as dt
import hashlib

from devex_clone_capture import CaptureReader, owner_evidence, read_json, verify_capture, write_json
from devex_clone_export import capture_inventory, logical_inventory, schema_models, schema_snapshot, validate_inventory, verify_migrations
from devex_clone_export_verify import verify_schema
from devex_clone_inventory import KEYS, capture_side_inventory, verify_side_inventory
from devex_clone_object_batch import capture_objects
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_model import exact, local_path
from devex_clone_tools import verify as verify_tools
from devex_clone_target_binding import redis_configuration
from devex_clone_target_resources import Resources
from restore_reference_io import ExternalTools
from restore_reference_plan import BUCKETS, plan_hash


def capture_image(backend: Path, execution: Path, selected: dict, request: dict, source: dict,
                  environment: dict, output: Path, run, *, control_environment=None) -> dict:
    output.mkdir()
    predecessor = source["request"]
    target = source["seed_target"]
    review = read_json(bound_file(backend, {key: value for key, value in target["review"].items() if key != "canonical_sha256"}))
    seed = review["scopes"]["seed"]
    redis_configuration(target, seed)
    resources = Resources(backend, target, output, seed, review, run, execution_backend=execution,
                          storage_run=source["directory"])
    storage = resources.storage_identity()
    redis = resources.redis_state(initialized=True, sentinel=True)
    tools = ExternalTools({"source": selected, "tools": predecessor["tools"]}, output, run)
    maintenance_path = bound_file(execution, request["maintenance_build"])
    with control_environment() if control_environment else nullcontext():
        maintenance = verify_tools(execution, maintenance_path, run)
    build = read_json(bound_file(backend, request["backend_build"]))
    generation = {"source": build["sources"]["full"]["source"]["snapshot"], "maintenance": maintenance}
    export_request = {**predecessor, "source": selected}
    models = schema_models(execution)
    observed_at = dt.datetime.now(dt.timezone.utc).isoformat()
    verify_migrations(tools, execution, maintenance, "migrations")
    schema = schema_snapshot(tools, models, "before")
    captured = capture_side_inventory(execution, "source", tools, maintenance_path, output / "databases",
                                      environment=environment, evidence_root=backend, control_environment=control_environment)
    inventories = {key: read_json(output / f"databases/after-target-{key}.json") for key in KEYS}
    owners = CaptureReader(tools, "source", output).owners("owners-before")
    inventory = capture_inventory(tools, execution, export_request, generation, models, "before", observed_at, observed=True)
    objects, _ = capture_objects(tools, export_request, inventory)
    object_evidence, object_images = [], {}
    for bucket in objects:
        object_images[bucket["bucket"]] = {}
        for item in bucket["entries"]:
            receipt = output / item["capture"]["file"]
            observed = read_json(receipt)
            object_evidence.append(binding(receipt))
            object_images[bucket["bucket"]][item["key"]] = {
                "bytes": item["artifact"]["bytes"], "sha256": item["artifact"]["sha256"],
                "metadata": observed["metadata"], "identity": observed["identity"]}
    after = capture_inventory(tools, execution, export_request, generation, models, "after", observed_at, observed=True)
    repeated = CaptureReader(tools, "source", output).owners("owners-after")
    owner_image = lambda rows: [{key: item[key] for key in ("bucket", "key", "bytes", "sha256")} for item in rows]
    if (logical_inventory(after) != logical_inventory(inventory) or owner_image(owners) != owner_image(repeated)
            or resources.storage_identity() != storage or resources.redis_state(initialized=True, sentinel=True) != redis
            or schema_snapshot(tools, models, "after") != schema):
        raise ValueError("source-generation 采集期间完整库存、schema 或对象 ownership 变化")
    value = {"format_version": 1, "kind": "seed-source-generation-image",
             "source_registration": source["review_successor"]["source_result"],
             "inventory": captured.receipt_file,
             "raw_inventories": [binding(output / f"databases/{phase}-target-{key}.json")
                                 for phase in ("before", "after") for key in KEYS],
             "object_evidence": object_evidence,
             "owner_evidence": [binding(output / item["file"]) for item in owners + repeated],
             "logical_inventories": [binding(output / f"inventory-{phase}.json") for phase in ("before", "after")],
             "schema_evidence": [binding(output / f"schema-{phase}-{database['key']}.json")
                                 for phase in ("before", "after") for database in selected["databases"]],
             "image": {"databases": inventories, "schema": schema, "objects": object_images,
                       "owners": owner_image(owners), "redis": redis, "storage": storage}}
    value["image_sha256"] = plan_hash(value["image"])
    value["scale"] = observed_scale(value["image"])
    write_json(output / "image.json", value)
    return binding(output / "image.json")


def verify_image(backend: Path, descriptor: dict, selected: dict, predecessor: dict, *, source_registration: dict) -> dict:
    path = bound_file(backend, descriptor)
    value = read_json(path)
    exact(value, {"format_version", "kind", "inventory", "raw_inventories", "object_evidence", "owner_evidence",
                  "logical_inventories", "schema_evidence", "image", "image_sha256", "scale", "source_registration"})
    exact(value["image"], {"databases", "schema", "objects", "owners", "redis", "storage"})
    exact(value["image"]["databases"], set(KEYS))
    exact(value["image"]["objects"], set(BUCKETS))
    if (type(value["format_version"]) is not int or value["format_version"] != 1 or value["kind"] != "seed-source-generation-image"
            or value["image_sha256"] != plan_hash(value["image"]) or value["scale"] != observed_scale(value["image"])
            or value["source_registration"] != source_registration):
        raise ValueError("source-generation 完整像类型或摘要不同")
    bound_file(backend, source_registration)
    runtime = read_json(local_path(backend, str(Path(selected["runtime_dir"]) / "runtime.json")))
    execution = Path(runtime["backend_root"])
    models = schema_models(execution)
    verify_schema({"databases": value["image"]["schema"]}, {"source": selected}, models)
    expected_schema = []
    for phase in ("before", "after"):
        for item in value["image"]["schema"]:
            schema_path = path.parent / f"schema-{phase}-{item['key']}.json"
            if read_json(schema_path) != {key: item[key] for key in ("key", "columns")}:
                raise ValueError("source-generation schema 与完整原始像不同")
            expected_schema.append(binding(schema_path))
    if value["schema_evidence"] != expected_schema:
        raise ValueError("source-generation schema 原始证据集合不同")
    if bound_file(backend, value["inventory"]) != path.parent / "databases/inventory.json":
        raise ValueError("source-generation 完整库存必须属于同一像目录")
    verified = verify_side_inventory(backend, value["inventory"], selected, execution)
    inventory = verified["receipt"]
    expected_raw = [binding(path.parent / f"databases/{phase}-target-{key}.json") for phase in ("before", "after") for key in KEYS]
    if (value["raw_inventories"] != expected_raw
            or inventory["inventories"] != {Path(row["path"]).name: {key: row[key] for key in ("bytes", "sha256")} for row in expected_raw}
            or value["image"]["databases"] != {key: read_json(path.parent / f"databases/after-target-{key}.json") for key in KEYS}
            or any(read_json(path.parent / f"databases/before-target-{key}.json") != value["image"]["databases"][key] for key in KEYS)):
        raise ValueError("source-generation 完整数据库像与八份原始库存绑定不同")
    expected_owners = []
    for phase in ("before", "after"):
        owners = [owner_evidence(path.parent, f"owners-{phase}", bucket, selected["scope_id"]) for bucket in sorted(BUCKETS)]
        if value["image"]["owners"] != [{key: item[key] for key in ("bucket", "key", "bytes", "sha256")} for item in owners]:
            raise ValueError("source-generation 必须绑定同一 scope 的完整五桶 ownership")
        for owner in owners:
            current = binding(path.parent / f"owners-{phase}-{owner['bucket']}.bin")
            if any(current[key] != owner[key] for key in ("bytes", "sha256")):
                raise ValueError("source-generation ownership 字节与完整像不同")
            expected_owners.append(current)
    if value["owner_evidence"] != expected_owners:
        raise ValueError("source-generation ownership 原始证据集合不同")
    tools = ExternalTools({"source": selected, "tools": predecessor["tools"]}, path.parent)
    observed = {bucket: {} for bucket in value["image"]["objects"]}
    for item in value["object_evidence"]:
        capture_path = bound_file(backend, item)
        raw = read_json(capture_path)
        bucket, key = raw["bucket"], raw["key"]
        token = hashlib.sha256((bucket + ":" + key).encode()).hexdigest()
        if capture_path != path.parent / "objects" / token / "capture.json":
            raise ValueError("source-generation 对象证据必须属于同一像与明确对象目录")
        expected = value["image"]["objects"][bucket][key]
        capture = verify_capture(tools, "source", bucket, key, capture_path.parent,
                                 expected={field: expected[field] for field in ("bytes", "sha256")},
                                 max_bytes=predecessor["max_object_bytes"])["capture"]
        if key in observed[bucket]:
            raise ValueError("source-generation 对象证据重复")
        observed[bucket][key] = {"bytes": capture["artifact"]["bytes"], "sha256": capture["artifact"]["sha256"],
                                 "metadata": capture["metadata"], "identity": capture["identity"]}
    if observed != value["image"]["objects"]:
        raise ValueError("source-generation 对象完整字节或存储元数据与原始采集不符")
    expected_logical = [binding(path.parent / f"inventory-{phase}.json") for phase in ("before", "after")]
    if value["logical_inventories"] != expected_logical:
        raise ValueError("运行中完整像缺少同目录两次观察库存")
    logical = [read_json(bound_file(backend, item)) for item in expected_logical]
    for current in logical:
        validate_inventory(current, {**predecessor, "source": selected}, models,
                           verified["binding"]["source"]["snapshot"]["head"], current.get("observed_at", ""), observed=True)
        for db in current["databases"]:
            raw = verified["databases"][db["key"]]
            rows = {item["table"]: item for item in raw["target"]["database"]["tables"] + raw["target"]["preserved_tables"]}
            if any(table != rows[table["table"]] for table in db["tables"]) or db["placements"] != raw["target"]["database"]["placements"]:
                raise ValueError("观察库存业务表或 placement 与完整原始像不同")
        objects = {item["bucket"]: {row["key"]: {key: row[key] for key in ("bytes", "sha256")} for row in item["entries"]}
                   for item in current["objects"]}
        if objects != {bucket: {key: {field: row[field] for field in ("bytes", "sha256")} for key, row in values.items()}
                       for bucket, values in value["image"]["objects"].items()}:
            raise ValueError("观察库存完整对象范围或字节摘要与采集像不同")
    if logical_inventory(logical[0]) != logical_inventory(logical[1]):
        raise ValueError("两次观察库存不同，不能证明只读稳定")
    return value


def observed_scale(image: dict) -> dict:
    databases = [value["target"]["database"] for value in image["databases"].values()]
    return {"tenants": len({item["tenant_id"] for database in databases for item in database["placements"]}),
            "post_rows": sum(item["rows"] for database in databases for item in database["tables"] if item["table"] == "sys_post"),
            "objects": sum(len(values) for values in image["objects"].values()),
            "object_bytes": sum(item["bytes"] for values in image["objects"].values() for item in values.values())}
