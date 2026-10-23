"""迁移测试的受限 MySQL 会话：只连接明确的本机隔离目标，不提供任意 SQL CLI。"""
from __future__ import annotations

import json
import os
from contextlib import contextmanager, ExitStack
import queue
import re
import subprocess
import threading
import time
import uuid
from pathlib import Path

from rate_limit_client import load_app_table
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


def bindings(backend: Path, target_key: str = "shared", *, config_dir: Path | None = None) -> tuple[dict, dict]:
    if target_key not in {"shared", "dedicated-a", "dedicated-b"}:
        raise ValueError("仅接受全栈测试明确登记的目标")
    if os.environ.get("APP_ENV") != "test":
        raise ValueError("迁移测试只接受 APP_ENV=test 的文件配置")
    settings = load_app_table(backend, {"APP_ENV": "test"}, config_dir=config_dir)
    control, configured_targets = configured_databases(settings)
    targets = [target for target in configured_targets
               if target.get("key") == target_key]
    mode = "shared" if target_key == "shared" else "dedicated"
    if len(targets) != 1 or targets[0].get("kind") != "mysql" or targets[0].get("mode") != mode:
        raise ValueError("测试需要唯一明确登记且模式一致的 MySQL 目标")
    target = dict(targets[0])
    for binding in (control, target):
        validate_binding(binding)
        if binding["tls_mode"] not in ("disabled", "preferred", "required"):
            raise ValueError("此隔离 gate 尚不支持需要 CA/主机名参数的 MySQL TLS 模式")
        binding["_scope_id"] = settings.get("scope_id")
        binding["_client_dir"] = str(backend / ".local-tests/mysql-client")
        if os.environ.get("GITHUB_ACTIONS") == "true":
            binding["_container_id"] = load_control(backend)["mysql_container_id"]
        else:
            binding["_mysql_client"] = load_control(backend)["mysql_client"]
    if int(control["port"]) != int(target["port"]) or control["database"] == target["database"]:
        raise ValueError("故障 gate 仅接受同一 MySQL 实例上的两个不同隔离 schema")
    return control, target


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


@contextmanager
def invocation(binding: dict):
    with client_invocation(binding, "mysql") as (command, environment, prefix):
        yield command + ["--batch", "--raw", "--unbuffered", "--skip-column-names",
                         "--skip-reconnect", "--connect-timeout=5"], environment, prefix


class MysqlSession:
    def __init__(self, binding: dict):
        self._resources = ExitStack()
        self._password = binding.get("password", "")
        try:
            command, environment, prefix = self._resources.enter_context(invocation(binding))
            self.process = subprocess.Popen(command, env=environment, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        text=True, encoding="utf-8", bufsize=1,
                                            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            if prefix:
                self.process.stdin.write(prefix)
                self.process.stdin.flush()
        except BaseException:
            try:
                if hasattr(self, "process"):
                    self.process.kill()
                    self.process.communicate()
            finally:
                self._resources.close()
            raise
        self.lines, self.errors = queue.Queue(), []
        self.readers = [threading.Thread(target=target, daemon=True) for target in (self._read, self._errors)]
        for reader in self.readers:
            reader.start()

    def _read(self):
        for line in self.process.stdout:
            self.lines.put(line.rstrip("\r\n"))
        self.lines.put(None)

    def _errors(self):
        for line in self.process.stderr:
            detail = line.rstrip()
            if self._password:
                escaped = client_options(self._password).splitlines()[1][10:-1]
                for secret in sorted({self._password, escaped, *self._password.splitlines()}, key=len, reverse=True):
                    if secret:
                        detail = detail.replace(secret, "<redacted>")
            self.errors.append(detail)
            self.errors = self.errors[-8:]

    def execute(self, sql: str, timeout: float = 10) -> list[dict]:
        sentinel = "gate-" + uuid.uuid4().hex
        self.process.stdin.write(sql.rstrip(";\n") + f";\nSELECT '{sentinel}';\n")
        self.process.stdin.flush()
        deadline, result = time.monotonic() + timeout, []
        while True:
            try:
                line = self.lines.get(timeout=max(0.001, deadline - time.monotonic()))
            except queue.Empty as error:
                raise TimeoutError("MySQL gate 会话没有返回已提交语句的确认") from error
            if line is None:
                raise RuntimeError("MySQL gate 会话退出：" + " | ".join(self.errors))
            if line == sentinel:
                return result
            result.append(json.loads(line))
            if time.monotonic() >= deadline:
                raise TimeoutError("MySQL gate 会话超时")

    def close(self):
        try:
            self._close_process()
        finally:
            self._resources.close()

    def _close_process(self):
        try:
            if self.process.poll() is None:
                self.execute("ROLLBACK", timeout=5)
                self.process.stdin.close()
                self.process.wait(timeout=5)
        finally:
            if self.process.poll() is None:
                self.process.kill()
                self.process.wait(timeout=5)
            for reader in self.readers:
                reader.join(timeout=5)
            for stream in (self.process.stdin, self.process.stdout, self.process.stderr):
                stream.close()

    def __enter__(self):
        return self

    def __exit__(self, *_args):
        self.close()


def ownership(session: MysqlSession, schema: str, scope: str, kind: str) -> str:
    identifier(schema)
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{2,63}", scope) or kind not in {"control", "tenant-data"}:
        raise ValueError("ownership 参数无效")
    rows = session.execute(f"SELECT JSON_OBJECT('scope',scope_id,'marker',marker,'server',@@server_uuid) "
                           f"FROM `{schema}`.ryframe_resource_ownership WHERE resource_kind='{kind}'")
    if len(rows) != 1 or rows[0].get("scope") != scope or rows[0].get("marker") != f"ryframe-owner:v1:{scope}:{kind}":
        raise ValueError("MySQL scope 或 ownership marker 不匹配")
    return rows[0]["server"]
