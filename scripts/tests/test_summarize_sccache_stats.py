from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "summarize_sccache_stats.py"
SPEC = importlib.util.spec_from_file_location("summarize_sccache_stats", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def snapshot(hits: int = 80, misses: int = 20) -> dict[str, object]:
    return {
        "stats": {
            "compile_requests": 130,
            "requests_executed": 110,
            "requests_not_cacheable": 10,
            "cache_hits": {"counts": {"Rust": hits}, "adv_counts": {}},
            "cache_misses": {"counts": {"Rust": misses}, "adv_counts": {}},
            "cache_errors": {"counts": {"Rust": 1}, "adv_counts": {}},
            "cache_timeouts": 2,
            "cache_read_errors": 3,
            "cache_write_errors": 4,
            "dist_errors": 5,
        }
    }


class SummarizeSccacheStatsTests(unittest.TestCase):
    def test_computes_hit_rate_and_all_error_counters(self) -> None:
        summary = MODULE.summarize(snapshot())

        self.assertEqual(summary.cacheable, 100)
        self.assertEqual(summary.requests, 130)
        self.assertAlmostEqual(summary.hit_rate, 0.8)
        self.assertEqual(summary.errors, 15)
        markdown = MODULE.render_markdown("Rust Gate", summary)
        self.assertIn("命中率：80.00%", markdown)
        self.assertIn("可缓存请求：100（命中 80，未命中 20）", markdown)
        self.assertIn("缓存错误：15", markdown)

    def test_reports_zero_cacheable_requests_without_dividing_by_zero(self) -> None:
        summary = MODULE.summarize(snapshot(hits=0, misses=0))

        self.assertIsNone(summary.hit_rate)
        self.assertIn("命中率：不可计算", MODULE.render_markdown("cold", summary))

    def test_rejects_invalid_or_negative_counters(self) -> None:
        invalid = snapshot()
        stats = invalid["stats"]
        assert isinstance(stats, dict)
        stats["compile_requests"] = -1
        with self.assertRaisesRegex(ValueError, "非负整数"):
            MODULE.summarize(invalid)

        with self.assertRaisesRegex(ValueError, "stats 必须是对象"):
            MODULE.summarize({})


if __name__ == "__main__":
    unittest.main()
