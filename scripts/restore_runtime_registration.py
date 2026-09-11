"""登记 fresh target 的三端零进程状态，并在恢复写入期间保持同一 ownership 锁。"""

from __future__ import annotations

import stat
from contextlib import contextmanager
from pathlib import Path

from devex_clone_source_proof import require_closed_port
from restore_build import repository, validate_new_output, write_new
from restore_reference_plan import validate_plan
from restore_runtime_evidence import (
    HEX_40,
    HEX_64,
    artifact_snapshot,
    canonical_endpoint,
    directory,
    exact_fields,
    read_json_document,
    reject_link_or_reparse,
)
from runtime_control_lock import ControllerLockSpec, assert_controller_lock, controller_lock
from process_sockets import endpoint

REGISTRATION_LOCK = ControllerLockSpec(
    lock_name="restore-runtime-registration.lock",
    guard_name="restore-runtime-registration.guard",
    owner_kind="restore-runtime-registration-controller",
    recovery_kind="restore-runtime-registration-reconciliation",
    recovery_prefix="restore-registration-reconcile",
)
ROLES = ("api", "worker", "frontend")


def add_arguments(parser) -> None:
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument("--target-plan", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write", action="store_true", required=True)


def _descriptor(document) -> dict:
    snapshot = artifact_snapshot(document.path)
    if snapshot.sha256 != document.sha256:
        raise ValueError("恢复运行登记输入在摘要采集期间发生变化")
    return snapshot.descriptor()


def _bound_document(backend: Path, descriptor: object, label: str):
    descriptor = exact_fields(descriptor, {"path", "bytes", "sha256"}, label)
    path = Path(descriptor["path"]) if isinstance(descriptor["path"], str) else Path()
    local = (backend / ".local-tests").resolve(strict=True)
    if (
        not path.is_absolute()
        or not path.absolute().is_relative_to(local)
        or type(descriptor["bytes"]) is not int
        or descriptor["bytes"] <= 0
        or not isinstance(descriptor["sha256"], str)
        or HEX_64.fullmatch(descriptor["sha256"]) is None
    ):
        raise ValueError(f"{label}不是后端忽略目录内的完整文件描述")
    document = read_json_document(path)
    if _descriptor(document) != descriptor:
        raise ValueError(f"{label}与当前文件不一致")
    return document


def _runtime_observation(path: Path) -> dict:
    current = path.absolute()
    anchor = current
    while True:
        try:
            metadata = anchor.lstat()
        except FileNotFoundError:
            if anchor.parent == anchor:
                raise ValueError("fresh target 运行目录没有可信现有父目录")
            anchor = anchor.parent
            continue
        break
    reject_link_or_reparse(anchor)
    if not stat.S_ISDIR(metadata.st_mode):
        raise ValueError("fresh target 运行目录的最近现有路径不是目录")
    anchor = directory(anchor, "fresh target 运行目录或现有父目录")
    before = anchor.stat()
    if anchor == current:
        entries = sorted(item.name for item in anchor.iterdir())
        exists = True
    else:
        try:
            current.lstat()
        except FileNotFoundError:
            pass
        else:
            raise ValueError("fresh target 运行目录状态在观察期间发生变化")
        entries = []
        exists = False
    after = anchor.stat()
    identity = (before.st_dev, before.st_ino, before.st_ctime_ns)
    if identity != (after.st_dev, after.st_ino, after.st_ctime_ns):
        raise ValueError("fresh target 运行目录身份在观察期间发生变化")
    if entries:
        raise ValueError("fresh target 运行目录已出现 lifecycle、launch、进程树或未知文件")
    return {
        "exists": exists,
        "entries": entries,
        "anchor": {
            "path": str(anchor),
            "device": before.st_dev,
            "inode": before.st_ino,
            "created_ns": before.st_ctime_ns,
        },
    }


def _ports(endpoints: dict) -> list[str]:
    for role in ROLES:
        require_closed_port(endpoints[role])
    return list(ROLES)


def _facts(reference, target, reference_descriptor: dict, target_descriptor: dict) -> dict:
    product = target.value["product_plan"]
    selected = reference.value["target"]
    endpoints = {
        "api": canonical_endpoint(product["api_ready_url"], "/readyz", "目标 API 端点"),
        "worker": canonical_endpoint(product["worker_ready_url"], "/readyz", "目标 Worker 端点"),
        "frontend": canonical_endpoint(selected["frontend_url"], "", "目标前端端点"),
    }
    if (
        target.value["target_side"] != reference.value["target_side"]
        or product["scope_id"] != selected["scope_id"]
        or not isinstance(product["frontend_sha"], str)
        or HEX_40.fullmatch(product["frontend_sha"]) is None
        or len({endpoint(url)[2] for url in endpoints.values()}) != len(ROLES)
    ):
        raise ValueError("恢复运行登记与目标侧、scope 或三端端点不一致")
    return {
        "reference_plan": reference_descriptor,
        "target_plan": target_descriptor,
        "runtime_directory": str(Path(selected["runtime_dir"]).absolute()),
        "endpoints": endpoints,
        "product_plan": product,
        "fresh_target": target.value["fresh_target"],
        "maintenance_execution": target.value["maintenance_execution"],
        "product_execution": target.value["product_execution"],
    }


def registration_inputs(
    backend: Path,
    reference_path: Path,
    target_path: Path,
    *,
    target_verifier=None,
) -> tuple[dict, tuple[object, object]]:
    backend = repository(backend, "恢复运行登记后端")
    reference = _bound_document(backend, _descriptor(read_json_document(reference_path)), "参考恢复计划")
    target = _bound_document(backend, _descriptor(read_json_document(target_path)), "正式恢复目标计划")
    validate_plan(reference.value, backend)
    if target_verifier is None:
        from restore_reference_target import verify_target_plan
        target_verifier = verify_target_plan
    verified = target_verifier(backend, reference.value, target.path)
    if verified != target.value:
        raise ValueError("正式恢复目标计划核验结果与登记文件不同")
    facts = _facts(reference, target, _descriptor(reference), _descriptor(target))
    reference.assert_unchanged()
    target.assert_unchanged()
    return facts, (reference, target)


def registration_binding(
    backend: Path,
    registration_path: Path,
    target_plan_descriptor: dict,
) -> tuple[dict, dict, tuple[object, ...]]:
    """运行开始后只重验登记时已经证明的不可变文件绑定，不重演 fresh 状态。"""
    backend = repository(backend, "恢复运行登记后端")
    registration = read_json_document(registration_path)
    value = validate_registration(registration.value)
    if value["target_plan"] != target_plan_descriptor:
        raise ValueError("调用方绑定的目标计划描述与恢复运行登记不同")
    reference = _bound_document(backend, value["reference_plan"], "参考恢复计划")
    target = _bound_document(backend, value["target_plan"], "正式恢复目标计划")
    facts = _facts(reference, target, value["reference_plan"], value["target_plan"])
    for document in (registration, reference, target):
        document.assert_unchanged()
    return value, facts, (registration, reference, target)


def _observation(facts: dict) -> dict:
    return {
        "runtime": _runtime_observation(Path(facts["runtime_directory"])),
        "ports_idle": _ports(facts["endpoints"]),
    }


def runtime_control_directory(backend: Path) -> Path:
    """正式恢复共享外部资源，同一后端工作树只允许一个运行控制器。"""
    return (backend / ".local-tests").resolve(strict=True)


def validate_registration(value: object) -> dict:
    registration = exact_fields(
        value,
        {"format_version", "kind", "reference_plan", "target_plan", "observation", "remote_writes"},
        "恢复运行登记",
    )
    if (
        type(registration["format_version"]) is not int
        or registration["format_version"] != 1
        or registration["kind"] != "restore-runtime-registration"
        or type(registration["remote_writes"]) is not int
        or registration["remote_writes"] != 0
    ):
        raise ValueError("恢复运行登记版本、类型或远端写入声明无效")
    for label in ("reference_plan", "target_plan"):
        descriptor = exact_fields(registration[label], {"path", "bytes", "sha256"}, label)
        if (
            not isinstance(descriptor["path"], str)
            or not Path(descriptor["path"]).is_absolute()
            or type(descriptor["bytes"]) is not int
            or descriptor["bytes"] <= 0
            or not isinstance(descriptor["sha256"], str)
            or HEX_64.fullmatch(descriptor["sha256"]) is None
        ):
            raise ValueError("恢复运行登记的计划描述无效")
    observation = exact_fields(registration["observation"], {"runtime", "ports_idle"}, "零进程观察")
    runtime = exact_fields(observation["runtime"], {"exists", "entries", "anchor"}, "运行目录观察")
    anchor = exact_fields(runtime["anchor"], {"path", "device", "inode", "created_ns"}, "运行目录身份")
    if (
        type(runtime["exists"]) is not bool
        or runtime["entries"] != []
        or observation["ports_idle"] != list(ROLES)
        or not isinstance(anchor["path"], str)
        or not Path(anchor["path"]).is_absolute()
        or any(type(anchor[field]) is not int for field in ("device", "inode", "created_ns"))
    ):
        raise ValueError("恢复运行登记没有证明空目录和三端口空闲")
    return registration


def verify_registration(
    backend: Path,
    registration_path: Path,
    target_plan_descriptor: dict,
    *,
    target_verifier=None,
) -> tuple[dict, dict, tuple[object, ...]]:
    backend = repository(backend, "恢复运行登记后端")
    value, facts, documents = registration_binding(
        backend, registration_path, target_plan_descriptor
    )
    inputs_facts, inputs = registration_inputs(
        backend,
        Path(value["reference_plan"]["path"]),
        Path(value["target_plan"]["path"]),
        target_verifier=target_verifier,
    )
    if any(facts[key] != inputs_facts[key] for key in (
        "reference_plan", "target_plan", "runtime_directory", "endpoints"
    )):
        raise ValueError("恢复运行登记的计划输入已经变化")
    if value["observation"] != _observation(facts):
        raise ValueError("fresh target 运行目录或三端口不再保持登记的零进程状态")
    for document in (*documents, *inputs):
        document.assert_unchanged()
    return value, facts, (*documents, *inputs)


def register_runtime(
    backend: Path,
    reference_path: Path,
    target_path: Path,
    output_path: Path,
    *,
    target_verifier=None,
) -> dict:
    backend = repository(backend, "恢复运行登记后端")
    facts, inputs = registration_inputs(
        backend, reference_path, target_path, target_verifier=target_verifier
    )
    observation = _observation(facts)
    output = validate_new_output(output_path, backend)
    runtime = Path(facts["runtime_directory"])
    if output.is_relative_to(runtime):
        raise ValueError("恢复运行登记不能写入需要证明为空的目标运行目录")
    control = runtime_control_directory(backend)
    operation = "register:" + facts["target_plan"]["sha256"]
    with controller_lock(control, operation, REGISTRATION_LOCK):
        repeated, repeated_inputs = registration_inputs(
            backend, reference_path, target_path, target_verifier=target_verifier
        )
        if repeated != facts or _observation(repeated) != observation:
            raise ValueError("取得 ownership 控制锁前恢复目标或零进程状态发生变化")
        value = {
            "format_version": 1,
            "kind": "restore-runtime-registration",
            "reference_plan": facts["reference_plan"],
            "target_plan": facts["target_plan"],
            "observation": observation,
            "remote_writes": 0,
        }
        write_new(output, value, backend)
        verified, current, documents = verify_registration(
            backend, output, facts["target_plan"], target_verifier=target_verifier
        )
        if verified != value or current != facts:
            raise ValueError("恢复运行登记写入后的绑定核验不一致")
        for document in (*inputs, *repeated_inputs, *documents):
            document.assert_unchanged()
    return {"status": "runtime_registered_not_started", "registration": _descriptor(read_json_document(output))}


def _recheck_context(
    backend: Path,
    registration_path: Path,
    target_plan_descriptor: dict,
    value: dict,
    facts: dict,
    documents: tuple[object, ...],
    *,
    target_verifier=None,
) -> None:
    after, after_facts, after_documents = verify_registration(
        backend,
        registration_path,
        target_plan_descriptor,
        target_verifier=target_verifier,
    )
    if after != value or after_facts != facts:
        raise ValueError("完整恢复写入期间运行登记发生变化")
    for document in (*documents, *after_documents):
        document.assert_unchanged()


@contextmanager
def registered_stopped_runtime(
    backend: Path,
    registration_path: Path,
    target_plan_descriptor: dict,
    *,
    target_verifier=None,
):
    """在完整恢复写临界区前后证明同一 target 从未启动，并阻止并发 start。"""
    value, facts, documents = verify_registration(
        backend,
        registration_path,
        target_plan_descriptor,
        target_verifier=target_verifier,
    )
    control = runtime_control_directory(backend)
    operation = f"restore:{documents[0].sha256}:{facts['target_plan']['sha256']}"
    with controller_lock(control, operation, REGISTRATION_LOCK) as owner:
        current, current_facts, current_documents = verify_registration(
            backend,
            registration_path,
            target_plan_descriptor,
            target_verifier=target_verifier,
        )
        if current != value or current_facts != facts:
            raise ValueError("取得 ownership 控制锁前恢复运行登记发生变化")
        for document in (*documents, *current_documents):
            document.assert_unchanged()
        active = True

        def checkpoint() -> dict:
            if not active:
                raise ValueError("恢复运行 checkpoint 已离开 ownership 临界区")
            assert_controller_lock(control, owner, REGISTRATION_LOCK)
            _recheck_context(
                backend,
                registration_path,
                target_plan_descriptor,
                value,
                facts,
                (*documents, *current_documents),
                target_verifier=target_verifier,
            )
            return {"registration": _descriptor(current_documents[0]), "target_plan": facts["target_plan"]}

        try:
            yield checkpoint
        except BaseException as error:
            try:
                _recheck_context(
                    backend,
                    registration_path,
                    target_plan_descriptor,
                    value,
                    facts,
                    (*documents, *current_documents),
                    target_verifier=target_verifier,
                )
            except BaseException as after:
                error.add_note("恢复失败后零进程状态复核也失败：" + str(after))
            raise
        else:
            _recheck_context(
                backend,
                registration_path,
                target_plan_descriptor,
                value,
                facts,
                (*documents, *current_documents),
                target_verifier=target_verifier,
            )
        finally:
            active = False


def execute(args, backend: Path) -> dict:
    return register_runtime(backend, args.plan, args.target_plan, args.output)
