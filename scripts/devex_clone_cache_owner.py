"""只恢复原初始化的两个验收标记；未知响应先核对，不恢复会话或锁。"""
import re
from types import SimpleNamespace

from devex_clone_capture import read_json, write_json
from devex_clone_model import exact
from devex_clone_run_state import binding
from devex_clone_source_proof import bound_file
from devex_clone_target_resources import Resources


def transport(request, environment):
    resource = SimpleNamespace(request={"storage": {"redis": request["previous"]}}, environment=environment)
    resource._response = Resources._response
    return lambda parts: Resources._redis(resource, parts)


def images(markers):
    before = {"keys": [], "owner": None, "sentinel": None}
    after = {"keys": [markers["ownership_key"]], "owner": markers["ownership_value"],
             "sentinel": markers["sentinel_value"]}
    return before, after


def classify(markers, observed):
    before, after = images(markers)
    return "before" if observed == before else "after" if observed == after else "mismatch"


def capture(call, markers):
    """只扫描已登记 namespace，并读取一个明确的命名空间外 sentinel。"""
    cursor, seen, found = "0", set(), set()
    for _ in range(10000):
        response = call(["SCAN", cursor, "MATCH", markers["namespace"] + "*", "COUNT", "1000"])
        if (not isinstance(response, list) or len(response) != 2 or not isinstance(response[0], str)
                or not response[0].isdigit() or not isinstance(response[1], list)):
            raise ValueError("Redis scoped SCAN 响应结构不同")
        cursor, keys = response
        if any(not isinstance(key, str) or not key.startswith(markers["namespace"]) for key in keys):
            raise ValueError("Redis scoped SCAN 返回越界键")
        found.update(keys)
        if len(found) > 10000:
            raise ValueError("Redis namespace 超出验收观察上限")
        if cursor == "0":
            break
        if cursor in seen:
            raise ValueError("Redis scoped SCAN 游标循环")
        seen.add(cursor)
    else:
        raise ValueError("Redis scoped SCAN 未完成")
    return {"keys": sorted(found), "owner": call(["GET", markers["ownership_key"]]),
            "sentinel": call(["GET", markers["sentinel_key"]])}


def load_intent(backend, path, request, runtime):
    bound_file(backend, runtime)
    value = read_json(path / "intent.json")
    before = bound_file(backend, value["before"])
    if before != path / "before.json" or read_json(before) != images(request["markers"])[0]:
        raise ValueError("缓存标记意图缺少当前写入的完整空前像")
    expected = {"request": request["binding"], "runtime": runtime, "before": binding(before),
                "operation": "MSETNX", "markers": request["markers"], "automatic_retry": False}
    if value != expected:
        raise ValueError("缓存标记意图不属于固定请求和实际 Redis 代次")
    return binding(path / "intent.json")


def confirmation(backend, path, request, runtime):
    intent = load_intent(backend, path, request, runtime)
    result = read_json(path / "confirmed.json")
    exact(result, {"intent", "observed", "response", "reconciled"})
    observed = bound_file(backend, result["observed"])
    if (result["intent"] != intent or observed.parent != path
            or classify(request["markers"], read_json(observed)) != "after"):
        raise ValueError("缓存标记确认不是当前意图的完整后像")
    if result["response"] is not None:
        response = bound_file(backend, result["response"])
        if response != path / "response.json" or read_json(response) != {"result": 1} or result["reconciled"] is not False:
            raise ValueError("缓存标记原子写入响应证据不符")
    else:
        if result["reconciled"] is not True or not re.fullmatch(r"observed-[0-9]{4}\.json", observed.name):
            raise ValueError("缓存标记确认缺少响应或显式核对证据")
        proof = read_json(path / observed.name.replace("observed-", "reconcile-"))
        if proof != {"intent": intent, "observed": result["observed"], "state": "after"}:
            raise ValueError("缓存标记确认不是同次完整后像核对")
    return binding(path / "confirmed.json")


def apply(backend, path, request, runtime, call, guard):
    path.mkdir()
    guard()
    before = capture(call, request["markers"])
    write_json(path / "before.json", before)
    if classify(request["markers"], before) != "before":
        raise ValueError("缓存恢复写入前 namespace 或 sentinel 不是原空前像")
    guard()
    write_json(path / "intent.json", {"request": request["binding"], "runtime": runtime,
        "before": binding(path / "before.json"), "operation": "MSETNX", "markers": request["markers"],
        "automatic_retry": False})
    markers = request["markers"]
    response = call(["MSETNX", markers["ownership_key"], markers["ownership_value"],
                     markers["sentinel_key"], markers["sentinel_value"]])
    write_json(path / "response.json", {"result": response})
    if type(response) is not int or response != 1:
        raise ValueError("缓存验收标记 MSETNX 未明确成功；先核对，禁止重放")
    guard()
    observed = capture(call, markers)
    write_json(path / "after.json", observed)
    if classify(markers, observed) != "after":
        raise ValueError("缓存验收标记后像不匹配；必须显式核对")
    guard()
    write_json(path / "confirmed.json", {"intent": binding(path / "intent.json"),
        "observed": binding(path / "after.json"), "response": binding(path / "response.json"), "reconciled": False})
    return confirmation(backend, path, request, runtime)


def reconcile(backend, path, request, runtime, call, guard, number):
    intent = load_intent(backend, path, request, runtime)
    guard()
    observed = capture(call, request["markers"])
    filename = path / f"observed-{number:04d}.json"
    write_json(filename, observed)
    state = classify(request["markers"], observed)
    proof = {"intent": intent, "observed": binding(filename), "state": state}
    write_json(path / f"reconcile-{number:04d}.json", proof)
    guard()
    if state == "after":
        if not (path / "confirmed.json").exists():
            write_json(path / "confirmed.json", {"intent": intent, "observed": binding(filename),
                       "response": None, "reconciled": True})
        confirmation(backend, path, request, runtime)
    elif state != "before":
        raise ValueError("缓存未知写入既非完整前像也非完整后像；拒绝继续")
    return state


def require_resumable(backend, path, request, runtime):
    intent = load_intent(backend, path, request, runtime)
    files = sorted(path.glob("reconcile-*.json"))
    if not files:
        raise ValueError("未知缓存写入必须先显式 reconcile")
    proof = read_json(files[-1])
    observed = bound_file(backend, proof["observed"])
    if (proof != {"intent": intent, "observed": proof["observed"], "state": "before"}
            or observed != path / files[-1].name.replace("reconcile-", "observed-")
            or classify(request["markers"], read_json(observed)) != "before"):
        raise ValueError("只有明确核对为完整前像才允许显式 resume")
