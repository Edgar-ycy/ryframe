"""逻辑租户例外严格限定，规则缓存不得保存业务内容或检查结论。"""
import unittest
from unittest.mock import patch

import devex_clone_rows as source
from devex_clone_rows import logical_tenant_patterns, reject_physical


class CloneRowFilterTests(unittest.TestCase):
    def test_nested_logical_identifiers_preserve_exact_boundaries(self):
        tenants = {"source-a-01", "source-a-010"}
        allowed = ("source-a-01", "/source-a-01/file", "source-a-010", "[source-a-01]", "普通业务内容")
        denied = ("source-a", "SOURCE-A", "source-a-0100", "prefix_source-a-01", "source-a-01_suffix")
        for value in allowed:
            with self.subTest(allowed=value):
                reject_physical({"tenant_id": "source-a-01", "items": [None, 1, {"content": value}]}, ["source-a"], tenants)
        for value in denied:
            with self.subTest(denied=value), self.assertRaises(ValueError):
                reject_physical({"items": [None, 1, {"content": value}]}, ["source-a"], tenants)
        with self.assertRaises(ValueError):
            reject_physical("ß", ["ss"], tenants)
        with self.assertRaises(ValueError):
            reject_physical(b"\xff", ["source-a"], tenants)

    def test_physical_fields_never_use_logical_tenant_exception(self):
        tenants = {"source-a-01"}
        for field in ("scope", "scope_id", "storage_scope", "object_scope", "redis_namespace"):
            with self.subTest(field=field), self.assertRaises(ValueError):
                reject_physical({field: "source-a-01"}, ["source-a"], tenants)
        reject_physical("service.a[1]", ["service"], {"service.a[1]"})
        with self.assertRaises(ValueError):
            reject_physical("serviceXa1", ["service"], {"service.a[1]"})

    def test_bounded_rule_cache_does_not_reuse_values_or_another_tenant_set(self):
        logical_tenant_patterns.cache_clear()
        reject_physical("source-a-01", ["source-a"], {"source-a-01"})
        with self.assertRaises(ValueError):
            reject_physical("source-a-01", ["source-a"], {"other"})
        with self.assertRaises(ValueError):
            reject_physical("source-a-02", ["source-a"], {"source-a-01"})
        for number in range(140):
            reject_physical("safe business content", ["forbidden"], {f"tenant-{number}"})
        self.assertEqual(logical_tenant_patterns.cache_info().currsize, 128)

    def test_unicode_physical_casefold_never_expands_logical_identifier_case(self):
        for logical, physical, denied in (("Source-a-01", "source-a", "SOURCE-A-01"),
                                           ("Σ-01", "σ", "ς-01"), ("K-01", "k", "K-01")):
            with self.subTest(logical=logical):
                reject_physical(logical, [physical], {logical})
                with self.assertRaises(ValueError):
                    reject_physical(denied, [physical], {logical})
        with self.assertRaises(ValueError):
            reject_physical("straße", ["STRASSE"], {"unrelated"})

    def test_nested_physical_field_disables_exceptions_for_its_entire_value(self):
        tenants = {"source-a-01"}
        for value in (["source-a-01"], {"deep": ["source-a-01"]}, {"source-a-01": "safe"}):
            with self.subTest(value=value), self.assertRaises(ValueError):
                reject_physical({"nested": [{"scope_id": value}]}, ["source-a"], tenants)
        reject_physical({"scope_id": "safe", "following": "source-a-01"}, ["source-a"], tenants)

    def test_top_level_call_prepares_forbidden_and_logical_rules_once(self):
        calls = []

        class Token(str):
            def casefold(self):
                calls.append(str(self))
                return super().casefold()

        value = {"safe": ["source-a-01", {"scope_id": "safe", "values": [None, 3, b"safe"]}]}
        with patch.object(source, "logical_tenant_patterns", wraps=logical_tenant_patterns) as prepare:
            reject_physical(value, [Token("source-a"), Token("forbidden")], {"source-a-01"})
            self.assertEqual(prepare.call_count, 1)
        self.assertEqual(calls, ["source-a", "forbidden"])

    def test_literal_prefilter_only_skips_absent_tenants_and_preserves_regex_boundaries(self):
        class Pattern:
            def __init__(self):
                self.calls = []

            def sub(self, replacement, value):
                self.calls.append(value)
                return value.replace("source-a-01", replacement)

        pattern = Pattern()
        with patch.object(source, "logical_tenant_patterns", return_value=(("source-a-01", pattern),)):
            reject_physical({"safe": ["ordinary", "source-a-01", b"plain"]}, ["source-a"], {"source-a-01"})
        self.assertEqual(pattern.calls, ["source-a-01"])
        for text in ("prefix_source-a-01", "source-a-01_suffix", "source-a-010"):
            with self.subTest(text=text), self.assertRaises(ValueError):
                reject_physical(text, ["source-a"], {"source-a-01"})

    def test_each_call_rechecks_mutated_values_and_mutated_forbidden_list(self):
        forbidden, value = ["blocked"], {"content": "safe"}
        reject_physical(value, forbidden)
        value["content"] = "blocked"
        with self.assertRaises(ValueError):
            reject_physical(value, forbidden)
        value["content"] = "safe"
        forbidden[:] = ["safe"]
        with self.assertRaises(ValueError):
            reject_physical(value, forbidden)


if __name__ == "__main__":
    unittest.main()
