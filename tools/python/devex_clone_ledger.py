"""开发复制的持久步骤账本；不执行 SQL、对象请求、自动重放或资源清理。"""
from __future__ import annotations

import copy
import datetime as dt
import json
import os
from pathlib import Path
import re
import uuid

from devex_clone_model import digest, exact, local_path, name
from devex_clone_copy_locks import CopyOwner, GUARDS, read_copy_owner, record_copy_lock, release_copy_lock
from process_guard import process_guard
from restore_reference_plan import plan_hash
from restore_reference_plan import BUCKETS
from process_sockets import endpoint

MAX_BYTES = 16 * 1024 * 1024
ZERO = "0" * 64


class LedgerError(ValueError):
    """账本证据不完整，需要显式核对；错误不携带业务内容。"""


def encoded(value: dict) -> bytes:
    return (json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False) + "\n").encode()


def checked_json(raw: bytes) -> dict:
    try:
        value = json.loads(raw)
        if not isinstance(value, dict) or encoded(value) != raw:
            raise LedgerError("账本 JSON 非规范格式、重复字段或已截断")
        return value
    except (ValueError, UnicodeError, TypeError):
        raise LedgerError("账本 JSON 无效，保留原文件等待核对") from None


def resource_digest(generation_sha256: str, resource: dict) -> str:
    """将当前初始化代次与精确物理资源绑定，调用方另核验对应实际 ownership。"""
    digest(generation_sha256)
    name(resource.get("scope_id"))
    if resource.get("kind") == "database":
        exact(resource, {"kind", "scope_id", "server_uuid", "database"})
        if not re.fullmatch(r"[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}", resource["server_uuid"]) or not re.fullmatch(r"[a-z0-9_]{1,64}", resource["database"]):
            raise LedgerError("数据库物理身份无效")
    elif resource.get("kind") == "object":
        exact(resource, {"kind", "scope_id", "endpoint", "bucket", "key"})
        endpoint(resource["endpoint"])
        logical = resource["key"].removeprefix(resource["scope_id"] + "/")
        if (resource["bucket"] not in BUCKETS or logical == resource["key"]
                or any(part in {"", ".", ".."} for part in logical.split("/")) or logical == ".ryframe-owner"
                or "\\" in logical or any(ord(char) < 32 or ord(char) == 127 for char in logical)
                or len(resource["key"].encode("utf-8")) > 1024):
            raise LedgerError("对象物理身份必须是明确 scope 内的业务 key")
    else:
        raise LedgerError("资源类型无效")
    return plan_hash({"generation_sha256": generation_sha256, **resource})


def image(value: dict) -> dict:
    """摘要由调用方真实观察者计算；身份摘要必须含 scope、generation 及精确物理身份。"""
    if not isinstance(value, dict):
        raise LedgerError("前后像必须是明确对象")
    kind = value.get("kind")
    if kind == "database":
        exact(value, {"kind", "resource_sha256", "schema_sha256", "tables_sha256", "preserved_sha256"})
        for field in set(value) - {"kind"}:
            digest(value[field])
    elif kind == "object":
        exact(value, {"kind", "resource_sha256", "exists", "bytes", "sha256", "metadata_sha256"})
        digest(value["resource_sha256"])
        if type(value["exists"]) is not bool:
            raise LedgerError("对象存在性必须来自明确观察，不能把错误当作缺失")
        if value["exists"]:
            if type(value["bytes"]) is not int or value["bytes"] < 0:
                raise LedgerError("对象长度无效")
            digest(value["sha256"])
            digest(value["metadata_sha256"])
        elif any(value[field] is not None for field in ("bytes", "sha256", "metadata_sha256")):
            raise LedgerError("不存在的对象不能携带虚构内容摘要")
    else:
        raise LedgerError("账本只支持明确数据库或对象步骤")
    return copy.deepcopy(value)


def image_pair(before: dict, after: dict) -> None:
    image(before)
    image(after)
    stable = {"kind", "resource_sha256"}
    if before["kind"] == "database":
        stable |= {"schema_sha256", "preserved_sha256"}
    if any(before.get(field) != after.get(field) for field in stable) or before == after:
        raise LedgerError("前后像身份、schema、保留数据不同或无法唯一判定")
    if before["kind"] == "object" and (before["exists"] or not after["exists"]):
        raise LedgerError("对象步骤必须从明确不存在到条件创建，不允许覆盖")


def project(events: list[dict]) -> dict:
    """严格回放账本事件，只重建证据状态，不重放任何资源操作。"""
    state = {"steps": {}, "session": None, "sessions": set()}
    for index, event in enumerate(events):
        kind = event.get("type")
        if index == 0:
            exact(event, {"type", "plan_sha256", "generation_sha256"})
            if kind != "created":
                raise LedgerError("账本缺少创建绑定")
            digest(event["plan_sha256"])
            digest(event["generation_sha256"])
        elif kind == "opened":
            exact(event, {"type", "session", "mode"})
            name(event["session"])
            if event["mode"] not in {"apply", "reconcile", "resume"} or event["session"] in state["sessions"]:
                raise LedgerError("账本会话类型无效或重复")
            previous = state["session"]
            if previous and previous["outcome"] != "partial" and event["mode"] != "reconcile":
                raise LedgerError("未完成或已封存账本只能显式核对")
            state["sessions"].add(event["session"])
            state["session"] = {"id": event["session"], "mode": event["mode"], "outcome": "open", "touched": set(), "completed_at": None}
        elif kind in {"intent", "confirmed", "reconciled", "resumed"}:
            session = state["session"]
            if session is None or session["outcome"] != "open":
                raise LedgerError("步骤事件不在持锁执行会话内")
            apply_step(state, event)
        elif kind == "closed":
            exact(event, {"type", "outcome", "completed_at"})
            if not state["session"] or state["session"]["outcome"] != "open" or event["outcome"] not in {"partial", "failed", "publishing"}:
                raise LedgerError("未知或重复的会话关闭事件")
            if event["outcome"] == "publishing":
                instant = dt.datetime.fromisoformat(event["completed_at"])
                if instant.tzinfo is None:
                    raise LedgerError("账本完成时间必须包含时区")
            elif event["completed_at"] is not None:
                raise LedgerError("非完成步骤不能声明完成时间")
            state["session"]["outcome"] = event["outcome"]
            state["session"]["completed_at"] = event["completed_at"]
        elif kind == "fault":
            exact(event, {"type", "stage"})
            if not state["session"] or event["stage"] not in {"lock_release", "receipt_write"}:
                raise LedgerError("未知账本失败事件")
            state["session"]["outcome"] = "failed"
        else:
            raise LedgerError("未知账本事件，拒绝忽略或兼容解码")
    return state


def apply_step(state: dict, event: dict) -> None:
    kind, session, steps = event["type"], state["session"], state["steps"]
    step_id = name(event.get("step_id"))
    if kind == "intent":
        exact(event, {"type", "step_id", "before", "after", "artifact_sha256"})
        if session["mode"] == "reconcile" or step_id in steps:
            raise LedgerError("核对会话不能写资源；重复步骤不能自动重放")
        if any(step["phase"] != "confirmed" for step in steps.values()):
            raise LedgerError("既有步骤未确认，禁止开始下一资源写入")
        image_pair(event["before"], event["after"])
        digest(event["artifact_sha256"])
        if any(step["before"]["resource_sha256"] == event["before"]["resource_sha256"] for step in steps.values()):
            raise LedgerError("不同步骤重复引用同一物理资源")
        steps[step_id] = {**copy.deepcopy(event), "phase": "unknown", "attempt": 1, "session": session["id"]}
    else:
        exact(event, {"type", "step_id", "observed"})
        if step_id not in steps:
            raise LedgerError("没有本账本 intent，不能接管已有资源")
        step = steps[step_id]
        observed = image(event["observed"])
        if kind == "confirmed":
            if session["mode"] == "reconcile" or step["phase"] != "unknown" or step["session"] != session["id"] or observed != step["after"]:
                raise LedgerError("确认必须对应本次写入 intent 和完整预期后像")
            step["phase"] = "confirmed"
        elif kind == "reconciled":
            if session["mode"] != "reconcile" or step_id in session["touched"]:
                raise LedgerError("只允许独立显式核对，不重复消费同一观察")
            step["phase"] = ("confirmed" if observed == step["after"] else
                             "reconciled_before" if observed == step["before"] else "mismatch")
            step["session"] = session["id"]
        else:
            if session["mode"] != "resume" or step["phase"] != "reconciled_before" or observed != step["before"]:
                raise LedgerError("显式续跑必须重新证明同代次完整前像")
            if any(other["phase"] in {"unknown", "mismatch"} for key, other in steps.items() if key != step_id):
                raise LedgerError("其他步骤结果未知或不匹配，禁止开始下一次续跑")
            step.update(phase="unknown", attempt=step["attempt"] + 1, session=session["id"])
    session["touched"].add(step_id)


class CloneLedger:
    def __init__(self, backend: Path, directory: Path, plan_sha256: str, generation_sha256: str,
                 *, create=False, mode="apply", copy_owner: CopyOwner | None = None):
        self.backend = backend.resolve()
        self.root = local_path(self.backend, str(directory), new=create)
        self.binding = {"plan_sha256": digest(plan_sha256), "generation_sha256": digest(generation_sha256)}
        if mode not in {"apply", "reconcile", "resume"} or create and mode != "apply":
            raise LedgerError("账本打开模式无效")
        self.create, self.mode = create, mode
        self.session_id = "session-" + uuid.uuid4().hex
        self.copy_owner = copy_owner
        if copy_owner is not None:
            owner, paths = read_copy_owner(copy_owner)
            if (copy_owner.backend != self.backend or paths["ledger"] != self.root / "clone.lock"
                    or any(owner[key] != value for key, value in self.binding.items())):
                raise LedgerError("复制账本与锁持有人来源或目标不匹配")
            self.session_id = copy_owner.session_id
        self.events, self.frames = [], []
        self.active = self.poisoned = self.finishing = False
        self.receipt = None

    def path(self, filename: str) -> Path:
        return local_path(self.backend, str(self.root / filename))

    def _write(self, path: Path, value: dict, *, replace=False) -> None:
        temporary = self.path(path.name + "." + uuid.uuid4().hex + ".tmp")
        with temporary.open("xb") as stream:
            stream.write(encoded(value))
            stream.flush()
            os.fsync(stream.fileno())
        if replace:
            os.replace(temporary, path)
        else:
            if path.exists():
                raise LedgerError("账本新收据不能覆盖既有文件")
            # rename 在 Unix 可覆盖竞态出现的文件；同目录硬链接提供原子排他发布。
            os.link(temporary, path)
            temporary.unlink()

    def _read(self) -> tuple[list, list]:
        journal, anchor = self.path("events.ndjson"), self.path("head.json")
        if journal.stat().st_size > MAX_BYTES or anchor.stat().st_size > 4096:
            raise LedgerError("账本超出固定读取上限")
        raw = journal.read_bytes()
        if not raw or not raw.endswith(b"\n"):
            raise LedgerError("账本已截断，禁止自动修复")
        frames, events, previous = [], [], ZERO
        for index, line in enumerate(raw.splitlines(keepends=True)):
            frame = checked_json(line)
            exact(frame, {"sequence", "previous", "event", "sha256"})
            body = {key: frame[key] for key in ("sequence", "previous", "event")}
            if type(frame["sequence"]) is not int or frame["sequence"] != index or frame["previous"] != previous or frame["sha256"] != plan_hash(body):
                raise LedgerError("账本事件顺序或摘要链被篡改")
            previous = frame["sha256"]
            frames.append(frame)
            events.append(frame["event"])
        expected = {"format_version": 1, **self.binding, "sequence": len(frames) - 1, "sha256": previous}
        if checked_json(anchor.read_bytes()) != expected or events[0] != {"type": "created", **self.binding}:
            raise LedgerError("计划、代次或账本 head 不匹配；禁止自动截断补写")
        project(events)
        return frames, events

    def _append(self, event: dict) -> None:
        if self.poisoned:
            raise LedgerError("账本写入结果未知，必须停止并保留证据")
        try:
            if not self.active or self.path("clone.lock").stat().st_ino != self.lock_identity:
                raise LedgerError("账本修改必须持有同一排他锁")
            if self.frames and self._read()[0] != self.frames:
                raise LedgerError("持锁期间账本发生变化")
            project([*self.events, event])
            body = {"sequence": len(self.frames), "previous": self.frames[-1]["sha256"] if self.frames else ZERO, "event": event}
            frame = {**body, "sha256": plan_hash(body)}
            journal, raw = self.path("events.ndjson"), encoded(frame)
            if (journal.stat().st_size if self.frames else 0) + len(raw) > MAX_BYTES:
                raise LedgerError("新增事件将超过账本上限，不能开始资源写入")
            with journal.open("ab" if self.frames else "xb") as stream:
                stream.write(raw)
                stream.flush()
                os.fsync(stream.fileno())
            self._write(self.path("head.json"), {"format_version": 1, **self.binding,
                        "sequence": frame["sequence"], "sha256": frame["sha256"]}, replace=bool(self.frames))
            self.frames.append(frame)
            self.events.append(copy.deepcopy(event))
        except BaseException:
            self.poisoned = True
            raise

    def __enter__(self):
        if self.create:
            self.root.mkdir(exist_ok=False)
        if not self.root.is_dir():
            raise LedgerError("账本目录不存在")
        if self.path("clone.lock").exists():
            raise LedgerError("复制账本被锁定或上次异常退出，禁止自动抢锁")
        self.guard = process_guard(self.root, GUARDS["ledger"])
        try:
            self.guard.__enter__()
        except ValueError as error:
            raise LedgerError("复制账本被锁定或正在恢复，禁止并发操作") from error
        try:
            return self._open_locked()
        except BaseException:
            self.guard.__exit__(None, None, None)
            raise

    def _open_locked(self):
        lock = self.path("clone.lock")
        try:
            lock.mkdir(exist_ok=False)
        except FileExistsError:
            raise LedgerError("复制账本被锁定或上次异常退出，禁止自动抢锁") from None
        self.lock_identity = lock.stat().st_ino
        self.active = True
        try:
            if self.copy_owner is not None:
                record_copy_lock(lock, "ledger", self.copy_owner)
            if self.create:
                self._append({"type": "created", **self.binding})
            else:
                self.frames, self.events = self._read()
                state = project(self.events)
                if self.mode != "reconcile" and any(step["phase"] in {"unknown", "mismatch"} for step in state["steps"].values()):
                    raise LedgerError("未知结果必须先显式核对，不能继续写入")
            self._append({"type": "opened", "session": self.session_id, "mode": self.mode})
            return self
        except BaseException as error:
            try:
                self._release()
            except Exception as release:
                raise BaseExceptionGroup("账本打开与锁释放均失败", [error, release]) from None
            raise

    def _release(self) -> None:
        lock = self.path("clone.lock")
        if not self.active or lock.stat().st_ino != self.lock_identity:
            raise LedgerError("账本锁身份变化，拒绝释放其他执行者的锁")
        if self.copy_owner is not None:
            release_copy_lock(lock, "ledger", self.copy_owner)
        else:
            lock.rmdir()
        self.active = False

    def _fault(self, stage: str) -> None:
        reacquired = False
        try:
            if not self.active:
                lock = self.path("clone.lock")
                lock.mkdir(exist_ok=False)
                self.lock_identity, self.active, reacquired = lock.stat().st_ino, True, True
                if self.copy_owner is not None:
                    record_copy_lock(lock, "ledger", self.copy_owner)
            self._append({"type": "fault", "stage": stage})
        except Exception:
            self.poisoned = True
        finally:
            if reacquired:
                try:
                    self._release()
                except Exception:
                    self.poisoned = True

    def __exit__(self, _type, error, _traceback):
        try:
            return self._exit_locked(error)
        finally:
            self.guard.__exit__(None, None, None)

    def _exit_locked(self, error):
        failures = []
        if not self.poisoned:
            try:
                outcome = "failed" if error else "publishing" if self.finishing else "partial"
                self._append({"type": "closed", "outcome": outcome,
                              "completed_at": dt.datetime.now(dt.timezone.utc).isoformat() if outcome == "publishing" else None})
            except Exception as failure:
                failures.append(failure)
        try:
            self._release()
        except Exception as failure:
            self._fault("lock_release")
            failures.append(failure)
        if error is None and not failures and not self.poisoned and self.finishing:
            try:
                self._publish()
            except Exception as failure:
                self._fault("receipt_write")
                failures.append(failure)
        if failures:
            raise BaseExceptionGroup("复制账本未完成，保留原始失败与收尾失败", ([error] if error else []) + failures)
        if self.poisoned and error is None:
            raise LedgerError("账本写入结果未知，不能将已捕获异常当作执行成功")
        return False

    def _step(self, event: dict) -> None:
        if not self.active or self.finishing:
            raise LedgerError("步骤必须在持锁且尚未收尾的账本会话中")
        self._append(event)

    def intent(self, step_id: str, before: dict, after: dict, artifact_sha256: str) -> None:
        self._step({"type": "intent", "step_id": step_id, "before": before, "after": after, "artifact_sha256": artifact_sha256})

    def confirm(self, step_id: str, observed_after: dict) -> None:
        self._step({"type": "confirmed", "step_id": step_id, "observed": observed_after})

    def reconcile(self, step_id: str, observed: dict) -> str:
        self._step({"type": "reconciled", "step_id": step_id, "observed": observed})
        return project(self.events)["steps"][step_id]["phase"]

    def resume(self, step_id: str, observed_before: dict) -> None:
        self._step({"type": "resumed", "step_id": step_id, "observed": observed_before})

    def finish(self) -> None:
        state = project(self.events)
        if not self.active or self.poisoned or self.finishing or not state["steps"] or any(step["phase"] != "confirmed" for step in state["steps"].values()):
            raise LedgerError("账本仍有未确认步骤，不能生成完成证据")
        if self.mode == "reconcile" and set(state["steps"]) != state["session"]["touched"]:
            raise LedgerError("未知会话收尾必须重新核对全部既有步骤")
        self.finishing = True

    def _publish(self) -> None:
        if self.path("clone.lock").exists() or self._read()[0] != self.frames:
            raise LedgerError("锁未释放或收尾期间账本变化")
        value = self._completion()
        path = self.path("complete-" + self.session_id + ".json")
        self._write(path, value)
        if self.path("clone.lock").exists() or self._read()[0] != self.frames:
            raise LedgerError("完成收据发布期间账本变化，不能作为完成证据")
        self.receipt = value

    def _completion(self) -> dict:
        state = project(self.events)
        return {"format_version": 1, "status": "ledger_evidence_complete", **self.binding,
                "session": state["session"]["id"], "journal_sha256": self.frames[-1]["sha256"],
                "steps": len(state["steps"]), "completed_at": state["session"]["completed_at"],
                "target_ready": False, "restore_success": False, "runtime_performance_passed": False}

    def inspect(self) -> dict:
        """只读重验原完成收据；保留原时间，不将检查时间写成新的完成时间。"""
        if self.active:
            raise LedgerError("活动会话只能查看当前 snapshot")
        self.frames, self.events = self._read()
        self.receipt = None
        state = project(self.events)
        if state["session"]["outcome"] == "publishing" and not self.path("clone.lock").exists():
            receipt = self.path("complete-" + state["session"]["id"] + ".json")
            if receipt.exists():
                value = checked_json(receipt.read_bytes())
                if value != self._completion() or any(step["phase"] != "confirmed" for step in state["steps"].values()):
                    raise LedgerError("完成收据与账本后像或计划代次不一致")
                self.receipt = value
        return self.snapshot()

    def snapshot(self) -> dict:
        state = project(self.events)
        uncertain = (self.poisoned or self.path("clone.lock").exists()
                     or state["session"]["outcome"] in {"open", "failed", "publishing"}
                     or any(step["phase"] != "confirmed" for step in state["steps"].values()))
        return {**self.binding, "status": "ledger_evidence_complete" if self.receipt else "needs_reconciliation" if uncertain else "partial",
                "steps": copy.deepcopy(state["steps"]), "target_ready": False,
                "completed_at": self.receipt["completed_at"] if self.receipt else None}
