"""恢复前端配置接口；只适配同一原生权威的无正文事实。"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from restore_frontend_native import request


@dataclass(frozen=True)
class FrontendInputs:
    root: Path
    environment: str
    configuration: dict
    identity: dict
    observed: list

    def assert_unchanged(self) -> None:
        # IPC 返回快照而不持有文件；窗口末端重读并比较身份、存在性和摘要。
        if _capture(self.root, self.environment) != self:
            raise ValueError("前端配置、工具、身份或所选文件集合漂移；请稳定文件后重新构建")


def _capture(root: Path, environment: str) -> FrontendInputs:
    facts = request("capture-inputs", root, environment)
    return FrontendInputs(root, environment, facts["configuration"], facts["identity"], facts["observed"])


def verify_frontend_inputs(root: Path, build: dict, *, expected_head: str | None = None) -> FrontendInputs:
    facts = request("inputs", root, build, expected_head=expected_head)
    return FrontendInputs(root, facts["configuration"]["environment"],
                          facts["configuration"], facts["identity"], facts["observed"])
