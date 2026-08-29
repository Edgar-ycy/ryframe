from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, NamedTuple


class SccacheSummary(NamedTuple):
    requests: int
    hits: int
    misses: int
    not_cacheable: int
    errors: int

    @property
    def cacheable(self) -> int:
        return self.hits + self.misses

    @property
    def hit_rate(self) -> float | None:
        return self.hits / self.cacheable if self.cacheable else None


def _non_negative_int(value: Any, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"{label} 必须是非负整数")
    return value


def _count_map(value: Any, label: str) -> int:
    if not isinstance(value, dict):
        raise ValueError(f"{label} 必须是对象")
    counts = value.get("counts")
    if not isinstance(counts, dict):
        raise ValueError(f"{label}.counts 必须是对象")
    return sum(
        _non_negative_int(count, f"{label}.counts.{name}")
        for name, count in counts.items()
    )


def summarize(document: dict[str, Any]) -> SccacheSummary:
    stats = document.get("stats")
    if not isinstance(stats, dict):
        raise ValueError("stats 必须是对象")
    errors = _count_map(stats.get("cache_errors"), "stats.cache_errors")
    for name in (
        "cache_timeouts",
        "cache_read_errors",
        "cache_write_errors",
        "dist_errors",
    ):
        errors += _non_negative_int(stats.get(name), f"stats.{name}")
    return SccacheSummary(
        requests=_non_negative_int(stats.get("compile_requests"), "stats.compile_requests"),
        hits=_count_map(stats.get("cache_hits"), "stats.cache_hits"),
        misses=_count_map(stats.get("cache_misses"), "stats.cache_misses"),
        not_cacheable=_non_negative_int(
            stats.get("requests_not_cacheable"), "stats.requests_not_cacheable"
        ),
        errors=errors,
    )


def render_markdown(label: str, summary: SccacheSummary) -> str:
    hit_rate = (
        f"{summary.hit_rate:.2%}"
        if summary.hit_rate is not None
        else "不可计算（0 个可缓存请求）"
    )
    return "\n".join(
        [
            f"### sccache · {label}",
            "",
            f"- 命中率：{hit_rate}",
            f"- 可缓存请求：{summary.cacheable}（命中 {summary.hits}，未命中 {summary.misses}）",
            f"- 编译请求：{summary.requests}",
            f"- 不可缓存请求：{summary.not_cacheable}",
            f"- 缓存错误：{summary.errors}",
            "",
        ]
    )


def _read_document(path: Path) -> dict[str, Any]:
    try:
        document: Any = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as error:
        raise ValueError(f"无法读取 sccache JSON {path}：{error}") from error
    if not isinstance(document, dict):
        raise ValueError("sccache JSON 根节点必须是对象")
    return document


def main() -> int:
    parser = argparse.ArgumentParser(description="输出 sccache JSON 的明确命中率摘要")
    parser.add_argument("--stats", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--summary", type=Path)
    args = parser.parse_args()
    try:
        markdown = render_markdown(args.label, summarize(_read_document(args.stats)))
        if args.summary is not None:
            with args.summary.open("a", encoding="utf-8", newline="\n") as output:
                output.write(markdown)
        print(markdown, end="")
    except ValueError as error:
        parser.error(str(error))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
