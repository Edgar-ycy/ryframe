"""在已审阅的 业务 crate 夹具环境中构建并控制唯一的源运行时。"""

from __future__ import annotations

from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import urlsplit

from ci_full_stack_resources import build_binaries
from devex_clone_capture import read_json, write_json
from process_environment import Environments, configured
from devex_clone_model import linked
from devex_clone_runtime import control, observe
from full_stack_provenance import verify_build_evidence
from full_stack_runtime import register_runtime, verify_runtime
from reference_fixture_environment import prepared_environment
from reference_fixture_service_context import application_settings
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
    return configured(values)


def _environment(execution: Path, values: dict, output: Path) -> dict:
    return _runtime_environment(values, write_pair(execution, output / "source-pair.json"))


def _run(command: list[str], *, cwd: Path, capture_output: bool) -> subprocess.CompletedProcess:
    return subprocess.run(command, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                          encoding="utf-8", check=True,
                          creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)


def _worker_control(execution: Path, output: Path, operation: str) -> str:
    """Worker 必须由正式全栈监督器启动并登记进程树；浏览器用例按该入口停止或崩溃 Worker。"""
    if operation not in {"start", "stop", "status"}:
        raise ValueError("夹具 Worker 控制操作无效")
    from full_stack_worker_native import invoke
    value = invoke("control", action=operation, backend=str(execution), directory=str(output), timeout=60)
    contract = read_json(output / "runtime.json")
    state = value.get("state") if isinstance(value, dict) else None
    if (not isinstance(value, dict) or value.get("operation") != operation
            or value.get("scope_id") != contract["scope_id"] or state not in {"running", "stopped"}):
        raise ValueError("Worker 控制收据与当前运行或操作不一致")
    return state


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
    bootstrap_path, execution, private = _bootstrap(backend, environment_path)
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
            runtime = register_runtime(execution, output, source_config_dir=bootstrap_path.parent / "config")
        return {"status": "reference_fixture_runtime_built", "source_pair": _bound(output / "source-pair.json"),
                "backend_build": _bound(output / "backend-build.json"), "runtime": _bound(output / "runtime.json"),
                "scope_id": runtime["scope_id"], "remote_writes": 0}
    except BaseException as error:
        write_json(output / "failure.json", {"status": "reference_fixture_runtime_failed",
                                               "error_type": type(error).__name__})
        raise


def verify(backend: Path, environment_path: Path, output_path: Path) -> dict:
    bootstrap_file, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=False)
    pair = read_json(output / "source-pair.json")
    values = _runtime_environment(private, pair)
    with Environments(values, values).use("source"):
        build = read_json(output / "backend-build.json")
        verify_build_artifacts(build)
        build_evidence = verify_build_evidence(execution, output)
        runtime = verify_runtime(execution, output)
    return {"status": "reference_fixture_runtime_verified", "runtime": _bound(output / "runtime.json"),
            "scope_id": runtime["scope_id"], "build_evidence": build_evidence, "remote_writes": 0}


def run(backend: Path, environment_path: Path, output_path: Path, operation: str) -> dict:
    if operation not in {"start", "stop", "status"}:
        raise ValueError("夹具运行时操作无效")
    verified = verify(backend, environment_path, output_path)
    bootstrap_file, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=False)
    pair = read_json(output / "source-pair.json")
    values = _runtime_environment(private, pair)
    settings = application_settings(bootstrap_file, execution)
    api_url = f"http://{settings['app']['host']}:{settings['app']['port']}"
    with Environments(values, values).use("source"):
        # 先停 Worker 再停 API，先起 API 再起受监督的 Worker。
        worker = _worker_control(execution, output, "stop") if operation == "stop" else None
        result = control(execution, output, operation, ("api",), api_url)
        if worker is None:
            worker = _worker_control(execution, output, operation)
    processes = {role: value["state"] for role, value in result["processes"].items()}
    processes["worker"] = worker
    return {"status": "reference_fixture_runtime_" + operation, "runtime": verified["runtime"],
            "processes": processes, "remote_writes": 0}


def observe_runtime(backend: Path, environment_path: Path, output_path: Path,
                    operation: str) -> dict:
    if operation != "status":
        raise ValueError("夹具运行时只读观察仅支持 status")
    verified = verify(backend, environment_path, output_path)
    bootstrap_file, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=False)
    pair = read_json(output / "source-pair.json")
    values = _runtime_environment(private, pair)
    settings = application_settings(bootstrap_file, execution)
    api_url = f"http://{settings['app']['host']}:{settings['app']['port']}"
    with Environments(values, values).use("source"):
        result = observe(execution, output, ("api",), api_url)
        worker = _worker_control(execution, output, "status")
    processes = {role: value["state"] for role, value in result["processes"].items()}
    processes["worker"] = worker
    return {"status": "reference_fixture_runtime_observed", "runtime": verified["runtime"],
            "processes": processes, "remote_writes": 0}


def worker_only(backend: Path, environment_path: Path, output_path: Path,
                operation: str) -> str:
    """只控制受监督 Worker 的运行态；API 保持原实例，供浏览器证据阶段恢复运行。"""
    if operation not in {"start", "stop", "status"}:
        raise ValueError("夹具 Worker 控制操作无效")
    verify(backend, environment_path, output_path)
    bootstrap_file, execution, private = _bootstrap(backend, environment_path)
    output = _output(execution, output_path, new=False)
    pair = read_json(output / "source-pair.json")
    values = _runtime_environment(private, pair)
    with Environments(values, values).use("source"):
        return _worker_control(execution, output, operation)


OPERATIONS = frozenset({
    "build", "verify", "start", "stop", "status", "bind", "browser",
    "browser-verify", "browser-close",
})
WRITING_OPERATIONS = frozenset({"build", "start", "stop", "bind", "browser"})
BROWSER_CONSUMERS = frozenset({"browser", "browser-verify", "browser-close"})
RUN_ID = re.compile(r"[a-z0-9][a-z0-9-]{0,63}")


class RuntimeProtocolError(ValueError):
    """私有入口协议错误；调用方应把它视作参数错误。"""


@dataclass(frozen=True)
class RuntimeRequest:
    operation: str
    backend_dir: Path
    environment: Path
    output: Path
    write: bool
    browser_binding: Path | None = None
    run_id: str | None = None
    server: str | None = None
    credential_endpoint: str | None = None


def _unique_object(pairs: list[tuple[str, object]]) -> dict:
    result = {}
    for key, value in pairs:
        if key in result:
            raise RuntimeProtocolError(f"私有协议字段重复：{key}")
        result[key] = value
    return result


def _exact_protocol(value: object, fields: set[str]) -> dict:
    if not isinstance(value, dict) or set(value) != fields:
        raise RuntimeProtocolError("私有协议字段不完整或含未知字段")
    return value


def _protocol_path(value: object, label: str) -> Path:
    if not isinstance(value, str) or not value or not Path(value).is_absolute():
        raise RuntimeProtocolError(f"{label}必须是非空绝对路径")
    if any(character in value for character in ("\n", "\r", "\0")):
        raise RuntimeProtocolError(f"{label}不能包含换行符或 NUL")
    return Path(value)


def private_protocol_request(argv: list[str] | None = None) -> RuntimeRequest:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) != 2 or arguments[0] != "--protocol-json":
        raise RuntimeProtocolError("夹具运行时只接受 --protocol-json <JSON> 参数")
    raw = arguments[1]
    if not isinstance(raw, str) or not raw or len(raw) > 32768:
        raise RuntimeProtocolError("夹具运行时私有协议缺失、为空或过长")
    if any(character in raw for character in ("\n", "\r", "\0")):
        raise RuntimeProtocolError("夹具运行时私有协议不能包含换行符或 NUL")
    try:
        protocol = json.loads(raw, object_pairs_hook=_unique_object)
    except RuntimeProtocolError:
        raise
    except (TypeError, json.JSONDecodeError) as error:
        raise RuntimeProtocolError("夹具运行时私有协议不是有效 JSON") from error
    return _request_from_protocol(protocol)


def _request_from_protocol(protocol: object) -> RuntimeRequest:
    if not isinstance(protocol, dict) or type(protocol.get("format_version")) is not int \
            or protocol.get("format_version") != 1:
        raise RuntimeProtocolError("夹具运行时私有协议版本无效")
    operation = protocol.get("operation")
    if not isinstance(operation, str) or operation not in OPERATIONS:
        raise RuntimeProtocolError("夹具运行时私有协议操作无效")
    fields = {"backend_dir", "environment", "format_version", "operation", "output", "write"}
    if operation == "bind":
        fields.update({"browser_binding", "run_id", "server"})
    elif operation in BROWSER_CONSUMERS:
        fields.add("browser_binding")
        if operation == "browser":
            fields.add("credential_endpoint")
    values = _exact_protocol(protocol, fields)
    write = values["write"]
    if type(write) is not bool or write != (operation in WRITING_OPERATIONS):
        raise RuntimeProtocolError("夹具运行时私有协议的写入授权与操作不一致")
    request = RuntimeRequest(
        operation=operation,
        backend_dir=_protocol_path(values["backend_dir"], "后端目录"),
        environment=_protocol_path(values["environment"], "环境收据"),
        output=_protocol_path(values["output"], "运行目录"),
        write=write,
        browser_binding=(_protocol_path(values["browser_binding"], "浏览器绑定")
                         if "browser_binding" in values else None),
        run_id=values.get("run_id"),
        server=values.get("server"),
        credential_endpoint=values.get("credential_endpoint"),
    )
    _validate_browser_request(request)
    return request


def _validate_browser_request(request: RuntimeRequest) -> None:
    if request.browser_binding is not None and request.browser_binding.parent != request.output:
        raise RuntimeProtocolError("浏览器绑定必须直接位于本次运行目录")
    if request.operation == "bind":
        if not isinstance(request.run_id, str) or RUN_ID.fullmatch(request.run_id) is None:
            raise RuntimeProtocolError("业务 crate 浏览器 run id 无效")
        if request.server not in {"dev", "preview"}:
            raise RuntimeProtocolError("业务 crate 浏览器 server 无效")
        if request.browser_binding.name != f"browser-binding-{request.run_id}.json":
            raise RuntimeProtocolError("浏览器绑定文件名与 run id 不一致")
    elif request.run_id is not None or request.server is not None:
        raise RuntimeProtocolError("非绑定操作不得包含 run id 或 server")
    if request.operation == "browser":
        value = request.credential_endpoint
        if not isinstance(value, str):
            raise RuntimeProtocolError("浏览器操作缺少有效的本机凭据端点")
        parsed = urlsplit(value)
        if (parsed.scheme != "http" or parsed.hostname != "127.0.0.1"
                or parsed.port is None or parsed.path != "/v1/credentials"
                or parsed.username is not None or parsed.password is not None
                or parsed.query or parsed.fragment
                or any(character in value for character in ("\n", "\r", "\0"))):
            raise RuntimeProtocolError("浏览器操作缺少有效的本机凭据端点")
    elif request.credential_endpoint is not None:
        raise RuntimeProtocolError("非浏览器执行操作不得包含凭据端点")


def execute(request: RuntimeRequest) -> dict:
    backend = request.backend_dir.resolve(strict=True)
    if request.operation == "build":
        return build(backend, request.environment, request.output)
    if request.operation == "verify":
        return verify(backend, request.environment, request.output)
    if request.operation in {"bind", "browser", "browser-verify", "browser-close"}:
        return _native_browser(request)
    return run(backend, request.environment, request.output, request.operation)


def _native_browser(request: RuntimeRequest) -> dict:
    from dataclasses import asdict
    from fixture_service_client import _request

    arguments = {key: str(value) if isinstance(value, Path) else value
                 for key, value in asdict(request).items() if value is not None}
    arguments["format_version"] = 1
    result = _request("__fixture-browser", json.dumps({
        "operation": "execute", "arguments": arguments, "write": request.write,
    }, ensure_ascii=False).encode("utf-8"), 6000, "invalid_fixture_browser")
    return result["value"]


def main(request: RuntimeRequest) -> None:
    print(json.dumps(execute(request), ensure_ascii=False))


if __name__ == "__main__":
    try:
        request = private_protocol_request()
        main(request)
    except RuntimeProtocolError:
        print("reference_fixture_runtime_protocol_error：夹具运行时私有协议无效。",
              file=sys.stderr)
        raise SystemExit(2)
    except Exception as error:
        from restore_reference_io import redact_object_diagnostic

        detail = type(error).__name__
        if isinstance(error, ValueError):
            detail += "：" + redact_object_diagnostic(str(error), os.environ)
        print(f"reference_fixture_runtime_failed：{detail}；请核对已登记证据，不自动重放。",
              file=sys.stderr)
        raise SystemExit(1)
