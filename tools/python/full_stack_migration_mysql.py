"""迁移测试的受限 MySQL 会话：只连接明确的本机隔离目标，不提供任意 SQL CLI。"""
from __future__ import annotations

import json
import os
import queue
import re
import subprocess
import threading
import time
import tomllib
import uuid
from pathlib import Path


def identifier(value: str) -> str:
    if not isinstance(value, str) or not re.fullmatch(r"[a-zA-Z0-9_]{1,64}", value):
        raise ValueError("测试数据库标识无效")
    return value


def bindings(backend: Path, target_key: str = "shared") -> tuple[dict, dict]:
    if target_key not in {"shared", "dedicated-a", "dedicated-b"}:
        raise ValueError("仅接受全栈测试明确登记的目标")
    control = {key: os.environ[f"APP_DATABASE_{name}"] for key, name in
               (("host", "HOST"), ("port", "PORT"), ("database", "NAME"),
                ("username", "USERNAME"), ("tls_mode", "TLS_MODE"))}
    control["password_env"] = "APP_DATABASE_PASSWORD"
    config = Path(os.environ["APP_CONFIG_DIR"])
    config = config if config.is_absolute() else backend / config
    settings = tomllib.loads((config / "app.test.toml").read_text(encoding="utf-8"))
    targets = [target for target in settings.get("tenant_data", {}).get("targets", [])
               if target.get("key") == target_key]
    mode = "shared" if target_key == "shared" else "dedicated"
    if len(targets) != 1 or targets[0].get("kind") != "mysql" or targets[0].get("mode") != mode:
        raise ValueError("测试需要唯一明确登记且模式一致的 MySQL 目标")
    target = targets[0]
    for binding in (control, target):
        identifier(binding["database"])
        if binding["host"] not in ("127.0.0.1", "localhost") or not 1 <= int(binding["port"]) <= 65535:
            raise ValueError("迁移测试只允许明确的本机 MySQL 地址")
        if binding["tls_mode"] not in ("disabled", "preferred", "required"):
            raise ValueError("此隔离 gate 尚不支持需要 CA/主机名参数的 MySQL TLS 模式")
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", binding["password_env"]):
            raise ValueError("测试 MySQL 密钥环境变量名无效")
    if int(control["port"]) != int(target["port"]) or control["database"] == target["database"]:
        raise ValueError("故障 gate 仅接受同一 MySQL 实例上的两个不同隔离 schema")
    return control, target


def client_invocation(binding: dict, client: str) -> tuple[list[str], dict]:
    if client not in {"mysql", "mysqldump"}:
        raise ValueError("未知 MySQL 测试客户端")
    environment = {**os.environ, "MYSQL_PWD": os.environ[binding["password_env"]]}
    if os.environ.get("GITHUB_ACTIONS") == "true":
        container = os.environ.get("RYFRAME_CI_MYSQL_CONTAINER_ID", "")
        run, attempt = os.environ.get("GITHUB_RUN_ID", ""), os.environ.get("GITHUB_RUN_ATTEMPT", "")
        if (not re.fullmatch(r"[a-f0-9]{12,64}", container)
                or not re.fullmatch(r"[1-9][0-9]*", run)
                or not re.fullmatch(r"[1-9][0-9]*", attempt)
                or os.environ.get("APP_SCOPE_ID") != f"ci-{run}-{attempt}"):
            raise ValueError("MySQL 容器必须明确属于当前 CI run 与 attempt")
        command = ["docker", "exec", "-i", "--env", "MYSQL_PWD", container, client]
        port = 3306
    else:
        executable = Path(os.environ.get("RYFRAME_E2E_MYSQL_CLIENT", ""))
        if not executable.is_absolute() or not executable.is_file():
            raise ValueError("RYFRAME_E2E_MYSQL_CLIENT 必须是实际 mysql 客户端的绝对路径")
        if client == "mysqldump":
            executable = executable.with_name("mysqldump.exe" if os.name == "nt" else "mysqldump")
            if not executable.is_file():
                raise ValueError("已验证 mysql 客户端目录缺少 mysqldump")
        command, port = [str(executable)], int(binding["port"])
    command += ["--no-defaults", "--host=127.0.0.1", f"--port={port}", f"--user={binding['username']}",
                "--protocol=TCP", "--default-character-set=utf8mb4",
                "--ssl-mode=" + binding["tls_mode"].upper()]
    return command, environment


def invocation(binding: dict) -> tuple[list[str], dict]:
    command, environment = client_invocation(binding, "mysql")
    return command + ["--batch", "--raw", "--unbuffered", "--skip-column-names",
                      "--skip-reconnect", "--connect-timeout=5"], environment


class MysqlSession:
    def __init__(self, binding: dict):
        command, environment = invocation(binding)
        self.process = subprocess.Popen(command, env=environment, stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                                        text=True, encoding="utf-8", bufsize=1,
                                        creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
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
            self.errors.append(line.rstrip())
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
