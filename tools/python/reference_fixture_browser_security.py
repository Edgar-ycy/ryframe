"""为 业务 crate 浏览器子进程构造最小环境并从权威秘密文件取得脱敏值。"""

from __future__ import annotations

from pathlib import Path
from urllib.parse import unquote, urlsplit

from process_environment import configured


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
    """只向子进程传递环境选择及已登记的第三方工具路径。"""
    if not isinstance(private.get("APP_ENV"), str) or not private["APP_ENV"]:
        raise ValueError("业务 crate 浏览器缺少 APP_ENV 环境选择")
    return configured({
        "APP_ENV": private["APP_ENV"],
        "COREPACK_ENABLE_NETWORK": "0", "PYTHONUTF8": "1",
        "COREPACK_HOME": binding["tools"]["corepack_home"],
        **{name: private[name] for name in ("TEMP", "TMP") if name in private},
    })


def browser_configuration(binding: dict, password: str) -> dict:
    """凭据仅写入本机受管工具文件，不进入环境、命令或公开收据。"""
    if not isinstance(password, str) or not password.strip():
        raise ValueError("业务 crate 浏览器缺少已登记管理员凭据")
    request, login = binding["rate_limits"]["request"], binding["rate_limits"]["login"]
    values = {
        "scopeId": binding["scope_id"], "tenantId": binding["identity"]["tenant_id"],
        "username": binding["identity"]["username"], "password": password,
        "backendDir": binding["backend"]["path"],
        "runtimeDir": str(Path(binding["runtime"]["path"]).parent),
        "python": binding["tools"]["python"]["path"],
        "requestCapacity": request["capacity"], "requestWindowSeconds": request["window_secs"],
        "loginCapacity": login["capacity"], "loginWindowSeconds": login["window_secs"],
        "loginBudgetState": binding["login_budget"]["path"],
        "port": urlsplit(binding["endpoints"]["frontend"]).port,
        "serverMode": binding["server"], "fixture": "business", "runId": binding["run_id"],
        "channel": "chrome",
    }
    response_audit = binding.get("response_audit")
    if binding["server"] == "preview":
        if not isinstance(response_audit, dict) or set(response_audit) != {
                "path", "initial_state", "first_writer"}:
            raise ValueError("业务 crate preview 缺少静态响应审计绑定")
        values["previewResponseAudit"] = response_audit["path"]
    elif response_audit is not None:
        raise ValueError("业务 crate dev 不得绑定生产静态响应审计")
    return values
