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
from devex_clone_model import linked
from devex_clone_runtime import control, observe
from full_stack_provenance import verify_build_evidence
from full_stack_runtime import register_runtime, verify_runtime
from reference_fixture_browser import RuntimeApi, bind_browser, run_browser, verify_browser
from reference_fixture_environment import prepared_environment
from reference_fixture_source_pair import write_pair
from restore_build import build_command, build_context, file_digest, source_snapshot, verify_build_artifacts
from source_inventory import build_source_domains, capture_inventory


def _bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _bootstrap(backend: Path, path: Path) -> tuple[Path, Path, dict]:
    receipt_path, execution, environment, _ = prepared_environment(backend, path)
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
            _backend_build(execution, output, binaries, inventory, context)
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


def observe_runtime(backend: Path, environment_path: Path, output_path: Path,
                    operation: str) -> dict:
    if operation != "status":
        raise ValueError("夹具运行时只读观察仅支持 status")
    verified = verify(backend, environment_path, output_path)
    _, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=False)
    pair = read_json(output / "source-pair.json")
    values = _runtime_environment(private, pair)
    api_url = f"http://{values['APP_APP_HOST']}:{values['APP_APP_PORT']}"
    with Environments(values, values).use("source"):
        result = observe(execution, output, ("api", "worker"), api_url)
    return {"status": "reference_fixture_runtime_observed", "runtime": verified["runtime"],
            "processes": {role: value["state"] for role, value in result["processes"].items()},
            "remote_writes": 0}


def _browser_api() -> RuntimeApi:
    return RuntimeApi(_bootstrap, _output, verify, run, observe_runtime)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=(
        "build", "verify", "start", "stop", "status", "bind", "browser",
        "browser-verify", "browser-close",
    ))
    parser.add_argument("--backend-dir", type=Path, required=True)
    parser.add_argument("--environment", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--browser-binding", type=Path)
    parser.add_argument("--run-id")
    parser.add_argument("--server", choices=("dev", "preview"))
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.operation in {"build", "start", "stop", "bind", "browser"} and not args.write:
        parser.error("构建、启动、停止、绑定和执行夹具运行时需要显式 --write")
    if args.operation in {"verify", "status", "browser-verify", "browser-close"} and args.write:
        parser.error("夹具运行时只读核验不接受 --write")
    if args.operation == "bind" and (
            args.browser_binding is None or args.run_id is None or args.server is None):
        parser.error("Device 浏览器 bind 需要 --browser-binding、--run-id 与 --server")
    consumers = {"browser", "browser-verify", "browser-close"}
    if args.operation in consumers and (
            args.browser_binding is None or args.run_id is not None or args.server is not None):
        parser.error("Device 浏览器执行和只读消费只接受 --browser-binding")
    if args.operation not in {"bind", *consumers} and (
            args.browser_binding is not None or args.run_id is not None or args.server is not None):
        parser.error("Device 浏览器参数只用于 bind/browser/browser-verify/browser-close")
    backend = args.backend_dir.resolve(strict=True)
    result = (build(backend, args.environment, args.output) if args.operation == "build"
              else verify(backend, args.environment, args.output) if args.operation == "verify"
              else bind_browser(_browser_api(), backend, args.environment, args.output,
                                args.browser_binding, args.run_id, args.server) if args.operation == "bind"
              else run_browser(_browser_api(), backend, args.environment, args.output,
                               args.browser_binding) if args.operation == "browser"
              else verify_browser(_browser_api(), backend, args.environment, args.output,
                                  args.browser_binding, closed=False)
              if args.operation == "browser-verify"
              else verify_browser(_browser_api(), backend, args.environment, args.output,
                                  args.browser_binding, closed=True)
              if args.operation == "browser-close"
              else run(backend, args.environment, args.output, args.operation))
    print(json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
