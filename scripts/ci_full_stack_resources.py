"""仅为显式全栈环境准备对象桶和可追溯的二进制路径。"""

from __future__ import annotations

import datetime as dt
import hashlib
import hmac
import json
import os
import socket
import time
import urllib.parse
import urllib.error
import urllib.request
from pathlib import Path

BUCKETS = ("uploads", "avatar", "exports", "imports", "config-packages")
BINARIES = (("bin-reset", "ryframe-reset"), ("bin-migrate", "ryframe-migrate"),
            ("bin-api", "ryframe"), ("bin-worker", "ryframe-worker"))


def prepare_storage(required) -> None:
    if os.environ.get("APP_OBJECT_STORAGE_BACKEND", "local") == "local":
        storage = Path(required("APP_OBJECT_STORAGE_LOCAL_BASE_DIR"))
        for bucket in BUCKETS:
            (storage / bucket).mkdir(parents=True, exist_ok=True)
        return
    # 只在本 Job 声明的全新 RustFS 实例创建精确的业务桶。
    required("RYFRAME_CI_S3_CONTAINER_ID")
    endpoint = required("APP_OBJECT_STORAGE_ENDPOINT")
    parsed = urllib.parse.urlsplit(endpoint)
    if (parsed.scheme not in ("http", "https") or parsed.hostname not in ("127.0.0.1", "localhost")
            or parsed.path not in ("", "/") or parsed.query or parsed.fragment or parsed.username):
        raise ValueError("CI S3 端点必须指向本 Job 的 loopback 实例")
    deadline = time.monotonic() + 90
    while True:
        try:
            with socket.create_connection((parsed.hostname, parsed.port or 80), timeout=2):
                break
        except OSError:
            if time.monotonic() >= deadline:
                raise ValueError("隔离 RustFS 就绪等待超时") from None
            time.sleep(1)
    for bucket in BUCKETS:
        request = bucket_request(endpoint, bucket, required("APP_OBJECT_STORAGE_ACCESS_KEY"),
                                 required("APP_OBJECT_STORAGE_SECRET_KEY"))
        create_bucket(request, bucket, deadline)


def create_bucket(request, bucket: str, deadline: float) -> None:
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(request, timeout=min(30, max(1, deadline - time.monotonic()))) as response:
                if response.status not in (200, 204):
                    raise ValueError(f"创建隔离对象桶 {bucket} 失败")
                return
        except urllib.error.HTTPError as error:
            # 只重试尚未就绪的隔离服务；认证失败与已存在的桶均必须失败。
            if error.code != 503:
                raise
        time.sleep(1)
    raise ValueError(f"创建隔离对象桶 {bucket} 等待超时")


def bucket_request(endpoint: str, bucket: str, access: str, secret: str,
                   now: dt.datetime | None = None) -> urllib.request.Request:
    moment = now or dt.datetime.now(dt.timezone.utc)
    stamp, date = moment.strftime("%Y%m%dT%H%M%SZ"), moment.strftime("%Y%m%d")
    host = urllib.parse.urlsplit(endpoint).netloc
    empty = hashlib.sha256(b"").hexdigest()
    signed = "host;x-amz-content-sha256;x-amz-date"
    headers = f"host:{host}\nx-amz-content-sha256:{empty}\nx-amz-date:{stamp}\n"
    canonical = f"PUT\n/{bucket}\n\n{headers}\n{signed}\n{empty}"
    scope = f"{date}/us-east-1/s3/aws4_request"
    signing = f"AWS4-HMAC-SHA256\n{stamp}\n{scope}\n{hashlib.sha256(canonical.encode()).hexdigest()}"
    key = ("AWS4" + secret).encode()
    for value in (date, "us-east-1", "s3", "aws4_request"):
        key = hmac.digest(key, value.encode(), "sha256")
    signature = hmac.new(key, signing.encode(), hashlib.sha256).hexdigest()
    return urllib.request.Request(endpoint.rstrip("/") + "/" + bucket, data=b"", method="PUT",
                                  headers={"X-Amz-Date": stamp, "X-Amz-Content-SHA256": empty,
                                           "Authorization": f"AWS4-HMAC-SHA256 Credential={access}/{scope}, SignedHeaders={signed}, Signature={signature}"})


def build_binaries(run, backend_root: Path, output_dir: Path) -> dict[str, str]:
    binaries: dict[str, str] = {}
    for feature, name in BINARIES:
        result = run(["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features",
                      "--features", feature, "--bin", name, "--message-format=json"],
                     cwd=backend_root, capture_output=True)
        for line in result.stdout.splitlines():
            event = json.loads(line)
            if (event.get("reason") == "compiler-artifact"
                    and event.get("target", {}).get("name") == name and event.get("executable")):
                binaries[name] = str(Path(event["executable"]).resolve())
        if name not in binaries:
            raise ValueError(f"Cargo 没有返回 {name} 的 executable")
    (output_dir / "binaries.json").write_text(json.dumps(binaries, indent=2) + "\n", encoding="utf-8")
    return binaries


def read_binaries(output_dir: Path) -> dict[str, str]:
    binaries = json.loads((output_dir / "binaries.json").read_text(encoding="utf-8"))
    if set(binaries) != {name for _, name in BINARIES}:
        raise ValueError("全栈二进制清单不完整")
    for value in binaries.values():
        if not Path(value).is_absolute() or not Path(value).is_file():
            raise ValueError("全栈二进制路径无效")
    return binaries
