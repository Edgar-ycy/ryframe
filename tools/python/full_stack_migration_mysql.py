"""CI 全栈数据库准备的配置校验与受管客户端参数，不持有交互式数据库会话。"""
from __future__ import annotations

import json
import os
from contextlib import contextmanager, ExitStack
import re
from pathlib import Path

from workspace_directory import WorkspaceDirectory


def load_control(backend: Path) -> dict:
    path = backend / ".local-tests/full-stack-control.json"
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 16 * 1024:
        raise ValueError("全栈工具缺少无链接的受管 full-stack-control.json")
    value = json.loads(path.read_text(encoding="utf-8"))
    base = {"format_version", "runtime_dir"}
    ci_fields = {"mysql_container_id", "redis_container_id", "s3_container_id"}
    if (not isinstance(value, dict) or set(value) not in (base | ci_fields, base | {"mysql_client"})
            or type(value["format_version"]) is not int or value["format_version"] != 1):
        raise ValueError("全栈工具配置字段无效")
    if not isinstance(value["runtime_dir"], str) or not Path(value["runtime_dir"]).is_absolute():
        raise ValueError("全栈 runtime_dir 必须是明确绝对路径")
    if "mysql_client" in value:
        client = value["mysql_client"]
        if not isinstance(client, str) or not Path(client).is_absolute() or not Path(client).is_file():
            raise ValueError("全栈工具配置 mysql_client 必须是实际客户端的绝对路径")
        return value
    for role in ("mysql", "redis", "s3"):
        if not isinstance(value[f"{role}_container_id"], str) or not re.fullmatch(r"[a-f0-9]{12,64}", value[f"{role}_container_id"]):
            raise ValueError(f"全栈工具配置 {role}_container_id 必须是明确容器 ID")
    return value


def identifier(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[a-zA-Z0-9_]{1,64}", value):
        raise ValueError("测试数据库标识无效")
    return value


def configured_databases(settings: dict) -> tuple[dict, list[dict]]:
    database, tenant_data = settings.get("database", {}), settings.get("tenant_data", {})
    if not isinstance(database, dict) or not isinstance(tenant_data, dict):
        raise ValueError("MySQL 文件配置字段 database 与 tenant_data 必须是表")
    control, targets = database.get("primary", {}), tenant_data.get("targets", [])
    if (not isinstance(control, dict) or not isinstance(targets, list)
            or any(not isinstance(target, dict) for target in targets)):
        raise ValueError("MySQL 文件配置字段 database.primary 或 tenant_data.targets 类型无效")
    return dict(control), targets


def validate_binding(binding: dict) -> None:
    if not isinstance(binding, dict):
        raise ValueError("MySQL 连接必须是文件配置表")
    identifier(binding.get("database"))
    if (binding.get("host") not in ("127.0.0.1", "localhost")
            or type(binding.get("port")) is not int or not 1 <= binding["port"] <= 65535):
        raise ValueError("迁移测试只允许明确的本机 MySQL 地址和有效端口")
    if binding.get("tls_mode") not in ("disabled", "preferred", "required", "verify_ca", "verify_identity"):
        raise ValueError("MySQL TLS 模式无效")
    if binding["tls_mode"] in ("verify_ca", "verify_identity") and not binding.get("tls_ca"):
        raise ValueError("MySQL 文件配置字段 tls_ca 必须明确提供 CA 文件")
    if bool(binding.get("tls_client_cert")) != bool(binding.get("tls_client_key")):
        raise ValueError("MySQL TLS 客户端证书与私钥必须成对配置")
    for field in ("username", "password"):
        value = binding.get(field)
        if (not isinstance(value, str) or not value
                or any(ord(char) < 32 and char not in "\n\r\t\b" for char in value)):
            raise ValueError(f"MySQL 文件配置字段 {field} 必须是有效的非空文本")
    if any(key.endswith("_env") for key in binding):
        raise ValueError("MySQL 文件配置不接受环境变量凭据引用")


def client_options(password: str) -> str:
    escaped = password.replace("\\", "\\\\").replace('"', '\\"')
    for value, replacement in (("\n", "\\n"), ("\r", "\\r"), ("\t", "\\t"), ("\b", "\\b")):
        escaped = escaped.replace(value, replacement)
    return f'[client]\npassword="{escaped}"\n'


# 前两行只用于选项文件；后续 stdin 完整交给客户端，连接退出即清理凭据。
CONTAINER_CLIENT = '''umask 077
config=$(mktemp) || exit 1
trap 'rm -f -- "$config"' EXIT
trap 'exit 130' HUP INT TERM
IFS= read -r header && IFS= read -r credential || exit 1
printf '%s\\n' "$header" "$credential" > "$config" || exit 1
client=$1
shift
"$client" --defaults-file="$config" "$@"
'''


@contextmanager
def client_invocation(binding: dict, client: str):
    """凭据只进入受管选项文件或 stdin，调用方持有上下文直到客户端退出。"""
    validate_binding(binding)
    if client not in {"mysql", "mysqldump"}:
        raise ValueError("未知 MySQL 测试客户端")
    host_names = {"PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC", "HOME", "USERPROFILE",
                  "HOMEDRIVE", "HOMEPATH", "TEMP", "TMP", "DOCKER_HOST", "DOCKER_CONFIG",
                  "DOCKER_TLS_VERIFY", "DOCKER_CERT_PATH"}
    environment = {key: value for key, value in os.environ.items() if key.upper() in host_names}
    options = client_options(binding["password"])
    with ExitStack() as resources:
        command, port, prefix = _client_command(binding, client, options, resources)
        command += [f"--host={binding['host']}", f"--port={port}", f"--user={binding['username']}",
                    "--protocol=TCP", "--default-character-set=utf8mb4",
                    "--ssl-mode=" + binding["tls_mode"].upper()]
        for field, flag in (("tls_ca", "--ssl-ca"), ("tls_client_cert", "--ssl-cert"),
                            ("tls_client_key", "--ssl-key")):
            if binding.get(field):
                command.append(f"{flag}={binding[field]}")
        yield command, environment, prefix


def _client_command(binding: dict, client: str, options: str, resources: ExitStack):
    if os.environ.get("GITHUB_ACTIONS") == "true":
        container = binding.get("_container_id", "")
        run, attempt = os.environ.get("GITHUB_RUN_ID", ""), os.environ.get("GITHUB_RUN_ATTEMPT", "")
        if (not re.fullmatch(r"[a-f0-9]{12,64}", container)
                or not re.fullmatch(r"[1-9][0-9]*", run)
                or not re.fullmatch(r"[1-9][0-9]*", attempt)
                or binding.get("_scope_id") != f"ci-{run}-{attempt}"):
            raise ValueError("MySQL 容器必须明确属于当前 CI run 与 attempt")
        return ["docker", "exec", "-i", container, "sh", "-c", CONTAINER_CLIENT,
                "ryframe-mysql", client], 3306, options
    else:
        executable = Path(binding.get("_mysql_client", ""))
        if not executable.is_absolute() or not executable.is_file():
            raise ValueError("绑定工具配置的 mysql_client 必须是实际客户端的绝对路径")
        if client == "mysqldump":
            executable = executable.with_name("mysqldump.exe" if os.name == "nt" else "mysqldump")
            if not executable.is_file():
                raise ValueError("已验证 mysql 客户端目录缺少 mysqldump")
        directory = Path(binding.get("_client_dir", Path(__file__).resolve().parents[2] / ".local-tests/mysql-client"))
        if not directory.is_absolute():
            raise ValueError("MySQL 受管客户端目录必须使用绝对路径")
        directory.mkdir(parents=True, exist_ok=True)
        temporary = resources.enter_context(WorkspaceDirectory(prefix="mysql-", dir=directory))
        option_file = Path(temporary) / "client.cnf"
        with option_file.open("x", encoding="utf-8", newline="\n") as output:
            option_file.chmod(0o600)
            output.write(options)
        return [str(executable), f"--defaults-file={option_file}"], binding["port"], ""
