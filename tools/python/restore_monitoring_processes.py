"""准备并管理正式恢复验收使用的三棵隔离监控进程树。"""

from __future__ import annotations

import json
import os
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

from process_environment import configured
from full_stack_process_monitor import completion_binding
from full_stack_process_tree import terminate_owned_process_tree
from restore_build import validate_new_output
from restore_monitoring_evidence import descriptor, validate_binding
from restore_monitoring_staging import RESOURCES

ROLES = ("webhook", "alertmanager", "prometheus")
CONFIG_NAMES = {
    "alertmanager": "alertmanager.yml",
    "prometheus": "prometheus.yml",
    "rules": "ryframe-alerts.yml",
    "rule-tests": "ryframe-alerts.test.yml",
}
VALIDATION_NAMES = {
    "alertmanager": "validate-alertmanager.log",
    "prometheus": "validate-prometheus.log",
    "rules": "validate-rules.log",
    "boundaries": "validate-boundaries.log",
}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args):
        return None


def _opener():
    return urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


def _write(path: Path, content: bytes, backend: Path) -> dict:
    path = validate_new_output(path, backend)
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    return descriptor(path)


def _json_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def configs(binding: dict, paths: dict[str, Path]) -> dict[str, str]:
    binding = validate_binding(binding)
    product = binding["authority"]["endpoints"]
    api = urlsplit(product["api_metrics"])
    worker = urlsplit(product["worker_metrics"])
    alertmanager = urlsplit(binding["endpoints"]["alertmanager"])
    prometheus = f"""global:
  scrape_interval: 1s
  evaluation_interval: 1s
rule_files:
  - {_json_string(str(paths['rules']))}
alerting:
  alertmanagers:
    - static_configs:
        - targets: [{_json_string(f'{alertmanager.hostname}:{alertmanager.port}')}]
scrape_configs:
  - job_name: ryframe-api
    metrics_path: {_json_string(api.path)}
    authorization:
      type: Bearer
      credentials_file: {_json_string(binding['credential']['path'])}
    static_configs:
      - targets: [{_json_string(f'{api.hostname}:{api.port}')}]
  - job_name: ryframe-worker
    metrics_path: {_json_string(worker.path)}
    authorization:
      type: Bearer
      credentials_file: {_json_string(binding['credential']['path'])}
    static_configs:
      - targets: [{_json_string(f'{worker.hostname}:{worker.port}')}]
"""
    webhook = binding["endpoints"]["webhook"] + "/alerts"
    alertmanager_config = f"""global: {{}}
route:
  receiver: ryframe-local-webhook
  group_by: [alertname, probe_id]
  group_wait: 0s
  group_interval: 1s
  repeat_interval: 1m
receivers:
  - name: ryframe-local-webhook
    webhook_configs:
      - url: {_json_string(webhook)}
        send_resolved: true
"""
    return {"prometheus": prometheus, "alertmanager": alertmanager_config}


def safe_environment(run_directory: Path) -> dict[str, str]:
    environment = configured(
        {
            "NO_PROXY": "127.0.0.1,localhost,::1",
            "TEMP": str(run_directory),
            "TMP": str(run_directory),
        }
    )
    names = {name.upper() for name in environment}
    if names & {"HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"}:
        raise ValueError("监控子进程环境不能继承代理")
    if names & {"APP_MONITOR_METRICS_BEARER_TOKEN", "APP_AUTH_JWT_SECRET"}:
        raise ValueError("监控工具进程不能继承产品秘密")
    return environment


def paths(run_directory: Path) -> dict[str, Path]:
    result: dict[str, Path] = {
        name: run_directory / name
        for name in ("configs", "processes", "evidence", "prometheus-data", "alertmanager-data")
    }
    for key, name in CONFIG_NAMES.items():
        result[key] = result["configs"] / name
    for key, name in VALIDATION_NAMES.items():
        result["validate-" + key] = result["configs"] / name
    result["events"] = result["evidence"] / "webhook-events.jsonl"
    return result


def create_directories(backend: Path, run_directory: Path) -> dict[str, Path]:
    result = paths(run_directory)
    for name in ("configs", "processes", "evidence", "prometheus-data", "alertmanager-data"):
        result[name].mkdir()
    _write(result["events"], b"", backend)
    return result


def write_configs(
    backend: Path,
    binding: dict,
    execution: dict,
    paths: dict[str, Path],
) -> dict:
    resources = dict(zip(("rules", "rule-tests"), RESOURCES, strict=True))
    receipts = {
        name: _write(paths[name], (execution["root"] / relative).read_bytes(), backend)
        for name, relative in resources.items()
    }
    receipts.update(
        {
            name: _write(paths[name], content.encode("utf-8"), backend)
            for name, content in configs(binding, paths).items()
        }
    )
    return receipts


def _validation_commands(
    execution: dict,
    paths: dict[str, Path],
) -> tuple[tuple[str, list[str]], ...]:
    tools = execution["tools"]
    return (
        ("alertmanager", [str(tools["amtool"]), "check-config", str(paths["alertmanager"])]),
        ("prometheus", [str(tools["promtool"]), "check", "config", str(paths["prometheus"])]),
        ("rules", [str(tools["promtool"]), "check", "rules", str(paths["rules"])]),
        ("boundaries", [str(tools["promtool"]), "test", "rules", str(paths["rule-tests"])]),
    )


def validate_configs(
    execution: dict,
    paths: dict[str, Path],
    environment: dict[str, str],
    run=subprocess.run,
) -> dict:
    receipts = {}
    for name, command in _validation_commands(execution, paths):
        log = paths["validate-" + name]
        with log.open("xb") as output:
            completed = run(
                command,
                cwd=paths["configs"],
                env=environment,
                stdin=subprocess.DEVNULL,
                stdout=output,
                stderr=subprocess.STDOUT,
                timeout=300,
                check=False,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
            )
            output.flush()
            os.fsync(output.fileno())
        if completed.returncode != 0:
            raise ValueError("监控配置、规则或 23/24 小时边界验证失败")
        receipts[name] = descriptor(log)
    return receipts


def commands(binding: dict, execution: dict, paths: dict[str, Path]) -> dict[str, list[str]]:
    ports = binding["ports"]
    return {
        "webhook": [
            *execution["runner_command"],
            "__webhook",
            "--host",
            "127.0.0.1",
            "--port",
            str(ports["webhook"]),
            "--events",
            str(paths["events"]),
        ],
        "alertmanager": [
            str(execution["tools"]["alertmanager"]),
            f"--config.file={paths['alertmanager']}",
            f"--storage.path={paths['alertmanager-data']}",
            f"--web.listen-address=127.0.0.1:{ports['alertmanager']}",
            "--cluster.listen-address=",
        ],
        "prometheus": [
            str(execution["tools"]["prometheus"]),
            f"--config.file={paths['prometheus']}",
            f"--storage.tsdb.path={paths['prometheus-data']}",
            f"--web.listen-address=127.0.0.1:{ports['prometheus']}",
            "--storage.tsdb.retention.time=2h",
        ],
    }


def ready_endpoints(binding: dict) -> dict[str, str]:
    return {
        "webhook": binding["endpoints"]["webhook"] + "/readyz",
        "alertmanager": binding["endpoints"]["alertmanager"] + "/-/ready",
        "prometheus": binding["endpoints"]["prometheus"] + "/-/ready",
    }


def wait_ready(url: str, timeout: float = 30) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with _opener().open(url, timeout=2) as response:
                if response.status == 200 and len(response.read(64 * 1024 + 1)) <= 64 * 1024:
                    return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.1)
    raise TimeoutError("监控进程未在期限内就绪")


def stop_processes(
    processes: dict[str, object],
    *,
    crash: bool,
    terminate=terminate_owned_process_tree,
    completion=completion_binding,
) -> dict:
    receipts, failures = {}, []
    for role in reversed(ROLES):
        process = processes.get(role)
        if process is None:
            continue
        tree = process.tree if hasattr(process, "tree") else process
        try:
            terminate(tree, crash=crash)
            receipts[role] = completion(tree)
        except BaseException as error:
            failures.append((role, error))
    if failures:
        primary = failures[0][1]
        for role, error in failures[1:]:
            primary.add_note(f"监控进程 {role} 回收同时失败：{type(error).__name__}")
        raise primary
    return receipts
