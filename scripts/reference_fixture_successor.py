"""把历史 pending seed 计划与新的三侧 ready 计划绑定为不可变 successor 证据。"""

from __future__ import annotations

import argparse
import copy
import json
from pathlib import Path
from urllib.parse import urlsplit

from devex_clone_capture import read_bound_json, write_json
from devex_clone_model import digest, exact, linked, local_path, name
from devex_clone_run_state import binding
from devex_clone_seed_source import _published_source as _deep_published_source
from devex_clone_target_binding import (
    pending_request_binding,
    request_binding,
    validate_review,
)
from process_sockets import endpoint
from reference_fixture_environment import validate_preflight_successor
from restore_reference_plan import BUCKETS, plan_hash


SIDES = ("seed", "base", "candidate")
FIELDS = {
    "format_version",
    "kind",
    "id",
    "source_result",
    "source_registration",
    "predecessor_review",
    "predecessor_request",
    "successor_review",
    "requests",
    "collision_projection",
    "relationship_sha256",
    "remote_writes",
    "restore_qualified",
}


def _document(
    backend: Path, value: Path, expected: dict | None = None
) -> tuple[Path, dict, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("successor 输入必须是受控普通文件")
    descriptor = binding(path)
    if expected is not None and descriptor != expected:
        raise ValueError("successor 输入绑定已变化")
    return path, read_bound_json(path, descriptor), descriptor


def _review_descriptor(path: Path, value: dict, descriptor: dict) -> dict:
    return {**descriptor, "canonical_sha256": plan_hash(value)}


def _pending_review(backend: Path, path: Path) -> tuple[dict, dict]:
    filename, review, descriptor = _document(backend, path)
    if review.get("ready_for_execution") is not False:
        raise ValueError("predecessor review 必须保持 pending")
    structural = copy.deepcopy(review)
    structural["ready_for_execution"] = True
    validate_review(structural)
    return review, _review_descriptor(filename, review, descriptor)


def _ready_review(
    backend: Path, path: Path, expected_predecessor: dict
) -> tuple[dict, dict]:
    filename, review, descriptor = _document(backend, path)
    preflight = review.get("preflight")
    predecessor = preflight.get("supersedes") if isinstance(preflight, dict) else None
    if not isinstance(predecessor, dict):
        raise ValueError("successor review 缺少 preflight predecessor")
    if predecessor != expected_predecessor:
        raise ValueError("successor review 未 supersede 指定 pending predecessor")
    prior_path, prior, _ = _document(
        backend, Path(predecessor.get("path", "")), predecessor
    )
    if prior.get("ready_for_execution") is not False:
        raise ValueError("successor preflight predecessor 必须保持 pending")
    try:
        validate_preflight_successor(prior, review)
    except (KeyError, TypeError, ValueError) as error:
        raise ValueError("successor review 与其 preflight predecessor 语义不同") from error
    if binding(prior_path) != predecessor:
        raise ValueError("successor preflight predecessor 在核对期间变化")
    return review, _review_descriptor(filename, review, descriptor)


def _socket(value: str, *, redis: bool = False) -> str:
    if redis:
        parsed = urlsplit(value if "://" in value else "redis://" + value)
        if (
            parsed.scheme != "redis"
            or parsed.hostname not in ("127.0.0.1", "::1")
            or parsed.port is None
        ):
            raise ValueError("successor Redis 端点必须是明确 loopback 端口")
        return f"{parsed.hostname}:{parsed.port}"
    _, host, port = endpoint(value)
    return f"{host}:{port}"


def _absolute(value: str) -> str:
    path = Path(value)
    if not path.is_absolute():
        raise ValueError("successor 碰撞路径必须是绝对路径")
    return str(path)


def _projection(review: dict, label: str) -> dict:
    result = {
        key: []
        for key in ("scopes", "databases", "redis", "objects", "paths", "sockets")
    }
    for side in SIDES:
        selected = review["scopes"][side]
        owner = f"{label}.{side}"
        scope = selected["scope_id"]
        result["scopes"].append({"owner": owner, "identity": scope})
        for database in selected["databases"]:
            result["databases"].append(
                {
                    "owner": owner,
                    "key": database["key"],
                    "identity": f"{database['expected_server_uuid']}:{database['database'].lower()}",
                }
            )
        cache = selected["redis"]
        expected_owner = f"ryframe-owner:v1:{scope}:redis"
        if (
            cache["namespace"] != f"ryframe:{{{scope}}}:"
            or cache["ownership_key"] != cache["namespace"] + ".ryframe-owner"
            or cache["ownership_value"] != expected_owner
        ):
            raise ValueError("successor review Redis ownership 投影无效")
        result["redis"].append({"owner": owner, "identity": cache["ownership_key"]})
        for bucket in sorted(BUCKETS):
            result["objects"].append(
                {
                    "owner": owner,
                    "bucket": bucket,
                    "identity": f"{selected['objects']['endpoint']}|{scope}/",
                }
            )
        result["paths"].extend(
            {"owner": owner, "identity": _absolute(selected[field])}
            for field in ("runtime_dir", "identity_ledger")
        )
        result["sockets"].extend(
            {"owner": f"{owner}.{field}", "identity": _socket(selected[field])}
            for field in ("api_url", "worker_ready_url", "frontend_url")
        )
    services = review["services"]
    result["paths"].extend(
        (
            {"owner": f"{label}.future", "identity": _absolute(review["future_root"])},
            {
                "owner": f"{label}.rustfs",
                "identity": _absolute(services["rustfs"]["data_dir"]),
            },
            {
                "owner": f"{label}.redis",
                "identity": _absolute(services["redis"]["directory"]),
            },
        )
    )
    result["sockets"].extend(
        (
            {
                "owner": f"{label}.rustfs-api",
                "identity": _socket(services["rustfs"]["api"]),
            },
            {
                "owner": f"{label}.rustfs-console",
                "identity": _socket(services["rustfs"]["console"]),
            },
            {
                "owner": f"{label}.redis",
                "identity": _socket(services["redis"]["endpoint"], redis=True),
            },
        )
    )
    return {
        key: sorted(
            values,
            key=lambda item: (
                item["identity"],
                item["owner"],
                item.get("key", ""),
                item.get("bucket", ""),
            ),
        )
        for key, values in result.items()
    }


def collision_projection(predecessor: dict, successor: dict) -> dict:
    projection = {
        "predecessor": _projection(predecessor, "predecessor"),
        "successor": _projection(successor, "successor"),
    }
    for field in ("scopes", "databases", "redis", "objects", "sockets"):
        before = {item["identity"] for item in projection["predecessor"][field]}
        after = {item["identity"] for item in projection["successor"][field]}
        if before & after:
            raise ValueError(f"successor {field} 与历史计划发生碰撞")
    for before in projection["predecessor"]["paths"]:
        old = Path(before["identity"])
        for after in projection["successor"]["paths"]:
            new = Path(after["identity"])
            if old == new or old.is_relative_to(new) or new.is_relative_to(old):
                raise ValueError("successor 路径与历史计划重叠")
    return projection


def relationship_hash(value: dict) -> str:
    exact(value, FIELDS)
    return plan_hash(
        {key: value[key] for key in sorted(FIELDS - {"relationship_sha256"})}
    )


def _descriptor_path(value: dict, label: str, *, canonical: bool = False) -> Path:
    fields = {"path", "bytes", "sha256"}
    if canonical:
        fields.add("canonical_sha256")
    if not isinstance(value, dict):
        raise ValueError(f"successor {label} 绑定结构无效")
    exact(value, fields)
    if not isinstance(value["path"], str):
        raise ValueError(f"successor {label} 路径无效")
    digest(value["sha256"])
    if canonical:
        digest(value["canonical_sha256"])
    return Path(value["path"])


def _validated_relationship(backend: Path, descriptor: dict) -> dict:
    path = _descriptor_path(descriptor, "证据")
    filename, value, _ = _document(backend, path, descriptor)
    exact(value, FIELDS)
    if (
        value["format_version"] != 1
        or value["kind"] != "devex-clone-seed-review-successor"
        or name(value["id"]) != value["id"]
        or value["remote_writes"] != 0
        or value["restore_qualified"] is not False
        or relationship_hash(value) != value["relationship_sha256"]
    ):
        raise ValueError("seed review successor 证据结构或关系摘要无效")
    requests = value["requests"]
    if not isinstance(requests, dict) or set(requests) != set(SIDES):
        raise ValueError("seed review successor 缺少三侧请求绑定")
    rebuilt = build(
        backend,
        _descriptor_path(value["source_result"], "source result"),
        _descriptor_path(
            value["predecessor_review"], "predecessor review", canonical=True
        ),
        _descriptor_path(
            value["predecessor_request"], "predecessor request", canonical=True
        ),
        _descriptor_path(value["successor_review"], "successor review", canonical=True),
        {
            side: _descriptor_path(requests[side], f"{side} request", canonical=True)
            for side in SIDES
        },
        value["id"],
    )
    if rebuilt != value or binding(filename) != descriptor:
        raise ValueError("seed review successor 与当前绑定输入不一致")
    return value


def _source_with_loader(
    backend: Path, descriptor: dict, *, live_storage: bool, loader
) -> dict:
    """通过 successor 特例恢复历史 pending seed，不改变普通发布源的就绪规则。"""
    backend = backend.resolve(strict=True)
    expected_descriptor = copy.deepcopy(descriptor)
    successor = _validated_relationship(backend, expected_descriptor)
    expected = successor["predecessor_request"]
    expected_binding = {key: expected[key] for key in ("path", "bytes", "sha256")}
    _, historical_request, _ = _document(
        backend,
        _descriptor_path(expected, "predecessor request", canonical=True),
        expected_binding,
    )
    if plan_hash(historical_request) != expected["canonical_sha256"]:
        raise ValueError("successor 历史 seed 请求规范摘要无效")
    validation_count = 0

    def validate_pending(root: Path, seed_target: dict) -> tuple[dict, dict]:
        nonlocal validation_count
        validation_count += 1
        if seed_target != historical_request:
            raise ValueError("发布源初始化历史不是 successor 绑定的 seed 请求")
        return pending_request_binding(
            root, seed_target, successor["predecessor_review"]
        )

    source = loader(
        backend,
        successor["source_result"],
        live_storage=live_storage,
        validate_seed_target=validate_pending,
    )
    if (
        validation_count != 1
        or source["result"].get("registration") != successor["source_registration"]
        or source["seed_target"] != historical_request
    ):
        raise ValueError("发布源与 seed review successor 关系不一致")
    if source.get("source_rebind") is not None:
        _, rebind, _ = _document(backend, Path(source["source_rebind"]["path"]), source["source_rebind"])
        if rebind.get("review_successor") != expected_descriptor:
            raise ValueError("存储重绑定不属于当前 successor 关系")
    if (
        descriptor != expected_descriptor
        or _validated_relationship(backend, expected_descriptor) != successor
    ):
        raise ValueError("seed review successor 或其输入在核对期间变化")
    return {
        **source,
        "review_successor": successor,
        "review_successor_binding": expected_descriptor,
    }


def published_source(backend: Path, descriptor: dict, *, live_storage: bool = False) -> dict:
    return _source_with_loader(backend, descriptor, live_storage=live_storage,
                               loader=_deep_published_source)


def build(
    backend: Path,
    source_result: Path,
    predecessor_review: Path,
    predecessor_request: Path,
    successor_review: Path,
    requests: dict[str, Path],
    successor_id: str,
) -> dict:
    """构造纯本地 successor 关系；C52 深层 published-source 核验由后续专用消费者完成。"""
    backend = backend.resolve(strict=True)
    name(successor_id)
    if set(requests) != set(SIDES):
        raise ValueError("successor 必须绑定 seed/base/candidate 三份请求")
    _, source, source_result_binding = _document(backend, source_result)
    if (
        source.get("status") != "seed_source_registered"
        or source.get("remote_writes") != 0
        or source.get("outbox_drained") is not True
        or source.get("restore_qualified") is not False
    ):
        raise ValueError("successor 来源外层结果尚未声明已排空的 seed registration")
    source_registration = source.get("registration")
    if not isinstance(source_registration, dict):
        raise ValueError("successor 来源外层结果缺少 inner registration 绑定")
    _, registration, registration_binding = _document(
        backend, Path(source_registration.get("path", "")), source_registration
    )
    if registration.get("kind") != "devex-clone-seed-source-registration":
        raise ValueError("successor 来源 inner registration 类型无效")
    predecessor, predecessor_binding = _pending_review(backend, predecessor_review)
    predecessor_path, historical_request, historical_binding = _document(
        backend, predecessor_request
    )
    pending_request_binding(backend, historical_request, predecessor_binding)
    if historical_request["side"] != "seed":
        raise ValueError("successor predecessor 请求必须是历史 seed 侧")
    predecessor_request_binding = {
        **historical_binding,
        "canonical_sha256": plan_hash(historical_request),
    }
    if binding(predecessor_path) != historical_binding:
        raise ValueError("successor predecessor 请求在核对期间变化")
    successor, successor_binding = _ready_review(
        backend, successor_review, predecessor_binding
    )
    request_bindings = {}
    storage_generations = set()
    request_ids = set()
    for side in SIDES:
        path, request, descriptor = _document(backend, requests[side])
        request_binding(backend, request)
        if request["side"] != side or request["review"] != successor_binding:
            raise ValueError("successor 三侧请求未绑定 ready review 的对应侧")
        request_bindings[side] = {**descriptor, "canonical_sha256": plan_hash(request)}
        storage_generations.add(plan_hash(request["storage"]))
        request_ids.add(request["id"])
        if binding(path) != descriptor:
            raise ValueError("successor 请求在核对期间变化")
    if len(storage_generations) != 1 or len(request_ids) != len(SIDES):
        raise ValueError("successor 三侧请求必须共享 seed 服务代次且使用不同请求 ID")
    result = {
        "format_version": 1,
        "kind": "devex-clone-seed-review-successor",
        "id": successor_id,
        "source_result": source_result_binding,
        "source_registration": registration_binding,
        "predecessor_review": predecessor_binding,
        "predecessor_request": predecessor_request_binding,
        "successor_review": successor_binding,
        "requests": request_bindings,
        "collision_projection": collision_projection(predecessor, successor),
        "remote_writes": 0,
        "restore_qualified": False,
    }
    result["relationship_sha256"] = plan_hash(result)
    return result


def publish(
    backend: Path,
    source_result: Path,
    predecessor_review: Path,
    predecessor_request: Path,
    successor_review: Path,
    requests: dict[str, Path],
    successor_id: str,
    output: Path,
) -> dict:
    backend = backend.resolve(strict=True)
    requested = output if output.is_absolute() else backend / output
    target = local_path(backend, str(requested), new=True)
    if not target.parent.is_dir():
        raise ValueError("successor 输出父目录不存在")
    result = build(
        backend,
        source_result,
        predecessor_review,
        predecessor_request,
        successor_review,
        requests,
        successor_id,
    )
    write_json(target, result)
    if (
        read_bound_json(target, binding(target)) != result
        or relationship_hash(result) != result["relationship_sha256"]
    ):
        raise ValueError("successor 证据发布后无法重算")
    if (
        build(
            backend,
            source_result,
            predecessor_review,
            predecessor_request,
            successor_review,
            requests,
            successor_id,
        )
        != result
    ):
        raise ValueError("successor 输入在发布期间变化")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    operations = parser.add_subparsers(dest="operation", required=True)
    relationship = operations.add_parser("relationship")
    relationship.add_argument("--backend-dir", type=Path, required=True)
    relationship.add_argument("--source-result", type=Path, required=True)
    relationship.add_argument("--predecessor-review", type=Path, required=True)
    relationship.add_argument("--predecessor-request", type=Path, required=True)
    relationship.add_argument("--successor-review", type=Path, required=True)
    for side in SIDES:
        relationship.add_argument(f"--{side}-request", type=Path, required=True)
    relationship.add_argument("--id", required=True)
    relationship.add_argument("--output", type=Path, required=True)
    relationship.add_argument("--write", action="store_true")
    from reference_fixture_successor_generation import arguments as generation_arguments

    generation_arguments(operations.add_parser("generation-request", allow_abbrev=False))
    arm = operations.add_parser("arm-request")
    arm.add_argument("--backend-dir", type=Path, required=True)
    arm.add_argument("--successor", type=Path, required=True)
    arm.add_argument("--source-export-result", type=Path, required=True)
    arm.add_argument("--workspace", type=Path, required=True)
    arm.add_argument("--id", required=True)
    arm.add_argument("--side", choices=("base", "candidate"), required=True)
    arm.add_argument("--copy-directory", type=Path, required=True)
    arm.add_argument("--output", type=Path)
    arm.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.operation == "generation-request":
        from reference_fixture_successor_generation import execute

        print(json.dumps(execute(args, parser), ensure_ascii=False))
        return
    if args.operation == "arm-request":
        from reference_fixture_successor_arm import build as build_arm
        from reference_fixture_successor_arm import publish as publish_arm
        from reference_fixture_successor_arm import summary

        if args.write != (args.output is not None):
            parser.error("arm-request 写入必须同时指定 --output 与 --write；默认只读不接受 --output")
        action = publish_arm if args.write else build_arm
        values = (
            args.backend_dir,
            args.successor,
            args.workspace,
            args.id,
            args.side,
            args.copy_directory,
        )
        keywords = {"source_export_result_path": args.source_export_result}
        request = action(*values, args.output, **keywords) if args.write else action(*values, **keywords)
        print(json.dumps(summary(request, side=args.side, written=args.write), ensure_ascii=False))
        return
    if not args.write:
        parser.error("签发 seed review successor 必须显式 --write")
    requests = {side: getattr(args, side + "_request") for side in SIDES}
    result = publish(
        args.backend_dir,
        args.source_result,
        args.predecessor_review,
        args.predecessor_request,
        args.successor_review,
        requests,
        args.id,
        args.output,
    )
    print(
        json.dumps(
            {
                "status": "seed_review_successor_published",
                "relationship_sha256": result["relationship_sha256"],
                "remote_writes": 0,
            },
            ensure_ascii=False,
        )
    )


if __name__ == "__main__":
    main()
