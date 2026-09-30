"""显式构建开发复制使用的维护 CLI；不连接服务、不初始化或复制资源。"""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess

from devex_clone import read_json
from devex_clone_model import exact, linked, local_path
from devex_provenance import worktree_fingerprint
from restore_build import cargo_artifact, file_digest, source_snapshot
from restore_reference_io import redact_object_diagnostic
from source_fingerprints import capture_inventory, checked_source, reusable_artifact_source

TOOLS = {"reset": ("bin-reset", "ryframe-reset"), "migrate": ("bin-migrate", "ryframe-migrate"),
         "tenant-data": ("bin-tenant-data", "ryframe-tenant-data")}


def command(role: str, target: Path) -> list[str]:
    feature, name = TOOLS[role]
    return ["cargo", "build", "--locked", "-p", "ryframe", "--no-default-features", "--features", feature,
            "--bin", name, "--jobs", "4", "--target-dir", str(target), "--message-format=json"]


def source_binding(backend: Path) -> dict:
    current = checked_source(backend)
    if current is not None:
        return {key: current[key] for key in ("snapshot", "worktree_fingerprint")}
    source = source_snapshot(backend)
    return {"snapshot": source, "worktree_fingerprint": worktree_fingerprint(backend, source["head"])}


def toolchain(backend: Path, run=subprocess.run) -> dict:
    result = {}
    for name, args in (("rustc", ["rustc", "-Vv"]), ("cargo", ["cargo", "-V"])):
        output = run(args, cwd=backend, check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                     creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
        result[name] = output.stdout.decode("utf-8").strip()
    return result


def write_new(path: Path, value: dict) -> None:
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def target_directory(backend: Path) -> Path:
    target = backend / "target/ci/backend"
    if any(linked(path) for path in (target, *target.parents)):
        raise ValueError("维护工具构建缓存不能经过链接")
    return target


def recorded_cargo_path(backend: Path, name: str, content: str) -> Path:
    """读取原始构建事件；验证冻结副本不依赖可随时重建的缓存文件仍存在。"""
    found = []
    manifest = backend / "crates/ryframe/Cargo.toml"
    for line in content.splitlines():
        event = json.loads(line)
        if (event.get("reason") == "compiler-artifact" and event.get("target", {}).get("name") == name
                and event.get("target", {}).get("kind") == ["bin"]
                and Path(event.get("manifest_path", "")).resolve() == manifest.resolve()
                and event.get("executable")):
            path = Path(event["executable"])
            if not path.is_absolute():
                raise ValueError("Cargo 产物事件必须使用实际绝对路径")
            found.append(path.resolve())
    if len(found) != 1:
        raise ValueError("维护工具必须恰好绑定一个当前 Workspace 的 Cargo 产物事件")
    return found[0]


def build(backend: Path, directory: Path, run=subprocess.run) -> dict:
    """Cargo JSON 决定实际产物；复制为本轮独立文件，后续其他构建不改变证据。"""
    directory = local_path(backend, str(directory), new=True)
    if not directory.parent.is_dir():
        raise ValueError("先明确创建构建证据父目录")
    target = target_directory(backend)
    directory.mkdir()
    artifacts, before, stage = {}, None, "source_binding"
    try:
        before = source_binding(backend)
        inventory = capture_inventory(backend, before["snapshot"])
        stage = "toolchain"
        versions = toolchain(backend, run)
        for role, (_, name) in TOOLS.items():
            stage = "build_" + role
            args = command(role, target)
            output, errors = directory / f"{role}.cargo.jsonl", directory / f"{role}.cargo.log"
            environment = {**os.environ, "RYFRAME_BUILD_COMMIT": before["snapshot"]["head"]}
            with output.open("xb") as stdout, errors.open("xb") as stderr:
                run(args, cwd=backend, stdout=stdout, stderr=stderr, check=True,
                    env=environment,
                    creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0)
            executable = cargo_artifact(backend, name, output.read_text(encoding="utf-8"))
            if not executable.is_relative_to(target.resolve()) or any(linked(p) for p in (executable, *executable.parents)):
                raise ValueError("Cargo 维护工具产物不属于本次固定 target")
            digest = file_digest(executable)
            saved = directory / (name + executable.suffix)
            with executable.open("rb") as source, saved.open("xb") as copy:
                shutil.copyfileobj(source, copy)
                copy.flush()
                os.fsync(copy.fileno())
            if file_digest(executable) != digest or file_digest(saved) != digest:
                raise ValueError("复制维护工具时二进制发生变化")
            if os.name != "nt":
                saved.chmod(0o700)
            artifacts[role] = {"executable": str(saved), "cargo_executable": str(executable), "command": args,
                               "cargo_output": {"file": output.name, **file_digest(output)},
                               "cargo_log": {"file": errors.name, **file_digest(errors)}, **digest}
        stage = "verify_source_and_toolchain"
        if source_binding(backend) != before or toolchain(backend, run) != versions:
            raise ValueError("维护工具构建期间源码或工具链变化")
        receipt = {"format_version": 1, "kind": "devex-clone-tool-build", "backend_root": str(backend),
                   "source": before, "source_inventory": inventory, "toolchain": versions, "target_directory": str(target),
                   "artifacts": artifacts, "restore_qualified": False, "resources_modified": False}
        stage = "publish_receipt"
        write_new(directory / "build.json", receipt)
        return receipt
    except Exception as error:
        write_new(directory / "failed.json", {"status": "failed", "error_type": type(error).__name__,
                                              "stage": stage, "source": before, "resources_modified": False,
                                              "stdout": redact_object_diagnostic(getattr(error, "stdout", b""), os.environ),
                                              "stderr": redact_object_diagnostic(getattr(error, "stderr", b""), os.environ)})
        raise


def verify_evidence(backend: Path, filename: Path, receipt: dict | None = None) -> dict:
    """只读核验固定维护构建收据、Cargo 事件及冻结二进制，不比较当前源码。"""
    filename = local_path(backend, str(filename))
    if filename.name != "build.json" or (filename.parent / "failed.json").exists():
        raise ValueError("维护工具构建失败或收据名称不匹配")
    before = file_digest(filename)
    observed = read_json(filename)
    if receipt is not None and observed != receipt:
        raise ValueError("维护工具构建收据与调用方绑定内容不同")
    receipt = observed
    exact(receipt, {"format_version", "kind", "backend_root", "source", "toolchain", "target_directory",
                    "artifacts", "restore_qualified", "resources_modified"}
          | ({"source_inventory"} if "source_inventory" in receipt else set()))
    target = target_directory(backend)
    if (receipt["format_version"] != 1 or receipt["kind"] != "devex-clone-tool-build"
            or receipt["backend_root"] != str(backend) or receipt["target_directory"] != str(target)
            or receipt["restore_qualified"] is not False or receipt["resources_modified"] is not False
            or set(receipt["artifacts"]) != set(TOOLS)):
        raise ValueError("维护工具构建证据格式或用途不符")
    for role, item in receipt["artifacts"].items():
        exact(item, {"executable", "cargo_executable", "command", "cargo_output", "cargo_log", "bytes", "sha256"})
        executable = local_path(backend, item["executable"])
        if executable.parent != filename.parent or item["command"] != command(role, target):
            raise ValueError("维护工具路径或定向构建命令不匹配")
        if file_digest(executable) != {key: item[key] for key in ("bytes", "sha256")}:
            raise ValueError("维护工具实际二进制与收据不一致")
        for field in ("cargo_output", "cargo_log"):
            binding = item[field]
            exact(binding, {"file", "bytes", "sha256"})
            if binding["file"] != f"{role}.cargo." + ("jsonl" if field == "cargo_output" else "log"):
                raise ValueError("维护工具日志路径与角色不匹配")
            log = local_path(backend, str(filename.parent / binding["file"]))
            if file_digest(log) != {key: binding[key] for key in ("bytes", "sha256")}:
                raise ValueError("维护工具构建日志发生变化")
        # 校验原始事件的归属和准确路径；缓存可能重建，因此不要求缓存副本仍与冻结工具相同。
        output = filename.parent / item["cargo_output"]["file"]
        found = recorded_cargo_path(backend, TOOLS[role][1], output.read_text(encoding="utf-8"))
        if found != Path(item["cargo_executable"]) or not found.is_relative_to(target.resolve()):
            raise ValueError("维护工具 Cargo 事件与实际构建路径不一致")
    if file_digest(filename) != before or read_json(filename) != receipt:
        raise ValueError("校验维护工具期间收据发生变化")
    return receipt


def verify(backend: Path, filename: Path, run=subprocess.run) -> dict:
    """只读重新校验当前开发源码、工具链、Cargo 事件及冻结的维护 CLI 字节。"""
    filename = local_path(backend, str(filename))
    before = file_digest(filename)
    receipt = verify_evidence(backend, filename)
    current = source_binding(backend)
    original = reusable_artifact_source(backend, receipt)
    if (receipt["source"] != (original or current)
            or receipt["toolchain"] != toolchain(backend, run)):
        raise ValueError("维护工具未绑定当前完整开发源码和工具链")
    if file_digest(filename) != before or source_binding(backend) != current:
        raise ValueError("校验维护工具期间源码或收据变化")
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build_command = commands.add_parser("build", help="定向构建并保存本轮维护 CLI，不调用服务")
    build_command.add_argument("--output-dir", required=True, type=Path)
    build_command.add_argument("--write", required=True, action="store_true")
    check = commands.add_parser("verify", help="只读验证本轮维护 CLI 与当前源码")
    check.add_argument("--receipt", required=True, type=Path)
    for child in (build_command, check):
        child.add_argument("--backend-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        backend = args.backend_dir.resolve(strict=True)
        receipt = build(backend, args.output_dir) if args.command == "build" else verify(backend, args.receipt)
        source = receipt["source"]
        print(json.dumps({"status": "built" if args.command == "build" else "verified",
                          "head": source["snapshot"]["head"], "clean": source["snapshot"]["clean"],
                          "worktree_fingerprint": source["worktree_fingerprint"],
                          "tools": list(receipt["artifacts"]), "restore_qualified": False, "resources_modified": False}))
        return 0
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError) as error:
        print(json.dumps({"status": "failed", "error_type": type(error).__name__}))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
