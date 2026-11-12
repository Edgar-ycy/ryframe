"""现有恢复接口的有界适配；前端规则仅由已绑定的 Rust 权威实现。"""
from __future__ import annotations

from contextlib import ExitStack
import json
from pathlib import Path
import subprocess

from rate_limit_tool import install_binding, tool_binding

ROOT = Path(__file__).resolve().parents[2]
B0_HEAD = "0087ea2ecf62530d042b9e52f5c950fb34c66d78"
B0_BACKEND_ADAPTER = "legacy-stable-readiness-b0-v1"


def b0_runtime_source(source: dict) -> bool:
    return (
        source["frontend_sha"] == B0_HEAD
        and source["backend_adapter_contract"] == B0_BACKEND_ADAPTER
        and source["backend_product_sha"] == "815c5eafb09d4b493319d255fb7ab88ddd02c8b6"
        and source["backend_execution_sha"] == "c05114bcdf5c369cd74087db6317ce3c8f89bee8"
    )


def request(action: str, root: Path = ROOT, value=None, *, expected_head=None, inventory=None):
    # 延迟导入：origin authority 的通用 JSON/path helper 不反向依赖本适配。
    from native_s3_client import _invoke, _read_response

    with ExitStack() as held:
        if tool_binding() is None:
            # 独立恢复 HTTP server 沿用现存固定 origin 绑定，无 PATH/环境发现或补编译。
            from artifact_digests import file_digest
            from frontend_origin_tool import _verified_tool

            executable = held.enter_context(_verified_tool())
            binding = {"format_version": 1, "kind": "ryframe-native-rate-limit-tool",
                       "path": str(executable), **file_digest(executable)}
            install_binding(json.dumps(binding, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        result, response = _invoke("__source-evidence", {
            "operation": "frontend-build-validation", "write": False,
            "arguments": {"action": action, "root": str(root), "value": value,
                          "expected_head": expected_head, "inventory": inventory},
        }, subprocess.run, may_write=False)
        return _read_response(result, response)[0]["value"]
