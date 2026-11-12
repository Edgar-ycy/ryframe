"""严格恢复目标计划、已有数据检查、来源数据准备与备份恢复；写入阶段要求 --write。"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

from dataclasses import dataclass
import json
import os
import subprocess
from pathlib import Path

from restore_build import file_digest
from restore_reference_backup import document_binding
from restore_reference_execution import RestoreInputs, restore_inputs, restore_output
from restore_reference_images import RestoreImages
from restore_reference_io import ExternalTools, redact_object_diagnostic
from restore_reference_plan import plan_hash, validate_plan, verify_artifacts
from restore_runtime_evidence import read_json
from restore_runtime_registration import registered_stopped_runtime
from restore_reference_target import execute_plan
from native_s3_client import _invoke, _read_response


def _coordinate(operation: str, arguments: dict, *, write=False):
    response, _ = _read_response(*_invoke(
        "__source-evidence", {"operation": "reference-" + operation,
            "arguments": arguments, "write": write}, subprocess.run, may_write=False,
    ))
    return response["value"]


def now() -> str:
    from datetime import datetime, timezone
    return datetime.now(timezone.utc).isoformat()


def write_json(path: Path, value: dict, *, new=True) -> None:
    with path.open("x" if new else "w", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")


def failure_diagnostic(error: BaseException) -> dict:
    """记录外部命令的脱敏输出，保留未知写入阶段的定位证据。"""
    result = {"error_type": type(error).__name__, "message": str(error)[:1000]}
    if getattr(error, "__notes__", None):
        result["notes"] = [redact_object_diagnostic(note[:1000], os.environ) for note in error.__notes__[:20]]
    if isinstance(error, subprocess.CalledProcessError):
        result["stdout"] = redact_object_diagnostic(error.stdout, os.environ)
        result["stderr"] = redact_object_diagnostic(error.stderr, os.environ)
    return result


def work_directory(plan: dict) -> Path:
    return Path(_coordinate("work-directory", {"plan": plan}, write=True))


def artifact(root: Path, path: Path, resource: str) -> dict:
    return {"relative_path": path.relative_to(root).as_posix(), "resource": resource, **file_digest(path)}


def restore(plan: dict, tools: ExternalTools, backend: Path, inputs: RestoreInputs) -> dict:
    inputs.assert_unchanged()
    result = None
    try:
        with registered_stopped_runtime(backend, inputs.registration.path, document_binding(inputs.target)) as checkpoint:
            result = restore_locked(plan, tools, backend, inputs, checkpoint)
        return result
    except BaseException as error:
        if result is not None:
            error.restore_evidence = result["resource_images"]
        raise


def restore_locked(plan: dict, tools: ExternalTools, backend: Path, inputs: RestoreInputs, checkpoint) -> dict:
    runtime = checkpoint()
    current = restore_inputs(backend, plan, inputs.target.path, inputs.root, inputs.record.path,
                             inputs.registration.path, read_only=False)
    if current.bindings() != inputs.bindings():
        raise ValueError("恢复执行前目标计划或运行记录发生变化")
    root, manifest, record = current.root, current.manifest.value, current.record.value
    tasks = _coordinate("restore-resources", {"plan": plan, "root": str(root), "manifest": manifest})
    images = RestoreImages(backend, plan, current, tools)
    try:
        with images.control():
            checkpoint()
            images.capture("before", after=False)
            checkpoint()
            current.assert_unchanged()
            try:
                restore_resources(plan, tools, current, tasks, checkpoint, images)
                checkpoint()
                verify_artifacts(root, manifest)
                images.capture("after", after=True)
            except BaseException as error:
                if "after" not in images.bindings:
                    try:
                        checkpoint()
                        images.capture("failure-after", after=True)
                    except BaseException as failure:
                        error.add_note("恢复失败后完整资源像复核也失败：" + type(failure).__name__)
                raise
            checkpoint()
            final = restore_inputs(backend, plan, current.target.path, current.root, current.record.path,
                                   current.registration.path, read_only=True)
            if final.bindings() != current.bindings():
                raise ValueError("恢复完成复核时来源或输入绑定发生变化")
    except BaseException as error:
        error.restore_evidence = dict(images.bindings)
        raise
    return _coordinate("restore-result", {
        "record": record, "bindings": current.bindings(), "runtime": runtime, "images": images.bindings,
    })


def restore_resources(plan: dict, tools: ExternalTools, current: RestoreInputs, tasks: list,
                      checkpoint, images: RestoreImages) -> None:
    for task in tasks:
        checkpoint()
        current.assert_unchanged()
        bound = _coordinate("bound-artifact", {
            "root": str(current.root), "manifest": current.manifest.value, "relative": task["relative"],
        })
        source = Path(bound["path"])
        expected = {key: bound[key] for key in ("bytes", "sha256")}
        with images.target_environment():
            if task["kind"] == "database":
                tools.restore_database(task["target"], task["tables"], source, expected)
            else:
                tools.create_object_if_absent(task["bucket"], task["key"], source, task["metadata"], expected)



class ReferenceProtocolError(ValueError):
    """Rust 与私有阶段之间的参数协议无效。"""


@dataclass
class ReferenceRequest:
    command: str
    backend_dir: Path
    plan: Path
    write: bool
    backup_root: Path | None = None
    record: Path | None = None
    runtime_registration: Path | None = None
    copy_id: str | None = None
    artifact: str | None = None
    side: str | None = None
    backup_receipt: Path | None = None
    comparison_sources: Path | None = None
    arm_input: Path | None = None
    fresh_target_verify: Path | None = None
    product_plan: Path | None = None
    target_plan: Path | None = None
    output: Path | None = None
    missing: bool = False
    restore_inputs: object | None = None


def private_protocol_request(argv: list[str] | None = None) -> ReferenceRequest:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) == 3 and arguments[0] == "--native-tool" and arguments[2] == "--stdin":
        from rate_limit_tool import install_binding
        try:
            install_binding(arguments[1])
        except (ValueError, TypeError) as error:
            raise ReferenceProtocolError("reference 原生工具绑定无效") from error
        arguments = ["--stdin"]
    if arguments != ["--stdin"]:
        raise ReferenceProtocolError("reference 私有脚本只接受 --stdin")
    raw = sys.stdin.buffer.read(65_537)
    try:
        return _request_from_raw(raw.decode("utf-8"))
    except (ValueError, UnicodeError) as error:
        raise ReferenceProtocolError("reference stdin 私有协议无效") from error


def _request_from_raw(raw: str) -> ReferenceRequest:
    try:
        values = _coordinate("request", {"raw": raw})
    except (ValueError, RuntimeError) as error:
        raise ReferenceProtocolError("reference 私有参数协议无效") from error
    if values["command"] in {"dataset", "backup", "restore"}:
        raise ReferenceProtocolError("参考数据准备、备份和恢复由 Rust 公共入口执行")
    paths = {name: Path(value) for name, value in values.items() if name in {
        "backend_dir", "plan",
        "backup_root", "record", "runtime_registration", "backup_receipt", "comparison_sources",
        "arm_input", "fresh_target_verify", "product_plan", "target_plan", "output"}}
    return ReferenceRequest(**{**values, **paths})


def main(args: ReferenceRequest) -> None:
    if args.command not in {"plan", "restore"}:
        raise ReferenceProtocolError("该参考阶段由 Rust 公共入口执行")
    backend, plan = args.backend_dir.resolve(strict=True), read_json(args.plan)
    validate_plan(plan, backend)
    if args.command == "plan":
        result = execute_plan(args, backend, plan)
        print(json.dumps(result if result is not None else _coordinate("summary", {"plan": plan}), ensure_ascii=False))
        return
    if not args.write:
        raise ReferenceProtocolError("所有执行阶段必须显式授权写入")
    if args.command == "restore":
        args.restore_inputs = restore_inputs(backend, plan, args.target_plan, args.backup_root, args.record,
                                             args.runtime_registration, read_only=True)
        restore_output(backend, plan, args.restore_inputs)
    work = work_directory(plan)
    tools = ExternalTools(plan, work)
    lock = work / ".reference-lock"
    lock.mkdir(exist_ok=False)
    try:
        result = execute(args, plan, backend, tools, work)
        print(json.dumps(result, ensure_ascii=False))
    finally:
        lock.rmdir()


def execute(args, plan: dict, backend: Path, tools: ExternalTools, work: Path) -> dict:
    if args.command != "restore":
        raise ReferenceProtocolError("参考写入阶段必须通过 Rust 公共入口运行")
    inputs = getattr(args, "restore_inputs", None) or restore_inputs(
        backend, plan, args.target_plan, args.backup_root, args.record, args.runtime_registration, read_only=True)
    output = restore_output(backend, plan, inputs)
    receipt = _coordinate("receipt-start", {
        "plan": plan, "command": args.command, "copy_id": args.copy_id,
        "root": None,
        "receipt": str(output),
        "bindings": inputs.bindings(),
    }, write=True)
    try:
        result = restore(plan, tools, backend, inputs)
        _coordinate("receipt-finish", {"receipt": receipt, "result": result,
            "failure": None, "resource_images": None}, write=True)
        return result
    except BaseException as error:
        try:
            _coordinate("receipt-finish", {"receipt": receipt, "result": None,
                "failure": failure_diagnostic(error),
                "resource_images": getattr(error, "restore_evidence", None)}, write=True)
        except BaseException as failure:
            error.add_note("失败收据发布未确认，保留运行收据和未知结果：" + type(failure).__name__)
        raise


if __name__ == "__main__":
    try:
        main(private_protocol_request())
    except ReferenceProtocolError:
        print("restore_reference_protocol_error：私有参数协议无效。", file=sys.stderr)
        raise SystemExit(2)
    except Exception:
        print("restore_reference_failed：阶段失败，请核对账本、前后像和未知结果；不自动重放。",
              file=sys.stderr)
        raise SystemExit(1)
