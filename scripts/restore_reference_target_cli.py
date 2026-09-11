"""参考恢复现有 plan 阶段的严格目标计划参数与发布。"""

from pathlib import Path

from devex_clone_capture import write_json
from devex_clone_model import local_path
from restore_reference_plan import plan_hash
from restore_reference_target import capture_target_plan, plan_output, verify_target_plan
from restore_runtime_evidence import read_json_document


INPUTS = ("backup_receipt", "comparison_sources", "arm_input", "fresh_target_verify", "product_plan")


def add_arguments(parser) -> None:
    for field in (*INPUTS, "target_plan", "output"):
        parser.add_argument("--" + field.replace("_", "-"), type=Path)


def validate_arguments(parser, args) -> None:
    creating = any(getattr(args, field) is not None for field in INPUTS)
    if (creating or args.output is not None) and args.command != "plan":
        parser.error("目标计划创建输入及 --output 仅用于 plan")
    if args.target_plan is not None and args.command not in {"plan", "restore"}:
        parser.error("--target-plan 仅用于 plan 复核或 restore 执行")
    if args.command != "plan":
        return
    if creating and (args.target_plan is not None or any(getattr(args, field) is None for field in INPUTS)):
        parser.error("目标计划必须完整提供五份输入，且不能同时指定 --target-plan")
    if (args.output is not None) != args.write or args.write and not creating:
        parser.error("仅完整目标计划可通过 --output 与 --write 显式发布")


def execute_plan(args, backend: Path, plan: dict) -> dict | None:
    reference = read_json_document(args.plan)
    if reference.value != plan:
        raise ValueError("参考计划在目标核对前发生变化")
    if args.target_plan is not None:
        value = verify_target_plan(backend, plan, args.target_plan)
        reference.assert_unchanged()
        return {"status": "target_plan_verified", "target_side": value["target_side"],
                "target_plan_sha256": plan_hash(value), "restore_success": False}
    if args.backup_receipt is None:
        return None
    if args.output is not None:
        local_path(backend, str(args.output.absolute()), new=True)
    value = capture_target_plan(backend, plan, **{field: getattr(args, field) for field in INPUTS})
    reference.assert_unchanged()
    if args.output is None:
        return value
    output = plan_output(backend, args.output, value)
    write_json(output, value)
    return {"status": "target_plan_published", "output": str(output), "target_side": value["target_side"],
            "target_plan_sha256": plan_hash(value), "restore_success": False}
