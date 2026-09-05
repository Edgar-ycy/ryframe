"""在 Linux CI 的专属 cgroup 中显式验收 DevEx 内存；Cargo 始终以 runner 身份运行。"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
import uuid

ROOT = Path("/sys/fs/cgroup")
BACKEND = Path(__file__).resolve().parents[1]
CASE = "devex_memory_tests::linux_cgroup_covers_grandchildren_and_cleans_only_its_own_group"
ENVIRONMENT = {"PATH", "HOME", "CARGO_HOME", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN",
               "CARGO_INCREMENTAL", "CARGO_TERM_COLOR", "CARGO_NET_RETRY"}


def write_json(path: Path, value: dict, *, replace: bool = False) -> None:
    temporary = path.with_suffix(".pending") if replace else path
    with temporary.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())
    if replace:
        temporary.replace(path)


def read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def kernel_identity(path: Path) -> dict:
    stat = path.stat(follow_symlinks=False)
    return {"device": stat.st_dev, "inode": stat.st_ino}


def output_path(value: Path) -> Path:
    resolved = value.resolve()
    if not resolved.is_relative_to((BACKEND / ".local-tests").resolve()):
        raise ValueError("cgroup 证据必须在当前后端 .local-tests 内")
    for path in (value, *value.parents):
        if path.is_symlink():
            raise ValueError("cgroup 证据路径不能经过符号链接")
    return resolved


def validate_plan(plan: dict) -> Path:
    if set(plan) != {"format_version", "run_id", "attempt", "nonce", "source_sha", "uid", "gid", "root", "backend", "cargo", "environment"}:
        raise ValueError("cgroup 计划字段不完整")
    if (plan["format_version"] != 1 or any(not re.fullmatch(r"[1-9][0-9]{0,19}", plan[key]) for key in ("run_id", "attempt"))
            or not re.fullmatch(r"[a-f0-9]{32}", plan["nonce"])
            or not re.fullmatch(r"[a-f0-9]{40}", plan["source_sha"])
            or any(type(plan[key]) is not int or plan[key] <= 0 for key in ("uid", "gid"))):
        raise ValueError("必须使用精确 CI run/attempt 和非 root runner 身份")
    path = ROOT / f"ryframe-devex-ci-{plan['run_id']}-{plan['attempt']}-{plan['nonce']}"
    if (plan["root"] != str(path) or Path(plan["backend"]) != BACKEND
            or not Path(plan["cargo"]).is_absolute() or set(plan["environment"]) - ENVIRONMENT):
        raise ValueError("cgroup 计划路径、工具或环境范围不匹配")
    return path


def require_privilege(plan: dict) -> None:
    if (sys.platform != "linux" or os.geteuid() != 0
            or os.environ.get("SUDO_UID") != str(plan["uid"])
            or os.environ.get("SUDO_GID") != str(plan["gid"])):
        raise ValueError("仅允许原 runner 通过 sudo 调用有限 cgroup 操作")


def parent_state() -> dict:
    # 只读全局层级；不为通过测试启用全局 controller 或改限额。
    entries = Path("/proc/self/mountinfo").read_text(encoding="utf-8").splitlines()
    if not any(line.split(" - ")[0].split()[4] == str(ROOT)
               and line.split(" - ")[1].split()[0] == "cgroup2" for line in entries):
        raise ValueError("CI 必须具有明确挂载的 cgroup v2")
    enabled = (ROOT / "cgroup.subtree_control").read_text(encoding="utf-8").split()
    if "memory" not in enabled:
        raise ValueError("父 cgroup 未启用 memory，拒绝修改全局 controller 或降级测量")
    return {"identity": kernel_identity(ROOT), "controllers": sorted(enabled)}


def owned_root(plan: dict, output: Path) -> Path:
    root = validate_plan(plan)
    created = read_json(output / "created.json")
    if (created["root"] != str(root) or created["run_id"] != plan["run_id"] or created["attempt"] != plan["attempt"]
            or root.is_symlink() or kernel_identity(root) != created["identity"]):
        raise ValueError("cgroup 内核身份或本次创建证据改变，拒绝接管")
    return root


def prepare(plan: dict, output: Path) -> dict:
    root = validate_plan(plan)
    parent = parent_state()
    root.mkdir()  # 已有同名目录直接失败，不能接管或清空。
    write_json(output / "created.json", {"root": str(root), "run_id": plan["run_id"], "attempt": plan["attempt"],
                                        "identity": kernel_identity(root), "parent": parent})
    (root / "cgroup.subtree_control").write_text("+memory")
    for name in ("driver", "measure"):
        (root / name).mkdir()
    (root / "measure/cgroup.subtree_control").write_text("+memory")
    # runner 在 driver leaf 中启动；共同祖先的 cgroup.procs 可写，才能迁移到测量子树。
    for directory in (root, root / "measure"):
        for path in (directory, *(directory / name for name in ("cgroup.procs", "cgroup.threads", "cgroup.subtree_control"))):
            os.chown(path, plan["uid"], plan["gid"], follow_symlinks=False)
    return {"status": "prepared", "root": str(root), "identity": kernel_identity(root), "parent": parent,
            "global_controller_modified": False, "limits_modified": False}


def process_fact(pid: int) -> dict | None:
    try:
        base = Path("/proc") / str(pid)
        fields = (base / "stat").read_text(encoding="utf-8").rsplit(")", 1)[1].split()
        uid = next(line.split()[1] for line in (base / "status").read_text(encoding="utf-8").splitlines() if line.startswith("Uid:"))
        return {"pid": pid, "started": fields[19], "parent": int(fields[1]), "uid": int(uid)}
    except FileNotFoundError:
        return None


def directories(root: Path) -> list[Path]:
    result = [root]
    for directory, names, _files in os.walk(root, followlinks=False):
        for name in names:
            path = Path(directory) / name
            if path.is_symlink() or not path.resolve().is_relative_to(root.resolve()):
                raise ValueError("cgroup 子路径越界")
            result.append(path)
    return result


def processes(root: Path) -> dict[int, dict]:
    result = {}
    for directory in directories(root):
        try:
            members = (directory / "cgroup.procs").read_text(encoding="utf-8").split()
        except FileNotFoundError:
            if directory == root:
                raise
            continue  # Rust 可以在两次观察之间移除已空的测量子 cgroup。
        for value in members:
            fact = process_fact(int(value))
            if fact is not None:
                result[fact["pid"]] = fact
    return result


def register_children(current: dict[int, dict], registered: dict[str, dict], uid: int) -> None:
    for pid, fact in current.items():
        known = registered.get(str(pid))
        if fact["uid"] != uid or (known and (known["started"] != fact["started"] or known["uid"] != uid)):
            raise ValueError("专属 cgroup 中出现未登记身份或复用 PID")
    pending = dict(current)
    while pending:
        changed = False
        for pid, fact in list(pending.items()):
            known = registered.get(str(pid))
            parent = registered.get(str(fact["parent"]))
            live_parent = current.get(fact["parent"])
            if known or (parent and live_parent and live_parent["started"] == parent["started"]
                         and live_parent["uid"] == parent["uid"] == uid):
                registered[str(pid)] = fact
                del pending[pid]
                changed = True
        if not changed:
            # 短命父进程可能先退出；不猜测归属。若未知进程留到清理阶段，必须拒绝终止。
            return


def cargo_command(plan: dict) -> list[str]:
    return [plan["cargo"], "test", "--locked", "-p", "xtask", "--no-default-features", "--test", "internal",
            "--target-dir", str(BACKEND / "target/ci/devex-cgroup"), "--jobs", "2", "--", "--exact", CASE,
            "--ignored", "--nocapture", "--test-threads=1"]


def validate_test_output(code: int, text: str) -> None:
    if (code != 0 or f"test {CASE} ... ok" not in text
            or not re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored;", text)):
        raise ValueError("cgroup 实机用例未恰好通过一次；零测试、ignored 或失败均不算验收")


def run_test(plan: dict, output: Path) -> dict:
    root = owned_root(plan, output)
    (root / "driver/cgroup.procs").write_text("0")
    os.setgroups([])
    os.setgid(plan["gid"])
    os.setuid(plan["uid"])
    if os.getuid() != plan["uid"] or os.geteuid() != plan["uid"] or os.getgid() != plan["gid"]:
        raise ValueError("Cargo 启动前未永久降权为原 runner")
    environment = {**plan["environment"], "RYFRAME_DEVEX_CGROUP_ROOT": str(root / "measure"), "CARGO_INCREMENTAL": "0"}
    command = cargo_command(plan)
    launcher = process_fact(os.getpid())
    if launcher is None:
        raise ValueError("无法登记本次 launcher 的内核创建身份")
    registry = {str(os.getpid()): launcher}
    write_json(output / "processes.json", registry)
    write_json(output / "test-start.json", {"command": command, "uid": os.getuid(), "gid": os.getgid(),
                                            "launcher": launcher, "cgroup": str(root / "driver"), "root_identity": kernel_identity(root)})
    with (output / "cargo.log").open("xb") as log:
        process = subprocess.Popen(command, cwd=BACKEND, env=environment, stdout=log, stderr=subprocess.STDOUT)
        child = process_fact(process.pid)
        if child is not None:
            registry[str(child["pid"])] = child
        write_json(output / "processes.json", registry, replace=True)
        deadline = time.monotonic() + 900
        while process.poll() is None:
            register_children(processes(root), registry, plan["uid"])
            write_json(output / "processes.json", registry, replace=True)
            if time.monotonic() >= deadline:
                raise TimeoutError("定向 cgroup Cargo 测试超过 15 分钟；保留证据并精确清理已登记进程")
            time.sleep(0.05)
        code = process.wait()
    text = (output / "cargo.log").read_text(encoding="utf-8", errors="replace")
    validate_test_output(code, text)
    if set(path.name for path in (root / "measure").iterdir() if path.is_dir()):
        raise ValueError("Rust 测试后测量子 cgroup 未清理")
    return {"status": "passed", "case": CASE, "exit_code": code, "uid": os.getuid(), "gid": os.getgid(),
            "cargo_log_sha256": hashlib.sha256((output / "cargo.log").read_bytes()).hexdigest()}


def verified_processes(current: dict[int, dict], registered: dict[str, dict], uid: int) -> None:
    for pid, fact in current.items():
        known = registered.get(str(pid))
        if not known or fact["uid"] != uid or fact["started"] != known["started"]:
            raise ValueError("清理发现未登记或已复用的进程，拒绝终止并保留 cgroup")


def freeze(root: Path) -> dict:
    (root / "cgroup.freeze").write_text("1")
    deadline = time.monotonic() + 5
    while True:
        events = dict(line.split() for line in (root / "cgroup.events").read_text(encoding="utf-8").splitlines())
        if events.get("frozen") == "1":
            return events
        if events.get("frozen") != "0" or time.monotonic() >= deadline:
            raise ValueError("本 job cgroup 未确认冻结，拒绝在进程可能继续 fork 时清理")
        time.sleep(0.05)


def cleanup(plan: dict, output: Path) -> dict:
    root = validate_plan(plan)
    previous = output / "cleanup.json"
    if previous.exists():
        result = read_json(previous)
        if (root.exists() or result.get("root") != str(root)
                or result.get("run_id") != plan["run_id"] or result.get("attempt") != plan["attempt"]):
            raise ValueError("清理收据与当前资源状态不一致")
        if result.get("status") == "not_created":
            if (output / "created.json").exists():
                raise ValueError("未创建收据与已有创建证据冲突")
            return result
        created = read_json(output / "created.json")
        if (result.get("status") != "cleaned" or root.exists() or result.get("root") != str(root)
                or result.get("run_id") != plan["run_id"] or result.get("attempt") != plan["attempt"]
                or result.get("identity") != created.get("identity") or created.get("root") != str(root)):
            raise ValueError("清理收据与当前资源状态不一致")
        return result
    if not (output / "created.json").exists():
        if root.exists():
            raise ValueError("没有本次创建证据，拒绝清理已有 cgroup")
        return {"status": "not_created", "root": str(root)}
    root = owned_root(plan, output)
    frozen = freeze(root)
    current = processes(root)
    registry = read_json(output / "processes.json") if (output / "processes.json").exists() else {}
    register_children(current, registry, plan["uid"])
    write_json(output / f"cleanup-observation-{uuid.uuid4().hex}.json", {
        "root": str(root), "identity": kernel_identity(root), "frozen_events": frozen,
        "current": list(current.values()), "registered": registry})
    verified_processes(current, registry, plan["uid"])
    if current:
        (root / "cgroup.kill").write_text("1")
    deadline = time.monotonic() + 5
    while processes(root):
        if time.monotonic() >= deadline:
            raise TimeoutError("本 job 已登记后代未按期退出，保留精确 cgroup")
        time.sleep(0.05)
    if parent_state() != read_json(output / "created.json")["parent"]:
        raise ValueError("父 cgroup 状态在验收期间变化，拒绝完整通过")
    for directory in sorted(directories(root), key=lambda path: len(path.parts), reverse=True):
        directory.rmdir()
    return {"status": "cleaned", "root": str(root), "identity": read_json(output / "created.json")["identity"],
            "terminated_registered_processes": list(current.values()), "root_absent": not root.exists()}


def privileged(phase: str, plan: dict, output: Path) -> dict:
    require_privilege(plan)
    try:
        result = {"prepare": prepare, "test": run_test, "cleanup": cleanup}[phase](plan, output)
        destination = output / f"{phase}.json"
        if not destination.exists():
            write_json(destination, {"run_id": plan["run_id"], "attempt": plan["attempt"], **result})
        if phase == "cleanup" and result.get("terminated_registered_processes"):
            raise ValueError("测试结束仍有已登记后代；虽已精确回收，仍拒绝完整通过")
        return result
    except BaseException as error:
        write_json(output / f"{phase}-failure-{uuid.uuid4().hex}.json", {
            "run_id": plan["run_id"], "attempt": plan["attempt"], "root": plan["root"],
            "error_type": type(error).__name__, "error": str(error), "status": "failed"})
        raise


def sudo_phase(phase: str, output: Path) -> None:
    command = ["sudo", "-n", sys.executable, str(Path(__file__).resolve()), f"_{phase}", "--output", str(output)]
    with (output / f"{phase}-controller-{uuid.uuid4().hex}.log").open("xb") as log:
        subprocess.run(command, check=True, stdout=log, stderr=subprocess.STDOUT, timeout=960 if phase == "test" else 30)


def execute(output: Path) -> None:
    if sys.platform != "linux" or os.environ.get("GITHUB_ACTIONS") != "true" or os.geteuid() == 0:
        raise ValueError("仅允许 Linux CI 非 root runner 显式运行")
    output.mkdir(parents=True, exist_ok=False)
    try:
        execute_created(output)
    except BaseException as error:
        write_json(output / f"runner-failure-{uuid.uuid4().hex}.json", {
            "status": "failed", "error_type": type(error).__name__, "error": str(error)})
        raise


def execute_created(output: Path) -> None:
    run_id, attempt, nonce = os.environ.get("GITHUB_RUN_ID", ""), os.environ.get("GITHUB_RUN_ATTEMPT", ""), uuid.uuid4().hex
    cargo = shutil.which("cargo")
    if not cargo:
        raise ValueError("缺少固定 Rust 工具链的 Cargo")
    source_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=BACKEND, text=True).strip()
    if source_sha != os.environ.get("GITHUB_SHA"):
        raise ValueError("实际 checkout 与本次 GitHub SHA 不匹配")
    plan = {"format_version": 1, "run_id": run_id, "attempt": attempt, "nonce": nonce,
            "source_sha": source_sha,
            "uid": os.getuid(), "gid": os.getgid(), "backend": str(BACKEND), "cargo": str(Path(cargo).absolute()),
            "root": str(ROOT / f"ryframe-devex-ci-{run_id}-{attempt}-{nonce}"),
            "environment": {name: os.environ[name] for name in ENVIRONMENT if name in os.environ}}
    validate_plan(plan)
    write_json(output / "plan.json", plan)
    try:
        sudo_phase("prepare", output)
        sudo_phase("test", output)
    finally:
        sudo_phase("cleanup", output)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("run", "cleanup", "_prepare", "_test", "_cleanup"))
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = output_path(args.output)
    if args.operation == "run":
        execute(output)
    elif args.operation == "cleanup":
        if (output / "plan.json").exists():
            sudo_phase("cleanup", output)
    else:
        plan = read_json(output / "plan.json")
        validate_plan(plan)
        privileged(args.operation.removeprefix("_"), plan, output)


if __name__ == "__main__":
    main()
