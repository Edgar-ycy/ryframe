"""仍在迁移的初始化消费者复用唯一原生 prepare 历史校验。"""
from pathlib import Path

from devex_clone_target_evidence import evidence


def validate_prepared_tree(backend: Path, target: Path, files: list[dict],
                           request_descriptor: dict, request: dict) -> None:
    evidence("validate-prepared-tree", backend, target, arguments={
        "files": files, "request_descriptor": request_descriptor, "request": request})
