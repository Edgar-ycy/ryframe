"""为 业务 crate 浏览器子进程构造最小环境并从权威秘密文件取得脱敏值。"""

from __future__ import annotations

from pathlib import Path
from urllib.parse import unquote, urlsplit

from process_environment import configured
from full_stack_rate_limit_config import login_budget_environment


RUNTIME_NAMES = frozenset({
    "APP_ENV", "APP_SCOPE_ID", "APP_CONFIG_DIR", "APP_APP_HOST", "APP_APP_PORT",
    "APP_CORS_ALLOW_ORIGINS", "APP_DATABASE_HOST", "APP_DATABASE_PORT", "APP_DATABASE_NAME",
    "APP_DATABASE_USERNAME", "APP_DATABASE_PASSWORD", "APP_DATABASE_TLS_MODE", "APP_DB_PASSWORD",
    "APP_TENANT_DATA_TARGETS", "APP_OBJECT_STORAGE_BACKEND", "APP_OBJECT_STORAGE_ENDPOINT",
    "APP_OBJECT_STORAGE_REGION", "APP_OBJECT_STORAGE_USE_SSL", "APP_OBJECT_STORAGE_ACCESS_KEY",
    "APP_OBJECT_STORAGE_SECRET_KEY", "APP_REDIS_HOST", "APP_REDIS_PORT", "APP_REDIS_DATABASE",
    "APP_REDIS_TLS", "APP_REDIS_PASSWORD", "APP_JOBS_MODE", "APP_JOBS_HEALTH_HOST",
    "APP_JOBS_HEALTH_PORT", "APP_AUTH_JWT_SECRET", "APP_MONITOR_METRICS_BEARER_TOKEN", "TEMP", "TMP",
})
SECRET_HINTS = ("PASSWORD", "SECRET", "TOKEN", "ACCESS_KEY", "CREDENTIAL")


def secret_values(private: dict, bootstrap: dict) -> tuple[str, ...]:
    """只从已复核 bootstrap 描述和明确敏感字段派生日志脱敏集合。"""
    values = set()
    for descriptor in bootstrap["secret_files"].values():
        path = Path(descriptor["path"])
        raw = path.read_text(encoding="utf-8")
        stripped = raw.strip()
        if stripped:
            values.add(stripped)
        for line in raw.splitlines():
            if line.strip().lower().startswith("password="):
                password = line.split("=", 1)[1].strip()
                if password:
                    values.add(password)
    for key, value in private.items():
        upper = key.upper()
        if value and any(hint in upper for hint in SECRET_HINTS):
            values.add(value)
        if value and upper.endswith(("URL", "URI", "ENDPOINT")):
            parsed = urlsplit(value)
            if parsed.username is not None or parsed.password is not None:
                values.add(value)
                values.update(unquote(item) for item in (parsed.username, parsed.password) if item)
    return tuple(sorted(values, key=len, reverse=True))


def browser_environment(private: dict, binding: dict) -> dict:
    missing = sorted(name for name in RUNTIME_NAMES if not isinstance(private.get(name), str))
    if missing:
        raise ValueError("业务 crate 浏览器环境缺少运行字段：" + "、".join(missing))
    values = configured({name: private[name] for name in RUNTIME_NAMES})
    request, login = binding["rate_limits"]["request"], binding["rate_limits"]["login"]
    values.update({
        "COREPACK_ENABLE_NETWORK": "0", "PLAYWRIGHT_CHANNEL": "chrome", "PYTHONUTF8": "1",
        "APP_API_DOCS_ENABLED": "false", "RYFRAME_CODE_SHA": binding["backend"]["source"]["head"],
        "VITE_APP_PROXY_TARGET": binding["endpoints"]["api"],
        "RYFRAME_E2E_FIXTURE": "business", "RYFRAME_E2E_SERVER": binding.get("server", "preview"),
        "RYFRAME_E2E_SCOPE_ID": binding["scope_id"], "RYFRAME_E2E_TENANT_ID": "system",
        "RYFRAME_E2E_USERNAME": "admin",
        "RYFRAME_E2E_PASSWORD": private[binding["identity"]["password_env"]],
        "RYFRAME_E2E_BACKEND_DIR": binding["backend"]["path"],
        "RYFRAME_E2E_RUNTIME_DIR": str(Path(binding["runtime"]["path"]).parent),
        "RYFRAME_E2E_PYTHON": binding["tools"]["python"]["path"],
        "RYFRAME_E2E_MYSQL_CLIENT": binding["tools"]["mysql"]["path"],
        "RYFRAME_E2E_RATE_LIMIT_CAPACITY": str(request["capacity"]),
        "RYFRAME_E2E_RATE_LIMIT_WINDOW_SECS": str(request["window_secs"]),
        "RYFRAME_E2E_FRONTEND_PORT": str(urlsplit(binding["endpoints"]["frontend"]).port),
        "RYFRAME_E2E_RUN_ID": binding["run_id"],
    })
    state = Path(binding["login_budget"]["path"])
    values.update(login_budget_environment(Path(binding["backend"]["path"]), private, state))
    if (values["RYFRAME_E2E_LOGIN_RATE_LIMIT_CAPACITY"] != str(login["capacity"])
            or values["RYFRAME_E2E_LOGIN_RATE_LIMIT_WINDOW_SECS"] != str(login["window_secs"])):
        raise ValueError("业务 crate 浏览器登录预算与绑定不一致")
    response_audit = binding.get("response_audit")
    if binding["server"] == "preview":
        if not isinstance(response_audit, dict) or set(response_audit) != {
                "path", "initial_state", "first_writer"}:
            raise ValueError("业务 crate preview 缺少静态响应审计绑定")
        values["RYFRAME_E2E_PREVIEW_RESPONSE_AUDIT"] = response_audit["path"]
    elif response_audit is not None:
        raise ValueError("业务 crate dev 不得绑定生产静态响应审计")
    return values
