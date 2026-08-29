from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from typing import Any, NamedTuple


class Counters(NamedTuple):
    requests: int
    hits: int
    misses: int
    c_cpp_hits: int
    c_cpp_misses: int
    not_cacheable: int
    errors: int

    def subtract(self, previous: "Counters") -> "Counters":
        values = (
            self.requests - previous.requests,
            self.hits - previous.hits,
            self.misses - previous.misses,
            self.c_cpp_hits - previous.c_cpp_hits,
            self.c_cpp_misses - previous.c_cpp_misses,
            self.not_cacheable - previous.not_cacheable,
            self.errors - previous.errors,
        )
        if any(value < 0 for value in values):
            raise ValueError("sccache 累计计数器发生回退")
        return Counters(*values)


def _read_object(path: Path) -> dict[str, Any]:
    try:
        value: Any = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"无法读取 JSON {path}：{error}") from error
    if not isinstance(value, dict):
        raise ValueError(f"{path} 的根节点必须是对象")
    return value


def _non_negative_int(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"{label} 必须是非负整数")
    return value


def _counts(value: Any, label: str) -> dict[str, int]:
    if not isinstance(value, dict):
        raise ValueError(f"{label} 必须是对象")
    counts = value.get("counts")
    if not isinstance(counts, dict):
        raise ValueError(f"{label}.counts 必须是对象")
    if any(not isinstance(name, str) or not name for name in counts):
        raise ValueError(f"{label}.counts 的分类名必须是非空字符串")
    return {
        name: _non_negative_int(count, f"{label}.counts.{name}")
        for name, count in counts.items()
    }


def _count_map(value: Any, label: str) -> int:
    return sum(_counts(value, label).values())


def _snapshot(document: dict[str, Any], label: str) -> tuple[Counters, tuple[str, ...]]:
    stats = document.get("stats")
    if not isinstance(stats, dict):
        raise ValueError(f"{label}.stats 必须是对象")
    errors = _count_map(stats.get("cache_errors"), f"{label}.stats.cache_errors")
    for name in (
        "cache_timeouts",
        "cache_read_errors",
        "cache_write_errors",
        "dist_errors",
    ):
        errors += _non_negative_int(stats.get(name), f"{label}.stats.{name}")
    cache_hits = _counts(stats.get("cache_hits"), f"{label}.stats.cache_hits")
    cache_misses = _counts(stats.get("cache_misses"), f"{label}.stats.cache_misses")
    counters = Counters(
        requests=_non_negative_int(
            stats.get("requests_executed"), f"{label}.stats.requests_executed"
        ),
        hits=sum(cache_hits.values()),
        misses=sum(cache_misses.values()),
        c_cpp_hits=cache_hits.get("C/C++", 0),
        c_cpp_misses=cache_misses.get("C/C++", 0),
        not_cacheable=_non_negative_int(
            stats.get("requests_not_cacheable"),
            f"{label}.stats.requests_not_cacheable",
        ),
        errors=errors,
    )
    basedirs = document.get("basedirs")
    if not isinstance(basedirs, list) or any(
        not isinstance(path, str) for path in basedirs
    ):
        raise ValueError(f"{label}.basedirs 必须是字符串数组")
    return counters, tuple(basedirs)


def _canonical_path(path: str) -> str:
    return os.path.normcase(os.path.realpath(path))


def _validate_basedirs(
    snapshots: tuple[tuple[str, ...], ...], expected_basedirs: tuple[str, ...]
) -> list[str]:
    errors: list[str] = []
    if (
        len(expected_basedirs) != 2
        or len(set(map(_canonical_path, expected_basedirs))) != 2
    ):
        return ["必须提供两个不同的 SCCACHE_BASEDIRS 绝对路径"]
    if any(not Path(path).is_absolute() for path in expected_basedirs):
        return ["SCCACHE_BASEDIRS 只能包含绝对路径"]
    expected = {_canonical_path(path) for path in expected_basedirs}
    for index, basedirs in enumerate(snapshots):
        actual = {_canonical_path(path) for path in basedirs}
        if actual != expected:
            errors.append(
                f"第 {index + 1} 份 sccache 统计的 basedirs 与双路径配置不一致"
            )
    return errors


def _duration_ms(timings: dict[str, Any], name: str) -> float:
    value = timings.get(name)
    if not isinstance(value, (int, float)) or isinstance(value, bool) or value <= 0:
        raise ValueError(f"timings.{name} 必须是正数")
    return float(value)


def evaluate(
    before_document: dict[str, Any],
    prime_document: dict[str, Any],
    warm_document: dict[str, Any],
    timings: dict[str, Any],
    expected_basedirs: tuple[str, ...],
    min_hit_rate: float = 0.80,
    min_speedup: float = 0.05,
    min_non_cacheable_reduction: float = 0.50,
) -> tuple[dict[str, Any], list[str]]:
    before, before_basedirs = _snapshot(before_document, "before")
    prime, prime_basedirs = _snapshot(prime_document, "prime")
    warm, warm_basedirs = _snapshot(warm_document, "warm")
    cold_delta = prime.subtract(before)
    warm_delta = warm.subtract(prime)
    all_delta = warm.subtract(before)
    cold_ms = _duration_ms(timings, "cold_ms")
    warm_ms = _duration_ms(timings, "warm_ms")
    cold_c_cpp_cacheable = cold_delta.c_cpp_hits + cold_delta.c_cpp_misses
    warm_c_cpp_cacheable = warm_delta.c_cpp_hits + warm_delta.c_cpp_misses
    cold_other_cacheable = cold_delta.hits + cold_delta.misses - cold_c_cpp_cacheable
    warm_other_cacheable = warm_delta.hits + warm_delta.misses - warm_c_cpp_cacheable
    if cold_other_cacheable < 0 or warm_other_cacheable < 0:
        raise ValueError("sccache 编译分类累计计数器发生回退")
    hit_rate = (
        warm_delta.c_cpp_hits / warm_c_cpp_cacheable
        if warm_c_cpp_cacheable
        else 0.0
    )
    speedup = (cold_ms - warm_ms) / cold_ms
    if cold_delta.not_cacheable == 0:
        non_cacheable_reduction = 1.0 if warm_delta.not_cacheable == 0 else -1.0
    else:
        non_cacheable_reduction = (
            cold_delta.not_cacheable - warm_delta.not_cacheable
        ) / cold_delta.not_cacheable

    errors = _validate_basedirs(
        (before_basedirs, prime_basedirs, warm_basedirs), expected_basedirs
    )
    if all_delta.errors:
        errors.append(f"canary 期间出现 {all_delta.errors} 个缓存错误")
    if warm_delta.requests <= 0:
        errors.append("warm 构建没有经过 sccache 编译请求")
    if cold_other_cacheable or warm_other_cacheable:
        errors.append(
            "canary 必须只包含 C/C++ 缓存请求，"
            f"实际 prime={cold_other_cacheable}、warm={warm_other_cacheable}"
        )
    if cold_c_cpp_cacheable <= 0:
        errors.append("prime 构建没有经过 sccache 的 C/C++ 编译请求")
    if warm_c_cpp_cacheable <= 0:
        errors.append("warm 构建没有经过 sccache 的 C/C++ 编译请求")
    elif hit_rate < min_hit_rate:
        errors.append(f"warm C/C++ 命中率 {hit_rate:.2%} 低于 {min_hit_rate:.0%}")
    if speedup < min_speedup:
        errors.append(f"warm 耗时改善 {speedup:.2%} 低于 {min_speedup:.0%}")
    if non_cacheable_reduction < min_non_cacheable_reduction:
        errors.append(
            "warm 不可缓存请求改善 "
            f"{non_cacheable_reduction:.2%} 低于 {min_non_cacheable_reduction:.0%}"
        )

    report: dict[str, Any] = {
        "schemaVersion": 2,
        "passed": not errors,
        "thresholds": {
            "minWarmHitRate": min_hit_rate,
            "minSpeedupRatio": min_speedup,
            "minNonCacheableReductionRatio": min_non_cacheable_reduction,
        },
        "cold": {
            "durationMs": cold_ms,
            "requests": cold_delta.requests,
            "cCppHits": cold_delta.c_cpp_hits,
            "cCppMisses": cold_delta.c_cpp_misses,
            "cCppCacheableRequests": cold_c_cpp_cacheable,
            "otherCacheableRequests": cold_other_cacheable,
            "notCacheable": cold_delta.not_cacheable,
        },
        "warm": {
            "durationMs": warm_ms,
            "requests": warm_delta.requests,
            "cCppHits": warm_delta.c_cpp_hits,
            "cCppMisses": warm_delta.c_cpp_misses,
            "cCppCacheableRequests": warm_c_cpp_cacheable,
            "otherCacheableRequests": warm_other_cacheable,
            "notCacheable": warm_delta.not_cacheable,
            "hitRate": hit_rate,
        },
        "cacheErrors": all_delta.errors,
        "speedupRatio": speedup,
        "nonCacheableReductionRatio": non_cacheable_reduction,
        "basedirs": list(expected_basedirs),
        "errors": errors,
    }
    return report, errors


def render_markdown(report: dict[str, Any]) -> str:
    status = "通过" if report["passed"] else "失败"
    cold = report["cold"]
    warm = report["warm"]
    lines = [
        "### AWS-LC C/C++ sccache canary",
        "",
        f"- 结果：{status}",
        f"- cold：{cold['durationMs']:.0f} ms（{cold['cCppCacheableRequests']} 个 C/C++ 请求）",
        f"- warm：{warm['durationMs']:.0f} ms（{warm['cCppCacheableRequests']} 个 C/C++ 请求）",
        f"- warm C/C++ 命中率：{warm['hitRate']:.2%}",
        f"- 耗时改善：{report['speedupRatio']:.2%}",
        f"- 不可缓存请求改善：{report['nonCacheableReductionRatio']:.2%}",
        f"- 缓存错误：{report['cacheErrors']}",
    ]
    if report["errors"]:
        lines.extend(("", "失败原因：", *[f"- {error}" for error in report["errors"]]))
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(
        description="评估 AWS-LC C/C++ sccache 双路径 canary"
    )
    parser.add_argument("--before", type=Path, required=True)
    parser.add_argument("--prime", type=Path, required=True)
    parser.add_argument("--warm", type=Path, required=True)
    parser.add_argument("--timings", type=Path, required=True)
    parser.add_argument("--expected-basedir", action="append", default=[])
    parser.add_argument("--min-hit-rate", type=float, default=0.80)
    parser.add_argument("--min-speedup", type=float, default=0.05)
    parser.add_argument("--min-non-cacheable-reduction", type=float, default=0.50)
    parser.add_argument("--report", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    args = parser.parse_args()
    if not all(
        0 <= threshold <= 1
        for threshold in (
            args.min_hit_rate,
            args.min_speedup,
            args.min_non_cacheable_reduction,
        )
    ):
        parser.error("命中率、耗时与不可缓存请求改善阈值必须位于 0 到 1 之间")
    try:
        report, errors = evaluate(
            _read_object(args.before),
            _read_object(args.prime),
            _read_object(args.warm),
            _read_object(args.timings),
            tuple(args.expected_basedir),
            args.min_hit_rate,
            args.min_speedup,
            args.min_non_cacheable_reduction,
        )
    except ValueError as error:
        parser.error(str(error))
    args.report.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    summary = render_markdown(report)
    args.summary.write_text(summary, encoding="utf-8")
    print(summary, end="")
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
