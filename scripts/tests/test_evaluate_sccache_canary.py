from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "evaluate_sccache_canary.py"
ROOT = SCRIPT.parents[1]
SPEC = importlib.util.spec_from_file_location("evaluate_sccache_canary", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
BASE_A = str((ROOT / ".canary-source-a").resolve())
BASE_B = str((ROOT / ".canary-source-b").resolve())


def snapshot(
    requests: int,
    hits: int,
    misses: int,
    not_cacheable: int = 0,
    errors: int = 0,
    basedirs: tuple[str, str] = (BASE_A, BASE_B),
) -> dict[str, object]:
    return {
        "stats": {
            "requests_executed": requests,
            "requests_not_cacheable": not_cacheable,
            "cache_hits": {"counts": {"C/C++": hits}, "adv_counts": {}},
            "cache_misses": {"counts": {"C/C++": misses}, "adv_counts": {}},
            "cache_errors": {"counts": {"C/C++": errors}, "adv_counts": {}},
            "cache_timeouts": 0,
            "cache_read_errors": 0,
            "cache_write_errors": 0,
            "dist_errors": 0,
        },
        "basedirs": list(basedirs),
    }


class EvaluateSccacheCanaryTests(unittest.TestCase):
    def evaluate(
        self,
        before: dict[str, object] | None = None,
        prime: dict[str, object] | None = None,
        warm: dict[str, object] | None = None,
        timings: dict[str, float] | None = None,
    ) -> tuple[dict[str, object], list[str]]:
        return MODULE.evaluate(
            before or snapshot(0, 0, 0),
            prime or snapshot(100, 0, 100),
            warm or snapshot(200, 90, 110),
            timings or {"cold_ms": 100_000, "warm_ms": 80_000},
            (BASE_A, BASE_B),
        )

    def test_accepts_cross_checkout_warm_cache_improvement(self) -> None:
        report, errors = self.evaluate()
        self.assertEqual(errors, [])
        self.assertTrue(report["passed"])
        self.assertEqual(report["warm"]["requests"], 100)
        self.assertAlmostEqual(report["warm"]["hitRate"], 0.9)
        self.assertAlmostEqual(report["speedupRatio"], 0.2)

    def test_rejects_cache_errors_even_when_hit_rate_is_high(self) -> None:
        report, errors = self.evaluate(warm=snapshot(200, 90, 110, errors=1))
        self.assertFalse(report["passed"])
        self.assertTrue(any("缓存错误" in error for error in errors))

    def test_rejects_missing_requests_low_hits_and_insufficient_speedup(self) -> None:
        cases = (
            (snapshot(100, 0, 100), {"cold_ms": 100, "warm_ms": 80}, "没有经过"),
            (snapshot(200, 79, 121), {"cold_ms": 100, "warm_ms": 80}, "命中率"),
            (snapshot(200, 90, 110), {"cold_ms": 100, "warm_ms": 96}, "耗时改善"),
        )
        for warm, timings, marker in cases:
            with self.subTest(marker=marker):
                _, errors = self.evaluate(warm=warm, timings=timings)
                self.assertTrue(any(marker in error for error in errors))

    def test_requires_exactly_two_distinct_absolute_basedirs(self) -> None:
        for basedirs in (
            (BASE_A,),
            (BASE_A, BASE_A),
            ("relative-a", BASE_B),
        ):
            with self.subTest(basedirs=basedirs):
                _, errors = MODULE.evaluate(
                    snapshot(0, 0, 0),
                    snapshot(100, 0, 100),
                    snapshot(200, 90, 110),
                    {"cold_ms": 100, "warm_ms": 80},
                    basedirs,
                )
                self.assertTrue(any("SCCACHE_BASEDIRS" in error for error in errors))

    def test_requires_non_cacheable_requests_to_drop_by_half(self) -> None:
        report, errors = self.evaluate(
            prime=snapshot(100, 0, 100, not_cacheable=10),
            warm=snapshot(200, 90, 110, not_cacheable=16),
        )
        self.assertFalse(report["passed"])
        self.assertTrue(any("不可缓存请求改善" in error for error in errors))

        report, errors = self.evaluate(
            prime=snapshot(100, 0, 100, not_cacheable=10),
            warm=snapshot(200, 90, 110, not_cacheable=15),
        )
        self.assertEqual(errors, [])
        self.assertAlmostEqual(report["nonCacheableReductionRatio"], 0.5)

    def test_rejects_snapshot_basedir_drift_and_counter_rollback(self) -> None:
        _, errors = self.evaluate(
            warm=snapshot(
                200,
                90,
                110,
                basedirs=(BASE_A, str((ROOT / ".unexpected").resolve())),
            )
        )
        self.assertTrue(any("basedirs" in error for error in errors))
        with self.assertRaisesRegex(ValueError, "计数器发生回退"):
            self.evaluate(warm=snapshot(99, 0, 99))

    def test_markdown_contains_threshold_results(self) -> None:
        report, _ = self.evaluate()
        markdown = MODULE.render_markdown(report)
        self.assertIn("warm 命中率：90.00%", markdown)
        self.assertIn("耗时改善：20.00%", markdown)
        self.assertIn("不可缓存请求改善：100.00%", markdown)


if __name__ == "__main__":
    unittest.main()
