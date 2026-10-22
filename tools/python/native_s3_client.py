"""调用当前协调器的原生 S3 私有协议；对象凭据只通过标准输入传递。"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shutil
import subprocess

from rate_limit_tool import tool_binding, verified_tool

ROOT = Path(__file__).resolve().parents[2]
MAX_PROTOCOL_BYTES = 1024 * 1024
_SYSTEM_NAMES = frozenset({
    "PATH", "PATHEXT", "SYSTEMROOT", "WINDIR", "COMSPEC", "HOME", "USERPROFILE",
    "HOMEDRIVE", "HOMEPATH", "TEMP", "TMP",
})


class NativeS3InvocationError(RuntimeError):
    def __init__(self, outcome: str, *, returncode=None, error_type=None,
                 remote_write_may_have_occurred: bool, http_status=None):
        self.outcome = outcome
        self.returncode = returncode
        self.error_type = error_type
        self.remote_write_may_have_occurred = remote_write_may_have_occurred
        self.http_status = http_status
        super().__init__(f"原生对象操作未确认：{outcome}；禁止降级或自动重试")


class NativeS3NotFound(NativeS3InvocationError):
    """仅由原生协议确认的 HEAD 404；不能由错误文本或其他失败推断。"""

    def __init__(self, *, returncode: int):
        super().__init__(
            "not_found", returncode=returncode, error_type="NativeS3NotFound",
            remote_write_may_have_occurred=False, http_status=404,
        )


def put_if_absent(request: dict, runner=subprocess.run) -> tuple[dict, dict]:
    result, response = _invoke("__s3-put-if-absent", request, runner, may_write=True)
    return _write_response(result, response)


def create_bucket(request: dict, runner=subprocess.run) -> tuple[dict, dict]:
    result, response = _invoke("__s3-create-bucket", request, runner, may_write=True)
    return _write_response(result, response)


def _write_response(result, response) -> tuple[dict, dict]:
    if (result.returncode == 0 and isinstance(response, dict)
            and set(response) == {"ok", "result"} and response["ok"] is True
            and isinstance(response["result"], dict)):
        outcome = response["result"]
        if set(outcome) - {
            "status", "reason", "http_status", "response",
            "remote_write_may_have_occurred", "automatic_retry_permitted",
        } or outcome.get("status") not in {
            "created_unverified", "already_exists", "denied", "needs_reconciliation",
        } or outcome.get("automatic_retry_permitted") is not False \
                or not isinstance(outcome.get("remote_write_may_have_occurred"), bool):
            raise NativeS3InvocationError(
                "needs_reconciliation", returncode=result.returncode,
                error_type="InvalidOutcome", remote_write_may_have_occurred=True,
            )
        return outcome, {"returncode": result.returncode, "error_type": None}
    if _known_rejection(result, response):
        raise NativeS3InvocationError(
            "request_rejected", returncode=result.returncode,
            error_type=response["error"].get("category"),
            remote_write_may_have_occurred=False,
        )
    raise NativeS3InvocationError(
        "needs_reconciliation", returncode=result.returncode,
        error_type="InconsistentProtocolResponse", remote_write_may_have_occurred=True,
    )


def read(request: dict, runner=subprocess.run) -> tuple[dict, dict]:
    result, response = _invoke("__s3-read", request, runner, may_write=False)
    return _read_response(result, response)


def inspect(request: dict, runner=subprocess.run) -> tuple[dict, dict]:
    result, response = _invoke("__s3-inspect", request, runner, may_write=False)
    return _read_response(result, response)


def _read_response(result, response) -> tuple[dict, dict]:
    if (result.returncode == 0 and isinstance(response, dict)
            and set(response) == {"ok", "result"} and response["ok"] is True
            and isinstance(response["result"], dict)):
        if response["result"] == {
                "status": "not_found", "http_status": 404,
                "remote_write_may_have_occurred": False}:
            raise NativeS3NotFound(returncode=result.returncode)
        return response["result"], {"returncode": result.returncode, "error_type": None}
    if _known_rejection(result, response):
        raise NativeS3InvocationError(
            "request_rejected", returncode=result.returncode,
            error_type=response["error"].get("category"),
            remote_write_may_have_occurred=False,
        )
    raise NativeS3InvocationError(
        "read_failed", returncode=result.returncode,
        error_type="InconsistentProtocolResponse", remote_write_may_have_occurred=False,
    )


def _invoke(command_name: str, request: dict, runner, *, may_write: bool):
    payload = json.dumps(
        request, ensure_ascii=False, sort_keys=True, separators=(",", ":")
    ).encode("utf-8")
    if not payload or len(payload) > MAX_PROTOCOL_BYTES:
        raise ValueError("原生对象私有协议请求为空或超过 1 MiB")
    options = {"creationflags": subprocess.CREATE_NO_WINDOW} if os.name == "nt" else {}
    result = None
    try:
        with verified_tool(tool_binding()) as executable:
            command = ([executable, command_name] if executable is not None else [
                os.environ.get("CARGO") or shutil.which("cargo") or "cargo",
                "run", "--quiet", "--locked", "-p", "xtask", "--",
                command_name,
            ])
            result = runner(
                command,
                cwd=ROOT,
                env=_child_environment(),
                input=payload,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=1810,
                check=False,
                **options,
            )
    except subprocess.TimeoutExpired as error:
        raise NativeS3InvocationError(
            "needs_reconciliation", returncode=None, error_type=type(error).__name__,
            remote_write_may_have_occurred=may_write,
        ) from None
    except subprocess.CalledProcessError as error:
        raise NativeS3InvocationError(
            "needs_reconciliation", returncode=error.returncode,
            error_type=type(error).__name__, remote_write_may_have_occurred=may_write,
        ) from None
    except OSError as error:
        raise NativeS3InvocationError(
            "native_tool_unavailable", returncode=None, error_type=type(error).__name__,
            remote_write_may_have_occurred=False,
        ) from None
    if len(result.stdout) > MAX_PROTOCOL_BYTES:
        raise NativeS3InvocationError(
            "needs_reconciliation", returncode=result.returncode,
            error_type="ProtocolOutputTooLarge", remote_write_may_have_occurred=may_write,
        )
    try:
        response = json.loads(result.stdout.decode("utf-8"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        raise NativeS3InvocationError(
            "needs_reconciliation", returncode=result.returncode,
            error_type="InvalidProtocolResponse", remote_write_may_have_occurred=may_write,
        ) from None
    return result, response


def _known_rejection(result, response) -> bool:
    return (result.returncode != 0 and isinstance(response, dict)
            and set(response) == {"ok", "error"} and response["ok"] is False
            and isinstance(response["error"], dict)
            and response["error"].get("remote_write_may_have_occurred") is False)


def _child_environment() -> dict[str, str]:
    return {
        key: value for key, value in os.environ.items()
        if key.upper() in _SYSTEM_NAMES
        or key.upper().startswith(("PROCESSOR_", "LC_", "CARGO_", "RUSTUP_"))
    }
