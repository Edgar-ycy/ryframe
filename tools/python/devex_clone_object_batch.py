"""有界并发捕获已停止源的对象；单对象证明完整保留，失败后等待本批退出。"""
from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import hashlib

from devex_clone_capture import capture_object, verify_capture
from restore_build import file_digest
from restore_reference_io import ExternalTools

WORKERS = 4


def ordered_batches(function, tasks: list, *, thread_name_prefix: str) -> list:
    """每批最多四项；结果保持输入顺序，失败等待全部在途退出且不启动下一批。"""
    results = []
    with ThreadPoolExecutor(max_workers=WORKERS, thread_name_prefix=thread_name_prefix) as executor:
        for offset in range(0, len(tasks), WORKERS):
            futures = [executor.submit(function, task) for task in tasks[offset:offset + WORKERS]]
            # 不使用 cancel 代替进程回收；异常离开 context 前等待本批所有实际调用完成。
            results.extend(future.result() for future in futures)
    return results


def capture_one(tools: ExternalTools, request: dict, bucket: dict, item: dict) -> tuple[dict, dict]:
    token = hashlib.sha256((bucket["bucket"] + ":" + item["key"]).encode()).hexdigest()
    output = tools.work / "objects" / token
    expected = {key: item[key] for key in ("bytes", "sha256")}
    captured = capture_object(tools, "source", bucket["bucket"], item["key"], output,
                              expected=expected, max_bytes=request["max_object_bytes"])
    verify_capture(tools, "source", bucket["bucket"], item["key"], output,
                   expected=expected, max_bytes=request["max_object_bytes"])
    artifact = {"file": (output / "object.bin").relative_to(tools.work).as_posix(), **expected}
    entry = {"key": item["key"], "artifact": artifact, "metadata": captured["metadata"],
             "capture": {"file": (output / "capture.json").relative_to(tools.work).as_posix(),
                         **file_digest(output / "capture.json")},
             "get_header_consistent": captured["get_header_consistent"]}
    return entry, artifact


def capture_objects(tools: ExternalTools, request: dict, inventory: dict) -> tuple[list, dict]:
    """最多四个独立证据目录同时写入；整批验证后才调度下一批，输出顺序固定。"""
    (tools.work / "objects").mkdir()
    buckets = sorted(inventory["objects"], key=lambda row: row["bucket"])
    tasks = [(bucket, item) for bucket in buckets
             for item in sorted(bucket["entries"], key=lambda row: row["key"])]
    rows = {bucket["bucket"]: [] for bucket in buckets}
    index = {}
    completed = ordered_batches(lambda task: capture_one(tools, request, *task), tasks,
                                thread_name_prefix="source-object")
    for (bucket, item), (entry, artifact) in zip(tasks, completed, strict=True):
        rows[bucket["bucket"]].append(entry)
        index[bucket["bucket"], item["key"].removeprefix(bucket["prefix"])] = artifact
    return [{"bucket": bucket["bucket"], "entries": rows[bucket["bucket"]]} for bucket in buckets], index
