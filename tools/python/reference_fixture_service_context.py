"""服务生命周期的只读输入快照与精确进程观察。"""
from __future__ import annotations

from pathlib import Path

from rate_limit_client import load_app_table
from fixture_service_client import _invoke


def service_run(review: dict) -> Path:
    return Path(_invoke("service-run", {"review": review}))


def application_settings(bootstrap_file: Path, execution: Path) -> dict:
    """从已绑定 bootstrap 相邻的唯一明文配置目录读取应用值。"""
    return load_app_table(execution, {"APP_ENV": "test"}, config_dir=bootstrap_file.parent / "config")


def load_prepared_environment(backend: Path, value: Path, secret_files: tuple[str, ...]) -> tuple[Path, Path, dict, dict]:
    receipt, execution, private, document = _invoke("service-prepared", {"path": value, "secret_files": secret_files}, backend=backend)
    return Path(receipt), Path(execution), private, document


def context(backend: Path, review_path: Path, bootstrap_path: Path) -> dict:
    value = _invoke("service-context", {"review": review_path, "environment": bootstrap_path}, backend=backend)
    value["backend"], value["run"] = Path(value["backend"]), Path(value["run"])
    return value


def guard(value: dict, state_binding: dict | None = None) -> None:
    _invoke("service-guard", {"value": value, "running": state_binding}, backend=value["backend"])


def registered_services(value: dict, *, settled: bool = True) -> dict:
    return _invoke("service-registered", {"value": value, "settled": settled}, backend=value["backend"])


def observe_services(services: dict) -> dict:
    return _invoke("service-observe", {"services": services})


def runtime_transition(backend: Path, run: Path, expected: dict) -> tuple[dict | None, dict | None]:
    return tuple(_invoke("service-transition", {"run": run, "expected": expected}, backend=backend))
