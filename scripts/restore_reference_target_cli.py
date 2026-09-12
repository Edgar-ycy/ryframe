"""参考恢复现有 plan 阶段的严格目标计划参数与发布。"""

from pathlib import Path

from devex_clone_capture import write_json
from devex_clone_model import local_path
from restore_reference_plan import plan_hash
from restore_reference_target import capture_target_plan, plan_output, verify_target_plan
from restore_runtime_evidence import read_json_document


INPUTS = ("backup_receipt", "comparison_sources", "arm_input", "fresh_target_verify", "product_plan")


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
