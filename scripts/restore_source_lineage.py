"""把 C52 的原始参考数据证明投影到当前 seed 来源；不读取或写入业务资源。"""

from __future__ import annotations

from pathlib import Path
import re

from devex_clone_capture import read_json
from devex_clone_model import exact
from devex_clone_run_state import binding, load_state
from devex_clone_source_proof import bound_file
from restore_reference_plan import plan_hash, validate_plan


LINEAGE_FIELDS = {
    "format_version", "kind", "status", "source_registration", "seed_registration",
    "post_copy", "post_verify", "post_verify_evidence", "post_verify_target",
    "reference_plan", "dataset", "copy_stage_receipt", "copy_result", "ledger_head",
    "copy_plan", "current_image", "scopes", "scale", "tenants", "objects",
    "restore_qualified",
}
DATASET_FIELDS = {
    "format_version", "plan_sha256", "source_scope_id", "started_at", "records",
    "object_bytes", "tenants", "request_interval_ms", "post_concurrency", "completed_at",
}
TENANT_FIELDS = {"tenant_id", "username", "password_env", "records", "posts", "files"}
POST_FIELDS = {"id", "code", "name"}
FILE_FIELDS = {"file_id", "file_name", "file_path", "file_url", "bytes", "sha256"}
POST_RESULT_FIELDS = {
    "status", "evidence", "objects", "registration", "restore_qualified",
    "worker_must_remain_stopped",
}
BUSINESS_FIELDS = {
    "actions", "clone_verified", "copy", "files", "format_version", "input_files",
    "plan_sha256", "posts", "restore_success", "scope_id", "side", "source_scope_id",
    "status", "target_binding_sha256", "tenants",
}
BUSINESS_TARGET_FIELDS = {
    "copy", "format_version", "kind", "source_plan_sha256", "source_scope_id", "target",
}


def _descriptor(backend: Path, value: dict, label: str) -> tuple[Path, dict]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} 缺少文件绑定")
    path = bound_file(backend, value)
    return path, read_json(path)


def _source_chain(backend: Path, source: dict) -> tuple[dict, object]:
    from devex_clone_post_registration import resolve_post_registration
    from devex_clone_seed_source import REGISTRATION_FIELDS, RESULT_FIELDS

    successor = source.get("review_successor")
    descriptor = successor.get("source_result") if isinstance(successor, dict) else None
    _, result = _descriptor(backend, descriptor, "C52 外层结果")
    exact(result, RESULT_FIELDS)
    registration_descriptor = result.get("registration")
    _, registration = _descriptor(backend, registration_descriptor, "C52 内层登记")
    exact(registration, REGISTRATION_FIELDS)
    if (
        result != source.get("result")
        or registration != source.get("registration")
        or result.get("status") != "seed_source_registered"
        or result.get("remote_writes") != 0
        or result.get("outbox_drained") is not True
        or result.get("restore_qualified") is not False
        or registration.get("remote_writes") != 0
        or registration.get("outbox_drained") is not True
        or registration.get("restore_qualified") is not False
        or registration.get("source_request") != result.get("source_request")
    ):
        raise ValueError("C52 外层结果、内层登记或已验证来源不同")
    directory = source.get("directory")
    if not isinstance(directory, Path):
        raise ValueError("C52 来源缺少已验证 run 目录")
    post = resolve_post_registration(
        backend, directory, descriptor=registration["post_copy"]
    )
    return {"outer": descriptor, "registration": registration_descriptor, "value": registration}, post


def _dataset_facts(backend: Path, post) -> dict:
    plan_path, plan = _descriptor(backend, post.request["reference_plan"], "原参考计划")
    dataset_path, dataset = _descriptor(backend, post.request["dataset"], "原数据集")
    validate_plan(plan, backend)
    exact(dataset, DATASET_FIELDS)
    origin = plan.get("source", {}).get("scope_id")
    if not isinstance(origin, str):
        raise ValueError("C52 原参考计划缺少租户 scope")
    expected_ids = {"system", *{f"{origin}-{index:02d}" for index in range(1, 11)}}
    if (
        dataset.get("format_version") != 1
        or dataset.get("plan_sha256") != plan_hash(plan)
        or dataset.get("source_scope_id") != origin
        or type(dataset.get("records")) is not int
        or dataset["records"] < 100_000
        or type(dataset.get("object_bytes")) is not int
        or dataset["object_bytes"] < 1024**3
        or not isinstance(dataset.get("tenants"), list)
        or len(dataset["tenants"]) != 11
    ):
        raise ValueError("C52 原数据集没有绑定原计划或未达到正式恢复规模")
    tenants, files, seen_files = [], [], set()
    for tenant in dataset["tenants"]:
        exact(tenant, TENANT_FIELDS)
        if (
            not isinstance(tenant["tenant_id"], str)
            or type(tenant["records"]) is not int
            or tenant["records"] < 0
            or not isinstance(tenant["posts"], list)
            or not isinstance(tenant["files"], list)
            or len(tenant["posts"]) < 3
            or not tenant["files"]
        ):
            raise ValueError("C52 原数据集租户记录或样本无效")
        for item in tenant["posts"]:
            exact(item, POST_FIELDS)
        for item in tenant["files"]:
            exact(item, FILE_FIELDS)
            identity = (tenant["tenant_id"], item["file_path"])
            if (
                identity in seen_files
                or not isinstance(item["file_path"], str)
                or not item["file_path"].startswith(tenant["tenant_id"] + "/")
                or type(item["bytes"]) is not int
                or item["bytes"] <= 0
                or not isinstance(item["sha256"], str)
                or re.fullmatch(r"[a-f0-9]{64}", item["sha256"]) is None
            ):
                raise ValueError("C52 原数据集对象样本重复或摘要无效")
            seen_files.add(identity)
            files.append((tenant["tenant_id"], item))
        tenants.append(tenant)
    if (
        {item["tenant_id"] for item in tenants} != expected_ids
        or sum(item["records"] for item in tenants) != dataset["records"]
        or sum(item["bytes"] for _, item in files) != dataset["object_bytes"]
    ):
        raise ValueError("C52 原数据集租户集合、记录总数或对象字节不一致")
    return {
        "plan_path": plan_path,
        "plan": plan,
        "dataset_path": dataset_path,
        "dataset": dataset,
        "tenants": tenants,
        "files": files,
    }


def _copy_facts(backend: Path, source: dict, post, dataset: dict) -> dict:
    from devex_clone_post import _validate_business_lineage
    from devex_clone_run import require_target_copy

    verified = require_target_copy(backend, source["directory"], source["manifest"], ("api",))
    plan = verified.plan
    _validate_business_lineage(backend, source["manifest"], post.request, plan)
    result_path, result = _descriptor(backend, post.request["copy_result"], "C18 复制结果")
    _descriptor(backend, post.request["copy_stage_receipt"], "C18 外层收据")
    _descriptor(backend, post.request["ledger_head"], "C18 账本头")
    plan_path = result_path.with_name("plan.json")
    bound_file(backend, binding(plan_path))
    expected = {
        "fresh_target_sha256": result["fresh_target_sha256"],
        "generation_sha256": result["generation_sha256"],
        "ledger_head_sha256": post.request["ledger_head"]["sha256"],
        "plan_sha256": plan["plan_sha256"],
        "source_export_sha256": result["source_export_sha256"],
        "stage_receipt_sha256": post.request["copy_stage_receipt"]["sha256"],
    }
    if (
        result.get("status") != "data_steps_verified"
        or result.get("plan_sha256") != plan.get("plan_sha256")
        or plan.get("copy_stage") != "source_to_seed"
        or plan.get("source_scope") != dataset["plan"]["source"]["scope_id"]
        or plan.get("target_scope") != source["request"]["source"]["scope_id"]
    ):
        raise ValueError("C52 复制计划、账本或来源/目标 scope 不一致")
    return {"plan": plan, "plan_path": plan_path, "result": result, "copy": expected}


def _post_verify(backend: Path, source: dict, chain: dict, post, dataset: dict, copy: dict) -> dict:
    descriptor = chain["value"]["post_verify"]
    path, result = _descriptor(backend, descriptor, "C29 外层结果")
    exact(result, POST_RESULT_FIELDS)
    attempts = [item for item in load_state(source["directory"])["attempts"]
                if item["stage"] == "post-copy" and item["mode"] == "verify"]
    matched = [item for item in attempts if item["status"] == "passed" and item["result"] == descriptor]
    if len(matched) != 1 or path != source["directory"] / "results" / f"{matched[0]['number']:04d}.json":
        raise ValueError("C29 不是同一 copy-run 已发布的唯一业务复验")
    evidence_path, evidence = _descriptor(backend, result["evidence"], "C29 业务复验")
    target_path = evidence_path.with_name("business-target.json")
    target_descriptor = binding(target_path)
    target = read_json(bound_file(backend, target_descriptor))
    exact(evidence, BUSINESS_FIELDS)
    exact(target, BUSINESS_TARGET_FIELDS)
    expected_actions = {"business": "read_only", "objects": "read_only", "session": "login_logout"}
    expected_inputs = {
        "plan_sha256": post.request["reference_plan"]["sha256"],
        "dataset_sha256": post.request["dataset"]["sha256"],
        "target_binding_sha256": target_descriptor["sha256"],
    }
    if (
        result.get("status") != "post_copy_existing_data_verified"
        or result.get("registration") != post.descriptor
        or result.get("worker_must_remain_stopped") is not True
        or result.get("restore_qualified") is not False
        or result.get("objects") != {
            "business_objects": len(copy["plan"]["objects"]),
            "additional_objects": 1,
            "additional_objects_downloaded": True,
            "all_scoped_keys_verified": True,
        }
        or evidence.get("format_version") != 1
        or evidence.get("status") != "copy_existing_data_verified"
        or evidence.get("side") != "copy_target"
        or evidence.get("clone_verified") is not False
        or evidence.get("restore_success") is not False
        or evidence.get("actions") != expected_actions
        or evidence.get("scope_id") != source["request"]["source"]["scope_id"]
        or evidence.get("source_scope_id") != dataset["dataset"]["source_scope_id"]
        or evidence.get("plan_sha256") != dataset["dataset"]["plan_sha256"]
        or evidence.get("input_files") != expected_inputs
        or evidence.get("copy") != copy["copy"]
        or evidence.get("target_binding_sha256") != plan_hash(target)
        or evidence.get("tenants") != len(dataset["tenants"])
        or evidence.get("posts") != sum(len(item["posts"]) for item in dataset["tenants"])
        or evidence.get("files") != len(dataset["files"])
        or target.get("format_version") != 1
        or target.get("kind") != "devex-copy-business-target"
        or target.get("copy") != copy["copy"]
        or target.get("source_plan_sha256") != dataset["dataset"]["plan_sha256"]
        or target.get("source_scope_id") != dataset["dataset"]["source_scope_id"]
        or target.get("target") != {
            "scope_id": source["request"]["source"]["scope_id"],
            "api_url": source["request"]["source"]["api_url"],
            "frontend_url": source["request"]["source"]["frontend_url"],
        }
    ):
        raise ValueError("C29 没有精确绑定原数据样本、C18 复制账本与当前 seed scope")
    return {"result": descriptor, "evidence": result["evidence"], "target": binding(target_path)}


def _tenant_projection(plan: dict, dataset: dict, image: dict) -> list[dict]:
    targets = plan.get("dataset", {}).get("tenant_targets")
    origin = dataset["dataset"]["source_scope_id"]
    tenant_by_id = {item["tenant_id"]: item for item in dataset["tenants"]}
    ordinary = [tenant_by_id[f"{origin}-{index:02d}"] for index in range(1, 11)]
    if (
        not isinstance(targets, list)
        or len(targets) != 10
        or len(ordinary) != 10
        or not any(item.startswith("dedicated-") for item in targets)
        or not any(item in {"shared", "shared-control"} for item in targets)
    ):
        raise ValueError("原计划没有同时覆盖共享与独立的十个普通租户")
    expected = {"system": "shared-control"}
    expected.update({tenant["tenant_id"]: target for tenant, target in zip(ordinary, targets, strict=True)})
    databases = image.get("image", {}).get("databases")
    if not isinstance(databases, dict):
        raise ValueError("当前完整像缺少数据库完整库存")
    observed = {}
    for key, value in databases.items():
        database = value.get("target", {}).get("database", {})
        if database.get("key") != key:
            raise ValueError("当前完整像的数据库逻辑键不一致")
        for placement in database.get("placements", []):
            tenant_id = placement.get("tenant_id")
            if tenant_id in observed:
                raise ValueError("当前完整像重复登记租户 placement")
            observed[tenant_id] = key
    if observed != expected:
        raise ValueError("当前完整像没有保留原十一租户的共享/独立 placement")
    return [{"tenant_id": tenant_id, "database": expected[tenant_id]}
            for tenant_id in ["system", *[item["tenant_id"] for item in ordinary]]]


def _object_projection(dataset: dict, copy: dict, image: dict) -> dict:
    origin = dataset["dataset"]["source_scope_id"]
    current = copy["plan"]["target_scope"]
    declared = {(tenant_id, item["file_path"]): item for tenant_id, item in dataset["files"]}
    objects = image.get("image", {}).get("objects")
    if not isinstance(objects, dict):
        raise ValueError("当前完整像缺少对象完整库存")
    mappings = []
    for item in copy["plan"]["objects"]:
        exact(item, {"bucket", "source_key", "target_key", "artifact", "metadata"})
        prefix = origin + "/"
        if not item["source_key"].startswith(prefix):
            raise ValueError("复制计划业务对象没有使用原数据 scope")
        relative = item["source_key"][len(prefix):]
        tenant_id = relative.split("/", 1)[0]
        sample = declared.pop((tenant_id, relative), None)
        observed = objects.get(item["bucket"], {}).get(item["target_key"])
        expected_key = current + "/" + relative
        if (
            sample is None
            or item["target_key"] != expected_key
            or item["artifact"].get("bytes") != sample["bytes"]
            or item["artifact"].get("sha256") != sample["sha256"]
            or not isinstance(observed, dict)
            or observed.get("bytes") != sample["bytes"]
            or observed.get("sha256") != sample["sha256"]
            or observed.get("metadata") != item["metadata"]
        ):
            raise ValueError("当前完整像没有精确保留原 256 个业务对象及 scope 重写")
        mappings.append({
            "bucket": item["bucket"], "source_key": item["source_key"],
            "target_key": item["target_key"], "bytes": sample["bytes"],
            "sha256": sample["sha256"], "metadata": item["metadata"],
        })
    if declared or len(mappings) != 256:
        raise ValueError("原数据集与复制计划的业务对象集合不完整")
    mappings.sort(key=lambda item: (item["bucket"], item["target_key"]))
    return {"business_objects": len(mappings), "business_bytes": sum(item["bytes"] for item in mappings),
            "mapping_sha256": plan_hash(mappings)}


def derive_dataset_lineage(
    backend: Path,
    source: dict,
    image_descriptor: dict,
    verified_image: dict,
) -> dict:
    """从已验证 C52 与当前完整像重建唯一派生血缘；返回值由调用方写入代次目录。"""
    backend = backend.resolve(strict=True)
    image_path = bound_file(backend, image_descriptor)
    if read_json(image_path) != verified_image:
        raise ValueError("当前完整像与已验证值不同")
    chain, post = _source_chain(backend, source)
    if verified_image.get("source_registration") != chain["outer"]:
        raise ValueError("当前完整像没有绑定同一 C52 外层结果")
    dataset = _dataset_facts(backend, post)
    copy = _copy_facts(backend, source, post, dataset)
    verification = _post_verify(backend, source, chain, post, dataset, copy)
    tenants = _tenant_projection(dataset["plan"], dataset, verified_image)
    objects = _object_projection(dataset, copy, verified_image)
    current_posts = sum(
        item["rows"]
        for value in verified_image["image"]["databases"].values()
        for item in value["target"]["database"]["tables"]
        if item["table"] == "sys_post"
    )
    expected_posts = sum(
        value["tables"].get("sys_post", 0) for value in copy["plan"]["databases"]
    )
    if current_posts != expected_posts or current_posts < dataset["dataset"]["records"]:
        raise ValueError("当前完整像的 sys_post 总数与已验证复制计划不同")
    return {
        "format_version": 1,
        "kind": "restore-source-derived-dataset-lineage",
        "status": "derived_dataset_verified",
        "source_registration": chain["outer"],
        "seed_registration": chain["registration"],
        "post_copy": post.descriptor,
        "post_verify": verification["result"],
        "post_verify_evidence": verification["evidence"],
        "post_verify_target": verification["target"],
        "reference_plan": post.request["reference_plan"],
        "dataset": post.request["dataset"],
        "copy_stage_receipt": post.request["copy_stage_receipt"],
        "copy_result": post.request["copy_result"],
        "ledger_head": post.request["ledger_head"],
        "copy_plan": binding(copy["plan_path"]),
        "current_image": image_descriptor,
        "scopes": {
            "origin_tenant_scope_id": dataset["dataset"]["source_scope_id"],
            "current_source_scope_id": source["request"]["source"]["scope_id"],
            "current_object_scope_id": copy["plan"]["target_scope"],
        },
        "scale": {
            "records": dataset["dataset"]["records"],
            "current_post_rows": current_posts,
            "tenants": len(tenants),
            "post_samples": sum(len(item["posts"]) for item in dataset["tenants"]),
            "business_objects": objects["business_objects"],
            "object_bytes": objects["business_bytes"],
        },
        "tenants": tenants,
        "objects": objects,
        "restore_qualified": False,
    }


def verify_dataset_lineage(
    backend: Path,
    source: dict,
    lineage_descriptor: dict,
    image_descriptor: dict,
    verified_image: dict,
) -> dict:
    """只读复核派生血缘及全部祖先绑定，拒绝缓存字段、未知字段和路径替换。"""
    path, value = _descriptor(backend.resolve(strict=True), lineage_descriptor, "派生数据集血缘")
    exact(value, LINEAGE_FIELDS)
    expected = derive_dataset_lineage(backend, source, image_descriptor, verified_image)
    if value != expected or binding(path) != lineage_descriptor:
        raise ValueError("派生数据集血缘与 C52、C29、复制账本或当前完整像不同")
    return value
