"""正式恢复的隔离 Prometheus、Alertmanager 与本地 webhook 验收入口。"""

from __future__ import annotations

import sys

sys.dont_write_bytecode = True

import argparse
import json
import re
import subprocess
from pathlib import Path

from devex_clone_source_proof import require_closed_port
from restore_build import repository, validate_new_output, write_new
from restore_monitoring_authority import monitoring_preflight
from restore_monitoring_evidence import (
    build_binding,
    descriptor,
    load_environment,
    require_run_members,
    verify_binding_inputs,
)
from restore_monitoring_staging import (
    DIRECTORY as STAGING_DIRECTORY,
    TOKEN_NAME,
    verified_staging_execution,
)
from restore_runtime_evidence import reject_link_or_reparse


def bind(
    backend: Path,
    runtime_receipt: Path,
    target_plan: Path,
    output: Path,
    run_id: str,
    credential: Path,
    tool_paths: dict[str, Path],
    ports: dict[str, int],
    *,
    preflight=monitoring_preflight,
    run=subprocess.run,
    port_check=require_closed_port,
    acl_reader=None,
) -> dict:
    backend = repository(backend, "监控验收协调后端")
    for label, path in (
        ("恢复运行收据", runtime_receipt),
        ("恢复目标计划", target_plan),
        ("监控凭据", credential),
        ("监控绑定输出", output),
    ):
        if not path.is_absolute():
            raise ValueError(f"{label}必须使用绝对路径")
    reject_link_or_reparse(output.parent)
    output = validate_new_output(output, backend)
    if output.name != "binding.json":
        raise ValueError("监控绑定输出必须命名为 binding.json")
    run_directory = output.parent
    if credential != run_directory / TOKEN_NAME:
        raise ValueError("监控凭据必须位于本次 run 的固定路径")
    try:
        require_run_members(run_directory, {TOKEN_NAME})
    except ValueError:
        raise ValueError("监控绑定要求只包含固定凭据的新 run 目录") from None
    authority = preflight(backend, runtime_receipt, target_plan)
    binding = build_binding(
        backend,
        run_id,
        authority,
        run_directory,
        credential,
        tool_paths,
        ports,
        run=run,
        port_check=port_check,
        acl_reader=acl_reader,
    )
    verified, snapshots = verify_binding_inputs(
        backend, binding, preflight, run, acl_reader=acl_reader
    )
    if verified != binding:
        raise ValueError("监控绑定发布前无法重建相同权威")
    for snapshot in snapshots:
        snapshot.assert_unchanged()
    require_run_members(run_directory, {TOKEN_NAME, STAGING_DIRECTORY})
    write_new(output, binding, backend)
    return binding


def _lifecycle_arguments(child: argparse.ArgumentParser, *, write: bool) -> None:
    child.add_argument("--backend-dir", type=Path, required=True)
    child.add_argument("--binding", type=Path, required=True)
    if write:
        child.add_argument("--write", action="store_true", required=True)


def _decode_result(raw: object) -> dict:
    if not isinstance(raw, (bytes, str)):
        raise ValueError("staged monitoring runner 没有返回 JSON")
    try:
        value = json.loads(raw.decode("utf-8") if isinstance(raw, bytes) else raw)
    except (UnicodeError, json.JSONDecodeError) as error:
        raise ValueError("staged monitoring runner 没有返回有效 JSON") from error
    if not isinstance(value, dict) or value.get("status") == "failed":
        raise ValueError("staged monitoring runner 执行失败")
    return value


def _failure_type(raw: object) -> str:
    if not isinstance(raw, (bytes, str)):
        return "Unknown"
    content = raw.decode("utf-8", errors="replace") if isinstance(raw, bytes) else raw
    if len(content) > 4096:
        return "InvalidOutput"
    try:
        value = json.loads(content)
    except json.JSONDecodeError:
        return "InvalidOutput"
    error_type = value.get("error_type") if isinstance(value, dict) and value.get("status") == "failed" else None
    return error_type if isinstance(error_type, str) and re.fullmatch(r"[A-Za-z][A-Za-z0-9_]{0,127}", error_type) else "InvalidOutput"


def _dispatch_staged(command: str, backend: Path, binding_path: Path, *, run=subprocess.run) -> dict:
    from restore_monitoring_runtime import publish_result, read_binding

    binding_document, binding = read_binding(backend, binding_path)
    binding, snapshots = verify_binding_inputs(backend, binding, monitoring_preflight, run)
    private, credential, environment_document = load_environment(
        backend, binding["authority"], Path(binding["credential"]["path"])
    )
    if credential != binding["credential"]:
        raise ValueError("监控私有环境凭据与绑定不同")
    with verified_staging_execution(
        binding_document.path.parent,
        binding["staging"],
        binding["coordinator"],
        private_environment=private,
    ) as execution:
        completed = run(
            [
                *execution["runner_command"],
                "__" + command,
                "--backend-dir",
                str(backend),
                "--binding",
                str(binding_path),
            ],
            cwd=execution["root"],
            env=execution["environment"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            timeout=900,
            check=False,
            creationflags=subprocess.CREATE_NO_WINDOW if __import__("os").name == "nt" else 0,
        )
        if completed.returncode != 0:
            receipt = binding_path.parent / f"{command}-failure.json"
            location = receipt.name if receipt.is_file() else "未形成阶段失败收据"
            raise ValueError(
                f"staged monitoring runner 阶段 {command} 失败："
                f"exit={completed.returncode}, error_type={_failure_type(completed.stdout)}, receipt={location}"
            )
        result = _decode_result(completed.stdout)
    for snapshot in (*snapshots, environment_document):
        snapshot.assert_unchanged()
    binding_document.assert_unchanged()
    return publish_result(backend, binding_document, binding, result) if command == "result" else result


def _require_staged(binding: dict) -> None:
    runner = Path(__file__).resolve(strict=True)
    python = Path(sys.executable).resolve(strict=True)
    if runner != Path(binding["runner"]["path"]) or python != Path(binding["python"]["path"]):
        raise ValueError("监控生命周期只能由绑定的 staged runner 与 Python 执行")


def _internal_lifecycle(command: str, backend: Path, binding_path: Path) -> dict:
    from restore_monitoring_runtime import (
        cleanup_interrupted_runtime,
        close_runtime,
        observe_runtime,
        prepare_result,
        read_binding,
        start_runtime,
        status,
    )

    binding_document, binding = read_binding(backend, binding_path)
    _require_staged(binding)
    private, credential, environment_document = load_environment(
        backend, binding["authority"], Path(binding["credential"]["path"])
    )
    if credential != binding["credential"] or dict(__import__("os").environ).get(
        "APP_MONITOR_METRICS_BEARER_TOKEN"
    ) != private["APP_MONITOR_METRICS_BEARER_TOKEN"]:
        raise ValueError("staged monitoring runner 没有继承绑定凭据")
    try:
        with verified_staging_execution(
            binding_document.path.parent,
            binding["staging"],
            binding["coordinator"],
            private_environment=private,
        ) as execution:
            if command == "start":
                result = start_runtime(backend, binding_document, binding, execution)
            elif command == "observe":
                result = observe_runtime(
                    backend,
                    binding_document,
                    binding,
                    execution,
                    private["APP_MONITOR_METRICS_BEARER_TOKEN"],
                )
            elif command == "close":
                result = close_runtime(backend, binding_document, binding)
            elif command == "result":
                result = prepare_result(binding_document, binding)
            elif command == "status":
                result = status(binding_document, binding)
            else:
                raise ValueError("未知监控生命周期阶段")
    except BaseException as error:
        if command in {"start", "observe"}:
            cleanup_interrupted_runtime(binding_document, binding, error)
        raise
    environment_document.assert_unchanged()
    binding_document.assert_unchanged()
    return result


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    commands = parser.add_subparsers(dest="command", required=True)
    child = commands.add_parser("bind", help="只读复核正式恢复运行并发布监控输入绑定", allow_abbrev=False)
    child.add_argument("--backend-dir", type=Path, required=True)
    child.add_argument("--runtime-receipt", type=Path, required=True)
    child.add_argument("--target-plan", type=Path, required=True)
    child.add_argument("--output", type=Path, required=True)
    child.add_argument("--run-id", required=True)
    child.add_argument(
        "--metrics-token-file",
        type=Path,
        required=True,
        help="必须是 binding.json 同目录内预先收紧权限的 metrics-token.txt",
    )
    for name in ("prometheus", "promtool", "alertmanager", "amtool"):
        child.add_argument("--" + name, type=Path, required=True)
    for name in ("prometheus", "alertmanager", "webhook"):
        child.add_argument("--" + name + "-port", type=int, required=True)
    child.add_argument("--write", action="store_true", required=True)
    for name in ("start", "observe", "close", "result"):
        _lifecycle_arguments(
            commands.add_parser(name, help=f"执行隔离监控 {name} 阶段", allow_abbrev=False),
            write=True,
        )
    _lifecycle_arguments(
        commands.add_parser("status", help="只读查看隔离监控生命周期", allow_abbrev=False),
        write=False,
    )
    for name in ("__start", "__observe", "__close", "__result", "__status"):
        _lifecycle_arguments(commands.add_parser(name, add_help=False, allow_abbrev=False), write=False)
    return parser


def main(arguments: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if arguments is None else arguments
    if arguments[:1] == ["__webhook"]:
        from restore_monitoring_webhook import main as webhook_main

        runner = Path(__file__).resolve(strict=True)
        if runner.parent.parent.name != STAGING_DIRECTORY or Path(sys.executable).parent.parent != runner.parent.parent:
            return 1
        webhook_main(arguments[1:])
        return 0
    options = [value.partition("=")[0] for value in arguments if value.startswith("--")]
    if len(options) != len(set(options)):
        _parser().error("监控投递选项不能重复")
    args = _parser().parse_args(arguments)
    try:
        if args.command == "bind":
            result = bind(
                args.backend_dir,
                args.runtime_receipt,
                args.target_plan,
                args.output,
                args.run_id,
                args.metrics_token_file,
                {name: getattr(args, name) for name in ("prometheus", "promtool", "alertmanager", "amtool")},
                {name: getattr(args, name + "_port") for name in ("prometheus", "alertmanager", "webhook")},
            )
        elif args.command.startswith("__"):
            result = _internal_lifecycle(args.command.removeprefix("__"), args.backend_dir, args.binding)
        else:
            result = _dispatch_staged(args.command, args.backend_dir, args.binding)
        if args.command.startswith("__"):
            print(json.dumps(result, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
            return 0
        if args.command == "bind":
            binding_receipt = descriptor(args.output)
        elif isinstance(result.get("binding"), dict):
            binding_receipt = result["binding"]
        else:
            binding_receipt = descriptor(args.binding)
        response = {
            "status": result["status"],
            "run_id": result["run_id"],
            "scope_id": result["scope_id"],
            "binding": binding_receipt,
            "external_contacts": 0,
        }
        for name in ("next_action", "processes_alive"):
            if name in result:
                response[name] = result[name]
        print(json.dumps(response, ensure_ascii=False, sort_keys=True, separators=(",", ":")))
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(
            json.dumps(
                {"status": "failed", "error_type": type(error).__name__},
                sort_keys=True,
                separators=(",", ":"),
            )
        )
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
