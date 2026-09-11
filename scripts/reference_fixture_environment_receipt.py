"""严格读取 Device 参考夹具的唯一已准备环境收据。"""

from __future__ import annotations

from pathlib import Path
from typing import Callable

from devex_clone_capture import read_json
from devex_clone_model import linked, local_path
from restore_reference_plan import plan_hash


def load(backend: Path, value: Path, secret_files: tuple[str, ...],
         bound: Callable[[Path], dict]) -> tuple[Path, Path, dict, dict]:
    backend = backend.resolve(strict=True)
    requested = value if value.is_absolute() else backend / value
    receipt_path = local_path(backend, str(requested))
    if linked(receipt_path) or not receipt_path.is_file():
        raise ValueError("夹具输入必须是受控目录中的普通文件")
    receipt = read_json(receipt_path)
    if (receipt.get("format_version") != 1 or receipt.get("kind") != "reference-fixture-environment"
            or receipt.get("status") != "prepared" or receipt.get("services_started") is not False
            or receipt.get("remote_writes") != 0 or receipt.get("historical_data_used") is not False
            or not isinstance(receipt.get("execution_backend"), str)
            or not isinstance(receipt.get("secret_files"), dict)
            or set(receipt["secret_files"]) != set(secret_files)
            or not isinstance(receipt.get("environment_sha256"), str)):
        raise ValueError("夹具运行时只能使用尚未启动服务的已准备私有环境")
    execution = Path(receipt["execution_backend"]).resolve(strict=True)
    if (linked(execution) or not execution.is_relative_to((backend / ".local-tests").resolve())
            or not (execution / "Cargo.toml").is_file()):
        raise ValueError("夹具执行工作树无效")
    environment_file = receipt_path.parent / "environment.json"
    if linked(environment_file) or not environment_file.is_file():
        raise ValueError("夹具私有环境文件缺失或经过链接")
    document = read_json(environment_file)
    if not isinstance(document, dict) or set(document) != {"environment"}:
        raise ValueError("夹具私有环境文件字段无效")
    environment = document["environment"]
    if (not isinstance(environment, dict)
            or any(not isinstance(key, str) or not isinstance(item, str)
                   for key, item in environment.items())
            or plan_hash(environment) != receipt["environment_sha256"]):
        raise ValueError("夹具私有环境与 bootstrap 摘要不一致")
    for name in secret_files:
        descriptor = receipt["secret_files"][name]
        if not isinstance(descriptor, dict) or set(descriptor) != {"path", "bytes", "sha256"}:
            raise ValueError("夹具秘密文件描述不完整")
        path = local_path(backend, descriptor["path"] if isinstance(descriptor.get("path"), str) else "")
        if path.name != name or linked(path) or not path.is_file() or bound(path) != descriptor:
            raise ValueError("夹具秘密文件与 bootstrap 绑定不一致")
    return receipt_path, execution, environment, receipt
