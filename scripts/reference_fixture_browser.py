"""将 Device 参考夹具运行时绑定到前端真实构建与浏览器验收。"""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import threading
from typing import Callable
from urllib.parse import urlsplit

from devex_clone_capture import read_json
from devex_clone_factory_context import configured
from devex_clone_model import linked, local_path
from devex_clone_source_proof import require_closed_port
from full_stack_process_monitor import completion_binding
from full_stack_process_tree import launch_supervised_process, terminate_owned_process_tree
from full_stack_provenance import verify_build_evidence
from full_stack_rate_limit_config import rate_limit_settings
from reference_fixture_browser_security import browser_environment, secret_values
from restore_build import file_digest
from restore_input_plan import _publish_json
from restore_runtime_evidence import artifact_snapshot, file_state
from source_inventory import capture_inventory, source_file


@dataclass(frozen=True)
class RuntimeApi:
    bootstrap: Callable
    output: Callable
    verify: Callable
    control: Callable


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


def browser_outputs(frontend: Path, runtime: Path, run_id: str) -> dict[str, Path]:
    prefix = f"browser-{run_id}"
    return {
        "login_budget": runtime / f"{prefix}-login-budget.json",
        "intent": runtime / f"{prefix}-intent.json",
        "result": runtime / f"{prefix}-result.json",
        "failure": runtime / f"{prefix}-failure.json",
        "build_log": runtime / f"{prefix}-build.log",
        "browser_log": runtime / f"{prefix}-check.log",
        "build_process": runtime / f"{prefix}-build-process",
        "browser_process": runtime / f"{prefix}-check-process",
        "build_receipt": frontend / "dist/.vite/restore-build.json",
        "report": frontend / f".local-tests/playwright-real/report/device/preview/{run_id}/index.html",
        "results": frontend / f".local-tests/playwright-real/results/device/preview/{run_id}",
    }


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
          run_id: str, *, require_fresh: bool) -> tuple[dict, dict]:
    if re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", run_id) is None:
        raise ValueError("Device 浏览器 run id 必须是小写字母、数字或连字符")
    bootstrap_file, execution, private = api.bootstrap(backend, environment_path)
    runtime = api.output(execution, output_path, new=False)
    verified = api.verify(backend, environment_path, runtime)
    status = api.control(backend, environment_path, runtime, "status")
    if status["processes"] != {"api": "running", "worker": "running"}:
        raise ValueError("Device 浏览器绑定要求 API 与 Worker 均已运行")
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
    outputs = browser_outputs(frontend, runtime, run_id)
    if require_fresh and (frontend / "dist").exists():
        raise ValueError("Device 前端已有生产产物，禁止覆盖未知构建")
    if require_fresh and any(path.exists() or linked(path) for path in outputs.values()):
        raise ValueError("Device 浏览器 run id 已存在产物，禁止重放")
    binding = {
        "format_version": 1, "kind": "reference-fixture-browser-binding", "status": "bound",
        "run_id": run_id, "scope_id": private["APP_SCOPE_ID"],
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
        "commands": ["corepack pnpm build --real",
                     "corepack pnpm check --stage browser --real --fixture device --server preview"],
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
                 binding_path: Path, run_id: str) -> dict:
    _, execution, _ = api.bootstrap(backend, environment_path)
    runtime = api.output(execution, output_path, new=False)
    target = _binding_path(execution, runtime, binding_path, new=True, run_id=run_id)
    binding, context = _plan(api, backend, environment_path, output_path, run_id, require_fresh=True)
    _assert_guards(context)
    _publish_json(target, binding)
    _assert_guards(context)
    repeated, _ = _plan(api, backend, environment_path, output_path, run_id, require_fresh=True)
    if read_json(target) != binding or repeated != binding:
        raise ValueError("Device 浏览器绑定发布后来源发生变化；保留文件且禁止重放")
    return {"status": "reference_fixture_browser_bound", "binding": _bound(target),
            "run_id": run_id, "remote_writes": 0}


class OutputCapture:
    """先在内存中收集受控子进程输出，脱敏后才写入证据文件。"""

    def __init__(self, stream, maximum: int = 64 * 1024 * 1024):
        self.stream = stream
        self.maximum = maximum
        self.chunks: list[bytes] = []
        self.size = 0
        self.overflow = False
        self.error: BaseException | None = None
        self.thread = threading.Thread(target=self._read, name="device-browser-log", daemon=False)

    def _read(self) -> None:
        try:
            while block := self.stream.read(64 * 1024):
                if not self.overflow and self.size + len(block) <= self.maximum:
                    self.chunks.append(block)
                    self.size += len(block)
                else:
                    self.overflow = True
        except BaseException as error:
            self.error = error

    def start(self) -> None:
        self.thread.start()

    def finish(self, path: Path, secrets: tuple[str, ...]) -> None:
        self.thread.join(timeout=15)
        if self.thread.is_alive():
            raise TimeoutError("Device 前端日志管道未在进程树停止后关闭")
        if self.error is not None:
            raise RuntimeError("Device 前端日志读取失败") from self.error
        raw = b"".join(self.chunks)
        for secret in secrets:
            raw = raw.replace(secret.encode("utf-8"), b"[REDACTED]")
        with path.open("xb") as stream:
            stream.write(raw)
            stream.flush()
            os.fsync(stream.fileno())
        if self.overflow:
            raise ValueError("Device 前端日志超过 64 MiB，完整输出未写入证据")


def _command(binding: dict, arguments: list[str]) -> list[str]:
    corepack = binding["tools"]["corepack"]["path"]
    if os.name != "nt":
        return [corepack, "pnpm", *arguments]
    command = subprocess.list2cmdline([corepack, "pnpm", *arguments])
    return [binding["tools"]["launcher"]["path"], "/d", "/s", "/c", command]


def _process_evidence(process, directory: Path) -> dict:
    tree = directory / "frontend-tree.json"
    receipt = directory / "frontend.json"
    return {"directory": str(directory), "process": _bound(receipt), "tree": _bound(tree),
            "completion": completion_binding(process.tree)}


def _frontend_command(binding: dict, frontend: Path, arguments: list[str], environment: dict,
                      log: Path, process_dir: Path, timeout: float, secrets: tuple[str, ...]) -> dict:
    process_dir.mkdir()
    process = None
    capture = None
    error = None
    try:
        process = launch_supervised_process(
            process_dir, "frontend", binding["scope_id"], _command(binding, arguments),
            frontend, environment, subprocess.PIPE,
        )
        if process.supervisor.stdout is None:
            raise ValueError("Device 前端监督进程没有提供受控日志管道")
        capture = OutputCapture(process.supervisor.stdout)
        capture.start()
        exit_code = process.wait(timeout=timeout)
        evidence = _process_evidence(process, process_dir)
        if exit_code != 0:
            raise subprocess.CalledProcessError(exit_code, ["corepack", "pnpm", *arguments])
        return evidence
    except BaseException as caught:
        error = caught
        if process is not None:
            try:
                terminate_owned_process_tree(process.tree, crash=True)
                process.wait(timeout=10)
                completion_binding(process.tree)
            except BaseException as cleanup:
                caught.add_note("Device 前端进程树回收失败：" + type(cleanup).__name__)
        raise
    finally:
        if capture is not None:
            try:
                capture.finish(log, secrets)
            except BaseException as cleanup:
                if error is None:
                    raise
                error.add_note("Device 前端日志脱敏失败：" + type(cleanup).__name__)


def _failure_process(directory: Path) -> dict:
    result = {"directory": str(directory)}
    for name in ("frontend.json", "frontend-tree.json"):
        path = directory / name
        if path.is_file():
            result[name.removesuffix(".json")] = _bound(path)
    tree_path = directory / "frontend-tree.json"
    if tree_path.is_file():
        operation = read_json(tree_path).get("operation_id")
        for label in ("result", "stopped"):
            path = directory / f"frontend-tree-{operation}-{label}.json"
            if isinstance(operation, str) and path.is_file():
                result[label] = _bound(path)
    return result


def run_browser(api: RuntimeApi, backend: Path, environment_path: Path, output_path: Path,
                binding_path: Path) -> dict:
    _, execution, _ = api.bootstrap(backend, environment_path)
    runtime = api.output(execution, output_path, new=False)
    path = _binding_path(execution, runtime, binding_path, new=False)
    binding = read_json(path)
    run_id = binding.get("run_id") if isinstance(binding, dict) else None
    if not isinstance(run_id, str):
        raise ValueError("Device 浏览器绑定缺少 run id")
    _binding_path(execution, runtime, path, new=False, run_id=run_id)
    expected, context = _plan(api, backend, environment_path, output_path, run_id, require_fresh=True)
    if binding != expected:
        raise ValueError("Device 浏览器绑定与当前源码、运行时或环境不一致")
    binding_guard = artifact_snapshot(path)
    _assert_guards(context)
    outputs = context["outputs"]
    environment = browser_environment(context["private"], binding)
    intent = {"format_version": 1, "kind": "reference-fixture-browser-intent",
              "binding": _bound(path), "commands": binding["commands"],
              "login_budget": {"path": str(outputs["login_budget"]), "existed_before": False}}
    _publish_json(outputs["intent"], intent)
    stage, business_started = "build", False
    processes = {}
    secrets = context["secrets"]
    try:
        build_environment = configured({"COREPACK_ENABLE_NETWORK": "0",
                                        "VITE_APP_API_ORIGIN": "",
                                        "VITE_APP_PROXY_TARGET": binding["endpoints"]["api"],
                                        "TEMP": context["private"]["TEMP"],
                                        "TMP": context["private"]["TMP"]})
        processes["build"] = _frontend_command(
            binding, context["frontend"], ["build", "--real"], build_environment,
            outputs["build_log"], outputs["build_process"], 1800, (),
        )
        if not outputs["build_receipt"].is_file():
            raise ValueError("Device 前端真实构建未生成来源收据")
        _assert_guards(context)
        repeated, _ = _plan(api, backend, environment_path, output_path, run_id, require_fresh=False)
        if repeated != binding:
            raise ValueError("Device 前端构建后来源或运行端点发生变化")
        binding_guard.assert_unchanged()
        stage, business_started = "browser", True
        processes["browser"] = _frontend_command(
            binding, context["frontend"],
            ["check", "--stage", "browser", "--real", "--fixture", "device",
             "--server", "preview"], environment, outputs["browser_log"],
            outputs["browser_process"], 3600, secrets,
        )
        _assert_guards(context)
        binding_guard.assert_unchanged()
        repeated, _ = _plan(api, backend, environment_path, output_path, run_id, require_fresh=False)
        if repeated != binding or not outputs["report"].is_file() or not outputs["results"].is_dir():
            raise ValueError("Device 浏览器结束后的来源、端点或结果不完整")
        result = {"format_version": 1, "kind": "reference-fixture-browser-result", "status": "passed",
                  "binding": _bound(path), "intent": _bound(outputs["intent"]),
                  "build": _bound(outputs["build_receipt"]),
                  "build_log": _bound(outputs["build_log"]),
                  "browser_log": _bound(outputs["browser_log"]), "processes": processes,
                  "report": str(outputs["report"]), "results": str(outputs["results"]),
                  "login_budget": _bound(outputs["login_budget"]),
                  "remote_writes": {"business_data": True}}
        _publish_json(outputs["result"], result)
        return result
    except BaseException as error:
        if outputs["result"].exists():
            error.add_note("Device 浏览器成功结果已经发布；保留并核对，未另写失败结果")
            raise
        failure = {"format_version": 1, "kind": "reference-fixture-browser-failure",
                   "status": "failed", "stage": stage, "binding": _bound(path),
                   "intent": _bound(outputs["intent"]), "error_type": type(error).__name__,
                   "returncode": getattr(error, "returncode", None),
                   "unknown_business_writes": business_started,
                   "process": _failure_process(outputs[stage + "_process"])}
        log = outputs["browser_log" if stage == "browser" else "build_log"]
        if log.is_file():
            failure["log"] = _bound(log)
        if outputs["login_budget"].is_file():
            failure["login_budget"] = _bound(outputs["login_budget"])
        try:
            _publish_json(outputs["failure"], failure)
        except Exception as evidence_error:
            raise RuntimeError(f"Device 浏览器失败且失败证据保存失败：{evidence_error}") from error
        raise
