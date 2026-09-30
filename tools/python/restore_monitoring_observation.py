"""核验真实指标、Prometheus 状态及 Alertmanager 的隔离投递闭环。"""

from __future__ import annotations

import json
import os
import re
import subprocess
import time
import urllib.error
import urllib.request
import uuid
from datetime import datetime, timedelta, timezone
from pathlib import Path

from restore_build import validate_new_output
from restore_monitoring_evidence import descriptor
from restore_monitoring_rules import ALERTS
from restore_runtime_evidence import decode_object

MAX_HTTP_BODY = 4 * 1024 * 1024
METRIC_NAME = re.compile(rb"^[a-zA-Z_:][a-zA-Z0-9_:]*")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *_args):
        return None


def _opener():
    return urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())


def request(
    url: str,
    *,
    headers: dict[str, str] | None = None,
    body: bytes | None = None,
    timeout: float = 5,
) -> dict:
    value = urllib.request.Request(url, data=body, headers=headers or {}, method="POST" if body is not None else "GET")
    try:
        response = _opener().open(value, timeout=timeout)
    except urllib.error.HTTPError as error:
        response = error
    with response:
        raw = response.read(MAX_HTTP_BODY + 1)
        if len(raw) > MAX_HTTP_BODY:
            raise ValueError("监控 HTTP 响应超过 4 MiB")
        return {
            "status": response.status,
            "content_type": response.headers.get("Content-Type", ""),
            "body": raw,
        }


def _write(path: Path, content: bytes, backend: Path) -> dict:
    path = validate_new_output(path, backend)
    with path.open("xb") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    return descriptor(path)


def _metric_facts(raw: bytes) -> tuple[int, int]:
    samples, ryframe = 0, 0
    for line in raw.splitlines():
        line = line.strip()
        if not line or line.startswith(b"#"):
            continue
        match = METRIC_NAME.match(line)
        if match is None:
            raise ValueError("产品指标不是有效 Prometheus 文本")
        samples += 1
        ryframe += int(match.group().startswith(b"ryframe_"))
    if samples == 0 or ryframe == 0:
        raise ValueError("产品指标没有真实 RyFrame 样本")
    return samples, ryframe


def verify_metrics(
    backend: Path,
    binding: dict,
    secret: str,
    evidence: Path,
    *,
    fetch=request,
) -> dict:
    result = {}
    endpoints = binding["authority"]["endpoints"]
    for role, endpoint_name in (("api", "api_metrics"), ("worker", "worker_metrics")):
        url = endpoints[endpoint_name]
        missing = fetch(url)
        invalid = fetch(url, headers={"Authorization": "Bearer ryframe-invalid-probe"})
        accepted = fetch(url, headers={"Authorization": "Bearer " + secret})
        if missing["status"] != 401 or invalid["status"] != 401 or accepted["status"] != 200:
            raise ValueError(f"{role} 指标 Bearer 鉴权未按固定合同执行")
        if accepted["content_type"].partition(";")[0].strip().lower() != "text/plain":
            raise ValueError(f"{role} 指标媒体类型无效")
        samples, ryframe = _metric_facts(accepted["body"])
        artifact = _write(evidence / f"{role}-metrics.txt", accepted["body"], backend)
        result[role] = {
            "artifact": artifact,
            "missing_status": missing["status"],
            "invalid_status": invalid["status"],
            "accepted_status": accepted["status"],
            "sample_count": samples,
            "ryframe_sample_count": ryframe,
        }
    return result


def _json_response(response: dict, label: str) -> dict:
    if response["status"] != 200 or len(response["body"]) > 1024 * 1024:
        raise ValueError(f"{label} API 状态无效")
    return decode_object(response["body"], label)


def _wait_json(url: str, validator, *, fetch=request, timeout: float = 30) -> tuple[bytes, object]:
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        response = fetch(url)
        try:
            value = _json_response(response, "监控状态")
            return response["body"], validator(value)
        except (KeyError, TypeError, ValueError) as error:
            last = error
        time.sleep(0.1)
    raise TimeoutError("监控状态未在期限内满足绑定") from last


def _targets(binding: dict, value: dict) -> dict:
    if value.get("status") != "success" or not isinstance(value.get("data"), dict):
        raise ValueError("Prometheus target API 返回失败")
    active = value["data"].get("activeTargets")
    if not isinstance(active, list):
        raise ValueError("Prometheus target API 缺少活动目标")
    expected = {
        "ryframe-api": binding["authority"]["endpoints"]["api_metrics"],
        "ryframe-worker": binding["authority"]["endpoints"]["worker_metrics"],
    }
    observed = {}
    for item in active:
        labels = item.get("labels") if isinstance(item, dict) else None
        job = labels.get("job") if isinstance(labels, dict) else None
        if job not in expected:
            continue
        if job in observed or item.get("health") != "up" or item.get("scrapeUrl") != expected[job] or item.get("lastError"):
            raise ValueError("Prometheus target 与真实 API/Worker 指标端点不同或未就绪")
        observed[job] = {"health": "up", "scrape_url": item["scrapeUrl"]}
    if set(observed) != set(expected):
        raise ValueError("Prometheus 没有同时采集真实 API 与 Worker")
    return observed


def _rules(value: dict) -> list[str]:
    if value.get("status") != "success" or not isinstance(value.get("data"), dict):
        raise ValueError("Prometheus rule API 返回失败")
    groups = value["data"].get("groups")
    if not isinstance(groups, list):
        raise ValueError("Prometheus rule API 缺少规则组")
    names = []
    for group in groups:
        for rule in group.get("rules", []) if isinstance(group, dict) else []:
            if isinstance(rule, dict) and rule.get("name") in ALERTS:
                names.append(rule["name"])
    if len(names) != len(ALERTS) or set(names) != ALERTS:
        raise ValueError("Prometheus 没有各加载一次固定的 8 个恢复告警")
    return sorted(names)


def verify_prometheus(backend: Path, binding: dict, evidence: Path, *, fetch=request) -> dict:
    base = binding["endpoints"]["prometheus"]
    targets_raw, targets = _wait_json(base + "/api/v1/targets", lambda value: _targets(binding, value), fetch=fetch)
    rules_raw, names = _wait_json(base + "/api/v1/rules?type=alert", _rules, fetch=fetch)
    return {
        "targets": targets,
        "targets_artifact": _write(evidence / "prometheus-targets.json", targets_raw, backend),
        "alerts": names,
        "rules_artifact": _write(evidence / "prometheus-rules.json", rules_raw, backend),
    }


def verify_boundaries(
    backend: Path,
    execution: dict,
    paths: dict[str, Path],
    environment: dict[str, str],
    *,
    run=subprocess.run,
) -> dict:
    log = paths["evidence"] / "promtool-boundaries.log"
    command = [str(execution["tools"]["promtool"]), "test", "rules", str(paths["rule-tests"])]
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
        raise ValueError("promtool 23/24 小时边界复核失败")
    return {"passed": True, "cases": ["23h", "23h30s", "24h30s"], "artifact": descriptor(log)}


def _alert_body(binding: dict, probe: str, started_at: datetime, *, resolved: bool) -> bytes:
    now = datetime.now(timezone.utc)
    end = now - timedelta(seconds=1) if resolved else now + timedelta(minutes=10)
    value = [
        {
            "labels": {
                "alertname": "RyFrameMonitoringDeliveryProbe",
                "service": "ryframe",
                "scope_id": binding["scope_id"],
                "probe_id": probe,
            },
            "annotations": {"summary": "RyFrame 隔离监控投递验收"},
            "startsAt": started_at.isoformat().replace("+00:00", "Z"),
            "endsAt": end.isoformat().replace("+00:00", "Z"),
            "generatorURL": binding["endpoints"]["prometheus"],
        }
    ]
    return json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode("utf-8")


def _probe_event(value: dict, binding: dict, probe: str, started_at: str) -> str | None:
    payload = value.get("payload")
    if not isinstance(payload, dict):
        return None
    alerts = payload.get("alerts")
    if not isinstance(alerts, list):
        return None
    candidates = [
        item for item in alerts
        if isinstance(item, dict)
        and isinstance(item.get("labels"), dict)
        and item["labels"].get("alertname") == "RyFrameMonitoringDeliveryProbe"
    ]
    if not candidates:
        return None
    expected_labels = {
            "alertname": "RyFrameMonitoringDeliveryProbe",
            "service": "ryframe",
            "scope_id": binding["scope_id"],
            "probe_id": probe,
    }
    item = candidates[0]
    status = payload.get("status")
    if (
        value.get("remote") not in {"127.0.0.1", "::1"}
        or payload.get("receiver") != "ryframe-local-webhook"
        or len(alerts) != 1
        or len(candidates) != 1
        or status not in {"firing", "resolved"}
        or item.get("status") != status
        or item.get("labels") != expected_labels
        or item.get("annotations") != {"summary": "RyFrame 隔离监控投递验收"}
        or item.get("startsAt") != started_at
        or item.get("generatorURL") != binding["endpoints"]["prometheus"]
    ):
        raise ValueError("隔离 webhook 的 probe 投递形状、接收器或身份无效")
    return status


def _events(path: Path) -> list[dict]:
    raw = path.read_bytes()
    if len(raw) > MAX_HTTP_BODY:
        raise ValueError("隔离 webhook 事件超过 4 MiB")
    result = []
    for line in raw.splitlines():
        if line:
            result.append(decode_object(line, "隔离 webhook 事件"))
    return result


def _wait_event(
    path: Path,
    binding: dict,
    probe: str,
    started_at: str,
    status: str,
    timeout: float,
) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if any(_probe_event(item, binding, probe, started_at) == status for item in _events(path)):
            return
        time.sleep(0.1)
    raise TimeoutError(f"Alertmanager 没有向隔离 webhook 投递 {status}")


def verify_delivery(
    binding: dict,
    events: Path,
    *,
    fetch=request,
    timeout: float = 30,
) -> dict:
    probe = uuid.uuid4().hex
    started_at = datetime.now(timezone.utc)
    started_text = started_at.isoformat().replace("+00:00", "Z")
    endpoint = binding["endpoints"]["alertmanager"] + "/api/v2/alerts"
    headers = {"Content-Type": "application/json"}
    firing = fetch(
        endpoint,
        headers=headers,
        body=_alert_body(binding, probe, started_at, resolved=False),
    )
    if firing["status"] != 200:
        raise ValueError("Alertmanager 拒绝隔离 firing 告警")
    _wait_event(events, binding, probe, started_text, "firing", timeout)
    resolved = fetch(
        endpoint,
        headers=headers,
        body=_alert_body(binding, probe, started_at, resolved=True),
    )
    if resolved["status"] != 200:
        raise ValueError("Alertmanager 拒绝隔离 resolved 告警")
    _wait_event(events, binding, probe, started_text, "resolved", timeout)
    first = events.read_bytes()
    time.sleep(0.1)
    second = events.read_bytes()
    if first != second:
        raise ValueError("隔离 webhook 事件在证据绑定时仍在变化")
    delivered = [
        status
        for event in _events(events)
        if (status := _probe_event(event, binding, probe, started_text)) is not None
    ]
    if delivered != ["firing", "resolved"]:
        raise ValueError("隔离 webhook 必须各接收一次 firing 与 resolved probe")
    return {
        "probe_id": probe,
        "firing": True,
        "resolved": True,
        "receiver": binding["endpoints"]["webhook"] + "/alerts",
        "events": descriptor(events),
    }
