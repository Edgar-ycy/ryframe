"""在已审阅的 Device 夹具环境中构建并控制唯一的源运行时。"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess

from ci_full_stack_resources import build_binaries
from devex_clone_capture import read_json, write_json
from devex_clone_factory_context import Environments, configured
from devex_clone_model import linked, local_path
from devex_clone_runtime import control
from full_stack_provenance import verify_build_evidence
from full_stack_runtime import register_runtime, verify_runtime
from reference_fixture_source_pair import write_pair
from restore_build import build_command, build_context, file_digest, source_snapshot, verify_build_artifacts
from source_inventory import build_source_domains, capture_inventory


def _bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _read(backend: Path, value: Path) -> tuple[Path, dict]:
    requested = value if value.is_absolute() else backend / value
    path = local_path(backend, str(requested))
    if linked(path) or not path.is_file():
        raise ValueError("夹具运行时输入必须是受控普通文件")
    return path, read_json(path)


def _bootstrap(backend: Path, path: Path) -> tuple[Path, Path, dict]:
    receipt_path, receipt = _read(backend, path)
    if (receipt.get("format_version") != 1 or receipt.get("kind") != "reference-fixture-environment"
            or receipt.get("status") != "prepared" or receipt.get("services_started") is not False
            or receipt.get("remote_writes") != 0 or not isinstance(receipt.get("execution_backend"), str)):
        raise ValueError("夹具运行时只能使用尚未启动服务的已准备私有环境")
    execution = Path(receipt["execution_backend"])
    if not execution.is_absolute() or linked(execution) or not (execution / "Cargo.toml").is_file():
        raise ValueError("夹具执行工作树无效")
    environment_path = receipt_path.parent / "environment.json"
    environment = read_json(environment_path).get("environment")
    if (not isinstance(environment, dict) or any(not isinstance(key, str) or not isinstance(value, str)
                                                  for key, value in environment.items())):
        raise ValueError("夹具私有环境无效")
    return receipt_path, execution, environment


def _output(execution: Path, value: Path, *, new: bool) -> Path:
    path = value.resolve() if value.is_absolute() else (execution / value).resolve()
    root = (execution / ".local-tests/reference-fixture").resolve()
    if (not path.is_relative_to(root) or path == root or linked(path)
            or (new and (path.exists() or not path.parent.is_dir()))
            or (not new and (not path.is_dir() or linked(path)))):
        raise ValueError("夹具运行时目录必须是参考夹具根内的明确新目录")
    return path


def _runtime_environment(values: dict, pair: dict) -> dict:
    source = pair["sources"]["backend"]
    head = source.get("head") if isinstance(source, dict) else None
    if not isinstance(head, str):
        raise ValueError("夹具来源组合缺少后端提交")
    return {**configured(values), "APP_API_DOCS_ENABLED": "false",
            "RYFRAME_E2E_FIXTURE": "device", "RYFRAME_CODE_SHA": head}


def _environment(execution: Path, values: dict, output: Path) -> dict:
    return _runtime_environment(values, write_pair(execution, output / "source-pair.json"))


def _run(command: list[str], *, cwd: Path, capture_output: bool) -> subprocess.CompletedProcess:
    return subprocess.run(command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                          encoding="utf-8", check=True,
                          creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)


def _backend_build(execution: Path, output: Path, binaries: dict[str, str], inventory: dict,
                   context: dict) -> dict:
    sources = build_source_domains(inventory, "backend")
    artifacts = {
        role: {"executable": binaries[name], "command": build_command(role), **file_digest(Path(binaries[name]))}
        for role, name in (("api", "ryframe"), ("worker", "ryframe-worker"))
    }
    receipt = {"format_version": 2, "kind": "restore-backend-build", "sources": sources,
               "build": context, "artifacts": artifacts}
    if capture_inventory(execution) != inventory or build_context(execution) != context:
        raise ValueError("夹具源构建期间源码发生变化")
    write_json(output / "backend-build.json", receipt)
    return receipt


def build(backend: Path, environment_path: Path, output_path: Path) -> dict:
    _, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=True)
    output.mkdir()
    try:
        values = _environment(execution, private, output)
        with Environments(values, values).use("source"):
            source = source_snapshot(execution)
            inventory = capture_inventory(execution, source)
            context = build_context(execution)
            binaries = build_binaries(_run, execution, output)
            backend_build = _backend_build(execution, output, binaries, inventory, context)
            runtime = register_runtime(execution, output)
        return {"status": "reference_fixture_runtime_built", "source_pair": _bound(output / "source-pair.json"),
                "backend_build": _bound(output / "backend-build.json"), "runtime": _bound(output / "runtime.json"),
                "scope_id": runtime["scope_id"], "remote_writes": 0}
    except BaseException as error:
        write_json(output / "failure.json", {"status": "reference_fixture_runtime_failed",
                                               "error_type": type(error).__name__})
        raise


def verify(backend: Path, environment_path: Path, output_path: Path) -> dict:
    _, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=False)
    pair = read_json(output / "source-pair.json")
    values = _runtime_environment(private, pair)
    with Environments(values, values).use("source"):
        build = read_json(output / "backend-build.json")
        verify_build_artifacts(build)
        verify_build_evidence(execution, output)
        runtime = verify_runtime(execution, output)
    return {"status": "reference_fixture_runtime_verified", "runtime": _bound(output / "runtime.json"),
            "scope_id": runtime["scope_id"], "remote_writes": 0}


def run(backend: Path, environment_path: Path, output_path: Path, operation: str) -> dict:
    if operation not in {"start", "stop", "status"}:
        raise ValueError("夹具运行时操作无效")
    verified = verify(backend, environment_path, output_path)
    _, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=False)
    pair = read_json(output / "source-pair.json")
    values = _runtime_environment(private, pair)
    api_url = f"http://{values['APP_APP_HOST']}:{values['APP_APP_PORT']}"
    with Environments(values, values).use("source"):
        result = control(execution, output, operation, ("api", "worker"), api_url)
    return {"status": "reference_fixture_runtime_" + operation, "runtime": verified["runtime"],
            "processes": {role: value["state"] for role, value in result["processes"].items()},
            "remote_writes": 0}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("build", "verify", "start", "stop", "status"))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.operation in {"build", "start", "stop"} and not args.write:
        parser.error("构建、启动和停止夹具运行时需要显式 --write")
    if args.operation in {"verify", "status"} and args.write:
        parser.error("夹具运行时只读核验不接受 --write")
    backend = args.backend_dir.resolve(strict=True)
    result = (build(backend, args.environment, args.output) if args.operation == "build"
              else verify(backend, args.environment, args.output) if args.operation == "verify"
              else run(backend, args.environment, args.output, args.operation))
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
