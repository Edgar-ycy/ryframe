"""将 Device 参考夹具运行时绑定到前端真实构建与浏览器验收。"""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re
import shutil
import sys
from typing import Callable
from urllib.parse import urlsplit

from devex_clone_capture import read_json
from devex_clone_factory_context import configured
from devex_clone_model import linked, local_path
from devex_clone_source_proof import require_closed_port
from full_stack_provenance import verify_build_evidence
from full_stack_rate_limit_config import rate_limit_settings
from reference_fixture_browser_evidence import (
    ArtifactManifestSnapshot,
    artifact_manifest_snapshot,
    device_tests,
    login_budget,
    verify_artifact_manifest,
)
from reference_fixture_browser_review import verify_browser_result
from reference_fixture_browser_process import (
    failure_process as _failure_process,
    run_frontend_command as _frontend_command,
)
from reference_fixture_browser_security import browser_environment, secret_values
from restore_build import file_digest
from restore_frontend_build import validate_frontend_build
from restore_input_plan import _publish_json
from restore_runtime_evidence import artifact_snapshot, file_state
from source_inventory import capture_inventory, source_file


@dataclass(frozen=True)
class RuntimeApi:
    bootstrap: Callable
    output: Callable
    verify: Callable
    control: Callable
    observe: Callable


@dataclass(frozen=True)
class SourceGuard:
    root: Path
    expected: dict
    inventory: dict
    states: tuple[tuple[str, tuple[int, int, int, int]], ...]

    @classmethod
    def capture(cls, root: Path, expected: dict) -> "SourceGuard":
        inventory = capture_inventory(root)
        snapshot = {key: value for key, value in inventory["source"]["snapshot"].items()
                    if key != "clean"}
        if snapshot != expected:
            raise ValueError("Device 浏览器源码与已登记生成来源不一致")
        states = []
        for item in inventory["files"]:
            found = source_file(root, item["path"])
            assert found is not None
            states.append((item["path"], file_state(found[1])))
        return cls(root, expected, inventory, tuple(states))

    def assert_unchanged(self) -> None:
        for relative, expected in self.states:
            found = source_file(self.root, relative, allow_missing=True)
            if found is None or file_state(found[1]) != expected:
                raise ValueError("Device 浏览器期间源码曾被替换或修改")
        if capture_inventory(self.root) != self.inventory:
            raise ValueError("Device 浏览器期间源码发生变化")


def _bound(path: Path) -> dict:
    return {"path": str(path), **file_digest(path)}


def _binding_path(execution: Path, runtime: Path, value: Path, *, new: bool,
                  run_id: str | None = None) -> Path:
    path = value.resolve() if value.is_absolute() else (execution / value).resolve()
    expected = f"browser-binding-{run_id}.json" if run_id is not None else path.name
    if (path.parent != runtime or path.name != expected or linked(path)
            or new and path.exists() or not new and (not path.is_file() or linked(path))):
        raise ValueError("Device 浏览器绑定必须是源运行时目录中的明确文件")
    return path


def _tool(path: Path, label: str) -> dict:
    resolved = path.resolve(strict=True)
    if linked(resolved) or not resolved.is_file():
        raise ValueError(f"{label}不是受控普通文件")
    return {"path": str(resolved), **file_digest(resolved)}


def _review(backend: Path, bootstrap_file: Path) -> tuple[dict, dict, dict]:
    bootstrap = read_json(bootstrap_file)
    plan = bootstrap.get("plan")
    binding = plan.get("review") if isinstance(plan, dict) else None
    side = plan.get("side") if isinstance(plan, dict) else None
    if not isinstance(binding, dict) or side not in {"seed", "base", "candidate"}:
        raise ValueError("Device 浏览器环境缺少审阅计划绑定")
    requested = Path(binding.get("path", ""))
    review_file = local_path(backend, str(requested if requested.is_absolute() else backend / requested))
    if linked(review_file) or not review_file.is_file():
        raise ValueError("Device 浏览器审阅计划必须是受控普通文件")
    review = read_json(review_file)
    if _bound(review_file) != {key: binding.get(key) for key in ("path", "bytes", "sha256")}:
        raise ValueError("Device 浏览器审阅计划摘要已变化")
    selected = review.get("scopes", {}).get(side)
    if not isinstance(selected, dict):
        raise ValueError("Device 浏览器审阅计划缺少当前侧")
    return review, selected, _bound(review_file)


def _frontend_source(build: dict) -> tuple[Path, dict]:
    source = build.get("source")
    roots = source.get("roots") if isinstance(source, dict) else None
    originals = source.get("original") if isinstance(source, dict) else None
    generated = source.get("generated") if isinstance(source, dict) else None
    if (source.get("fixture") if isinstance(source, dict) else None) != "device" \
            or not isinstance(roots, dict) or not isinstance(originals, dict) \
            or not isinstance(generated, dict) or originals.get("frontend") is None \
            or generated.get("frontend") is None:
        raise ValueError("Device 浏览器构建来源缺少完整前端工作树")
    frontend = Path(roots.get("frontend", "")).resolve(strict=True)
    if linked(frontend) or not (frontend / "package.json").is_file():
        raise ValueError("Device 浏览器前端工作树无效")
    return frontend, {"original": originals["frontend"], "generated": generated["frontend"]}


def browser_outputs(frontend: Path, runtime: Path, run_id: str, server: str) -> dict[str, Path]:
    prefix = f"browser-{run_id}"
    outputs = {
        "login_budget": runtime / f"{prefix}-login-budget.json",
        "intent": runtime / f"{prefix}-intent.json",
        "result": runtime / f"{prefix}-result.json",
        "failure": runtime / f"{prefix}-failure.json",
        "browser_log": runtime / f"{prefix}-check.log",
        "browser_process": runtime / f"{prefix}-check-process",
        "report": frontend / f".local-tests/playwright-real/report/device/{server}/{run_id}",
        "results": frontend / f".local-tests/playwright-real/results/device/{server}/{run_id}",
    }
    if server == "preview":
        outputs.update({
            "build_log": runtime / f"{prefix}-build.log",
            "build_process": runtime / f"{prefix}-build-process",
            "build_verify_before_log": runtime / f"{prefix}-build-verify-before.log",
            "build_verify_before_process": runtime / f"{prefix}-build-verify-before-process",
            "build_verify_after_log": runtime / f"{prefix}-build-verify-after.log",
            "build_verify_after_process": runtime / f"{prefix}-build-verify-after-process",
            "build_receipt": frontend / "dist/.vite/restore-build.json",
        })
    return outputs


def _assert_output_set(runtime: Path, run_id: str, outputs: dict[str, Path], *, fresh: bool) -> None:
    expected = {path for path in outputs.values() if path.parent == runtime}
    actual = set(runtime.glob(f"browser-{run_id}-*"))
    if actual - expected:
        raise ValueError("Device 浏览器运行目录包含未登记的同 run id 产物")
    if fresh and (actual or any(path.exists() or linked(path) for path in outputs.values())):
        raise ValueError("Device 浏览器 run id 已存在产物，禁止重放")


def _commands(frontend: Path, server: str) -> list[list[str]]:
    browser = ["check", "--stage", "browser", "--real", "--fixture", "device", "--server", server]
    if server == "dev":
        return [browser]
    verify = ["exec", "node", "scripts/restore-build.mjs", "verify", "--source-root", str(frontend)]
    return [["build", "--real"], verify, browser, verify]


def _input_guards(binding: dict) -> tuple:
    descriptors = {}

    def collect(value: object) -> None:
        if isinstance(value, dict):
            if set(value) == {"path", "bytes", "sha256"} and isinstance(value["path"], str):
                descriptors[value["path"]] = value
            else:
                for item in value.values():
                    collect(item)
        elif isinstance(value, list):
            for item in value:
                collect(item)

    collect(binding)
    guards = tuple(artifact_snapshot(Path(path)) for path in sorted(descriptors))
    if any(guard.descriptor() != descriptors[str(guard.path)] for guard in guards):
        raise ValueError("Device 浏览器输入文件与绑定摘要不一致")
    return guards


def _plan(api: RuntimeApi, backend: Path, environment_path: Path, output_path: Path,
          run_id: str, server: str, *, require_fresh: bool,
          process_state: str = "running", read_only: bool = False) -> tuple[dict, dict]:
    if re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", run_id) is None:
        raise ValueError("Device 浏览器 run id 必须是小写字母、数字或连字符")
    if server not in {"dev", "preview"}:
        raise ValueError("Device 浏览器 server 必须是 dev 或 preview")
    if process_state not in {"running", "stopped"}:
        raise ValueError("Device 浏览器运行时状态无效")
    bootstrap_file, execution, private = api.bootstrap(backend, environment_path)
    runtime = api.output(execution, output_path, new=False)
    verified = api.verify(backend, environment_path, runtime)
    status = (api.observe if read_only else api.control)(
        backend, environment_path, runtime, "status"
    )
    expected_processes = {"api": process_state, "worker": process_state}
    if status["processes"] != expected_processes:
        raise ValueError(f"Device 浏览器核验要求 API 与 Worker 均为 {process_state}")
    build = verify_build_evidence(execution, runtime)
    frontend, frontend_source = _frontend_source(build)
    review, selected, review_binding = _review(backend, bootstrap_file)
    api_url = f"http://{private['APP_APP_HOST']}:{private['APP_APP_PORT']}"
    frontend_url = selected.get("frontend_url")
    parsed = urlsplit(frontend_url if isinstance(frontend_url, str) else "")
    if (selected.get("scope_id") != private.get("APP_SCOPE_ID") or selected.get("api_url") != api_url
            or parsed.scheme != "http" or parsed.hostname not in {"127.0.0.1", "::1"}
            or parsed.port is None or parsed.path not in {"", "/"} or parsed.query or parsed.fragment
            or private.get("APP_CORS_ALLOW_ORIGINS") != frontend_url):
        raise ValueError("Device 浏览器端点、scope 或 CORS 与已审阅环境不一致")
    require_closed_port(frontend_url)
    mysql = review.get("tools", {}).get("mysql")
    if not isinstance(mysql, dict) or not isinstance(mysql.get("path"), str):
        raise ValueError("Device 浏览器审阅计划缺少 MySQL 客户端")
    mysql_tool = _tool(Path(mysql["path"]), "MySQL 客户端")
    if mysql_tool["sha256"] != mysql.get("sha256"):
        raise ValueError("Device 浏览器 MySQL 客户端摘要已变化")
    safe = configured({})
    corepack_path = shutil.which("corepack", path=safe.get("PATH"))
    launcher_path = safe.get("COMSPEC") if os.name == "nt" else corepack_path
    if corepack_path is None or not launcher_path:
        raise ValueError("本机缺少 Corepack 或安全命令启动器")
    limits = rate_limit_settings(execution, private)
    outputs = browser_outputs(frontend, runtime, run_id, server)
    if require_fresh and server == "preview" and (frontend / "dist").exists():
        raise ValueError("Device 前端已有生产产物，禁止覆盖未知构建")
    _assert_output_set(runtime, run_id, outputs, fresh=require_fresh)
    binding = {
        "format_version": 1, "kind": "reference-fixture-browser-binding", "status": "bound",
        "run_id": run_id, "server": server, "scope_id": private["APP_SCOPE_ID"],
        "bootstrap": _bound(bootstrap_file),
        "environment": _bound(bootstrap_file.parent / "environment.json"),
        "review": review_binding, "runtime": verified["runtime"],
        "source_pair": _bound(runtime / "source-pair.json"),
        "backend_build": _bound(runtime / "backend-build.json"),
        "backend": {"path": str(execution), "source": build["source"]["generated"]["backend"]},
        "frontend": {"path": str(frontend), "source": frontend_source},
        "tools": {"python": _tool(Path(sys.executable), "Python"), "mysql": mysql_tool,
                  "corepack": _tool(Path(corepack_path), "Corepack"),
                  "launcher": _tool(Path(launcher_path), "命令启动器")},
        "endpoints": {"api": api_url, "frontend": frontend_url},
        "identity": {"tenant_id": "system", "username": "admin",
                     "password_env": "RYFRAME_RESET_ADMIN_PASSWORD"},
        "rate_limits": {"request": {"capacity": limits["capacity"],
                                    "window_secs": limits["window_secs"]},
                        "login": {"capacity": limits["api_limits"].get(
                            "POST /api/v1/auth/login", 5), "window_secs": limits["api_window_secs"]}},
        "login_budget": {"path": str(outputs["login_budget"]), "initial_state": "absent",
                         "first_writer": "frontend-real-browser-login"},
        "commands": _commands(frontend, server),
        "remote_writes": 0,
    }
    if not private.get(binding["identity"]["password_env"]):
        raise ValueError("Device 浏览器环境缺少已登记管理员凭据")
    source_guards = (
        SourceGuard.capture(execution, binding["backend"]["source"]),
        SourceGuard.capture(frontend, frontend_source["generated"]),
    )
    bootstrap = read_json(bootstrap_file)
    return binding, {"private": private, "frontend": frontend, "runtime": runtime,
                     "outputs": outputs, "source_guards": source_guards,
                     "input_guards": _input_guards(binding),
                     "secrets": secret_values(private, bootstrap)}


def _assert_guards(context: dict) -> None:
    for guard in (*context["source_guards"], *context["input_guards"]):
        guard.assert_unchanged()


def bind_browser(api: RuntimeApi, backend: Path, environment_path: Path, output_path: Path,
                 binding_path: Path, run_id: str, server: str) -> dict:
    _, execution, _ = api.bootstrap(backend, environment_path)
    runtime = api.output(execution, output_path, new=False)
    target = _binding_path(execution, runtime, binding_path, new=True, run_id=run_id)
    binding, context = _plan(
        api, backend, environment_path, output_path, run_id, server, require_fresh=True
    )
    _assert_guards(context)
    _publish_json(target, binding)
    _assert_guards(context)
    repeated, _ = _plan(
        api, backend, environment_path, output_path, run_id, server, require_fresh=True
    )
    if read_json(target) != binding or repeated != binding:
        raise ValueError("Device 浏览器绑定发布后来源发生变化；保留文件且禁止重放")
    return {"status": "reference_fixture_browser_bound", "binding": _bound(target),
            "run_id": run_id, "server": server, "remote_writes": 0}


def _build_environment(context: dict, binding: dict) -> dict:
    return configured({"COREPACK_ENABLE_NETWORK": "0", "VITE_APP_API_ORIGIN": "",
                       "VITE_APP_PROXY_TARGET": binding["endpoints"]["api"],
                       "TEMP": context["private"]["TEMP"], "TMP": context["private"]["TMP"]})


def _preview_build(binding: dict, context: dict,
                   processes: dict) -> tuple[dict, ArtifactManifestSnapshot]:
    outputs, frontend = context["outputs"], context["frontend"]
    environment = _build_environment(context, binding)
    processes["build"] = _frontend_command(
        binding, frontend, binding["commands"][0], environment,
        outputs["build_log"], outputs["build_process"], 1800, (),
    )
    processes["build_verify_before"] = _frontend_command(
        binding, frontend, binding["commands"][1], environment, outputs["build_verify_before_log"],
        outputs["build_verify_before_process"], 300, (),
    )
    _, receipt = validate_frontend_build(frontend)
    snapshot = artifact_manifest_snapshot(frontend / "dist", frontend, "Device 前端生产产物")
    if receipt.path != outputs["build_receipt"]:
        raise ValueError("Device 前端真实构建收据路径不一致")
    return {"receipt": _bound(receipt.path), "dist": snapshot.manifest}, snapshot


def _verify_preview_after(binding: dict, context: dict, processes: dict,
                          build: dict, build_guard: ArtifactManifestSnapshot) -> None:
    outputs, frontend = context["outputs"], context["frontend"]
    processes["build_verify_after"] = _frontend_command(
        binding, frontend, binding["commands"][3], _build_environment(context, binding),
        outputs["build_verify_after_log"], outputs["build_verify_after_process"], 300, (),
    )
    build_guard.assert_unchanged()
    _, receipt = validate_frontend_build(frontend)
    if _bound(receipt.path) != build["receipt"]:
        raise ValueError("Device 浏览器期间生产构建收据发生变化")
    verify_artifact_manifest(build["dist"], frontend / "dist", frontend, "Device 前端生产产物")


def _browser_artifacts(binding: dict, context: dict) -> tuple[dict, tuple[ArtifactManifestSnapshot, ...]]:
    outputs, frontend = context["outputs"], context["frontend"]
    report, results = outputs["report"], outputs["results"]
    sidecar = results / "device-tests.json"
    if not (report / "index.html").is_file():
        raise ValueError("Device 浏览器报告缺少首页")
    tests = device_tests(sidecar, binding["server"], binding["run_id"])
    report_snapshot = artifact_manifest_snapshot(
        report, frontend / ".local-tests/playwright-real/report", "Device 浏览器 HTML 报告"
    )
    results_snapshot = artifact_manifest_snapshot(
        results, frontend / ".local-tests/playwright-real/results", "Device 浏览器结果"
    )
    report_manifest, results_manifest = report_snapshot.manifest, results_snapshot.manifest
    if not any(item["path"] == "index.html" for item in report_manifest["files"]) \
            or not any(item["path"] == "device-tests.json" for item in results_manifest["files"]):
        raise ValueError("Device 浏览器完整清单缺少报告首页或场景收据")
    return ({"report": report_manifest, "results": results_manifest, "tests": tests},
            (report_snapshot, results_snapshot))

def _run_logs(outputs: dict, server: str) -> dict:
    keys = ["browser"] if server == "dev" else [
        "build", "build_verify_before", "browser", "build_verify_after"
    ]
    return {name: _bound(outputs[name + "_log"]) for name in keys}


def _failure_evidence(outputs: dict, server: str, scope_id: str,
                      completed: dict) -> tuple[dict, dict]:
    keys = ["browser"] if server == "dev" else [
        "build", "build_verify_before", "browser", "build_verify_after"
    ]
    processes = dict(completed)
    logs = {}
    for name in keys:
        process_dir = outputs[name + "_process"]
        if name not in processes and process_dir.is_dir():
            processes[name] = _failure_process(process_dir, scope_id)
        log = outputs[name + "_log"]
        if log.is_file():
            logs[name] = _bound(log)
    return processes, logs


def run_browser(api: RuntimeApi, backend: Path, environment_path: Path, output_path: Path,
                binding_path: Path) -> dict:
    _, execution, _ = api.bootstrap(backend, environment_path)
    runtime = api.output(execution, output_path, new=False)
    path = _binding_path(execution, runtime, binding_path, new=False)
    binding = read_json(path)
    run_id = binding.get("run_id") if isinstance(binding, dict) else None
    server = binding.get("server") if isinstance(binding, dict) else None
    if not isinstance(run_id, str) or not isinstance(server, str):
        raise ValueError("Device 浏览器绑定缺少 run id 或 server")
    _binding_path(execution, runtime, path, new=False, run_id=run_id)
    expected, context = _plan(
        api, backend, environment_path, output_path, run_id, server, require_fresh=True
    )
    if binding != expected:
        raise ValueError("Device 浏览器绑定与当前源码、运行时或环境不一致")
    binding_guard = artifact_snapshot(path)
    _assert_guards(context)
    outputs = context["outputs"]
    intent = {"format_version": 1, "kind": "reference-fixture-browser-intent",
              "binding": _bound(path), "commands": binding["commands"],
              "login_budget": {"path": str(outputs["login_budget"]), "existed_before": False}}
    _publish_json(outputs["intent"], intent)
    stage, business_started = ("build", False) if server == "preview" else ("browser", True)
    processes = {}
    secrets = context["secrets"]
    try:
        if server == "preview":
            build, build_guard = _preview_build(binding, context, processes)
        else:
            build, build_guard = None, None
        _assert_guards(context)
        repeated, _ = _plan(
            api, backend, environment_path, output_path, run_id, server, require_fresh=False
        )
        if repeated != binding:
            raise ValueError("Device 前端构建后来源或运行端点发生变化")
        binding_guard.assert_unchanged()
        stage, business_started = "browser", True
        if outputs["login_budget"].exists():
            raise ValueError("Device 登录预算在浏览器首次写入前已经出现")
        environment = browser_environment(context["private"], binding)
        processes["browser"] = _frontend_command(
            binding, context["frontend"], binding["commands"][2 if server == "preview" else 0],
            environment, outputs["browser_log"],
            outputs["browser_process"], 3600, secrets,
        )
        if server == "preview":
            stage = "build_verify_after"
            _verify_preview_after(binding, context, processes, build, build_guard)
        stage = "evidence"
        _assert_guards(context)
        binding_guard.assert_unchanged()
        repeated, _ = _plan(
            api, backend, environment_path, output_path, run_id, server, require_fresh=False
        )
        if repeated != binding:
            raise ValueError("Device 浏览器结束后的来源、端点或结果不完整")
        artifacts, artifact_guards = _browser_artifacts(binding, context)
        budget = login_budget(outputs["login_budget"], binding)
        result = {"format_version": 1, "kind": "reference-fixture-browser-result", "status": "passed",
                  "run_id": run_id, "server": server, "binding": _bound(path),
                  "intent": _bound(outputs["intent"]), "build": build,
                  "logs": _run_logs(outputs, server), "processes": processes,
                  "artifacts": artifacts, "login_budget": budget,
                  "remote_writes": {"business_data": True}}
        _publish_json(outputs["result"], result)
        for guard in artifact_guards:
            guard.assert_unchanged()
        verify_browser_result(binding, context, path)
        _assert_guards(context)
        binding_guard.assert_unchanged()
        return result
    except BaseException as error:
        if outputs["result"].exists():
            error.add_note("Device 浏览器成功结果已经发布；保留并核对，未另写失败结果")
            raise
        failure = {"format_version": 1, "kind": "reference-fixture-browser-failure",
                   "status": "failed", "stage": stage, "binding": _bound(path),
                   "intent": _bound(outputs["intent"]), "error_type": type(error).__name__,
                   "returncode": getattr(error, "returncode", None),
                   "unknown_business_writes": business_started}
        failure["processes"], failure["logs"] = _failure_evidence(
            outputs, server, binding["scope_id"], processes
        )
        if outputs["login_budget"].is_file():
            failure["login_budget"] = _bound(outputs["login_budget"])
        try:
            _publish_json(outputs["failure"], failure)
        except Exception as evidence_error:
            raise RuntimeError(f"Device 浏览器失败且失败证据保存失败：{evidence_error}") from error
        raise


def verify_browser(api: RuntimeApi, backend: Path, environment_path: Path, output_path: Path,
                   binding_path: Path, *, closed: bool = False) -> dict:
    _, execution, _ = api.bootstrap(backend, environment_path)
    runtime = api.output(execution, output_path, new=False)
    path = _binding_path(execution, runtime, binding_path, new=False)
    binding = read_json(path)
    run_id, server = binding.get("run_id"), binding.get("server")
    if not isinstance(run_id, str) or not isinstance(server, str):
        raise ValueError("Device 浏览器绑定缺少 run id 或 server")
    _binding_path(execution, runtime, path, new=False, run_id=run_id)
    expected, context = _plan(
        api, backend, environment_path, output_path, run_id, server,
        require_fresh=False, process_state="stopped" if closed else "running", read_only=True,
    )
    if binding != expected:
        raise ValueError("Device 浏览器绑定与当前源码、运行时或环境不一致")
    binding_guard = artifact_snapshot(path)
    _assert_guards(context)
    verify_browser_result(binding, context, path)
    _assert_guards(context)
    binding_guard.assert_unchanged()
    return {"status": "reference_fixture_browser_closed" if closed else
            "reference_fixture_browser_verified", "binding": _bound(path),
            "result": _bound(context["outputs"]["result"]), "run_id": run_id,
            "server": server, "remote_writes": 0}
