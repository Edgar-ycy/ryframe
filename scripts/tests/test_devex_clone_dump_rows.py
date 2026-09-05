"""完整转储计数复用唯一严格行解析；无服务依赖。"""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import devex_clone_rows as source
import devex_clone_export as export
import restore_reference_io as restore_io
from devex_clone_export_verify import verify_dump_declarations
from restore_build import file_digest
from restore_reference_io import DATA_HEADER, ExternalTools


class DumpRowTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.path = self.root / "control.sql"
        self.catalog = {"sys_post": {"id": "BIGINT", "name": "TEXT"}, "sys_notice": {"id": "BIGINT"}}
        self.models = (self.catalog, {}, self.catalog, {})

    def declaration(self, content, *, counts=None, inventory_counts=None):
        self.path.write_text(content, encoding="utf-8")
        counts = {"sys_post": 0, "sys_notice": 0} if counts is None else counts
        inventory_counts = counts if inventory_counts is None else inventory_counts
        value = {"source": {"scope_id": "source-scope", "s3": {"endpoint": "http://127.0.0.1:9000"},
                           "databases": [{"key": "control", "kind": "combined", "database": "source_control"}]},
                 "databases": [{"key": "control", "tables": counts,
                                "artifact": {"file": "control.sql", **file_digest(self.path)}}]}
        inventory = {"databases": [{"key": "control", "tables": [
            {"table": table, "rows": rows} for table, rows in inventory_counts.items()]}]}
        return value, inventory

    def check(self, content, **options):
        value, inventory = self.declaration(content, **options)
        verify_dump_declarations(self.root, value, inventory, self.models)
        tools = ExternalTools({"source": value["source"]}, self.root)
        # 本组只核验语法与计数；完整租户、placement、任务和对象关系另由真实来源夹具覆盖。
        with patch.object(export, "declared_tenants", return_value={"system"}), \
                patch.object(export, "validate_relations") as relations:
            export.validate_dump_state(tools, self.models, value["databases"], {})
            relations.assert_called_once()

    def test_complete_rows_validate_and_parse_each_insert_exactly_once(self):
        content = DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'first');\n"
        content += "INSERT INTO `sys_post` (`id`, `name`) VALUES (2, 'next');\n"
        with patch.object(source, "validate_insert", wraps=source.validate_insert) as validate, \
                patch.object(restore_io, "validate_insert", validate), \
                patch.object(source, "parse_row", wraps=source.parse_row) as parse:
            self.check(content, counts={"sys_post": 2, "sys_notice": 0})
        self.assertEqual(validate.call_count, 2)
        self.assertEqual(parse.call_count, 2)

    def test_empty_dump_still_requires_exact_header_and_all_empty_tables(self):
        self.check(DATA_HEADER)
        for content in ("", "-- invalid\n", DATA_HEADER + "\n", DATA_HEADER + "-- comment\n"):
            with self.subTest(content=content), self.assertRaises(ValueError):
                self.check(content)
        with self.assertRaises(ValueError):
            self.check(DATA_HEADER, counts={"sys_post": 0})

    def test_malicious_sql_and_incomplete_or_duplicate_columns_remain_rejected(self):
        invalid = (
            "DELETE FROM `sys_post`;", "INSERT INTO `sys_post` (`id`, `name`) VALUES (SLEEP(1), 'x');",
            "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'x'); DROP TABLE `sys_post`;",
            "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'bad\\q');",
            "INSERT INTO `sys_post` (`id`) VALUES (1);",
            "INSERT INTO `sys_post` (`id`, `id`) VALUES (1, 2);",
            "INSERT INTO `sys_post` (`id`, `name`) VALUES (1);",
            "INSERT INTO `unknown` (`id`) VALUES (1);",
        )
        for line in invalid:
            with self.subTest(line=line), self.assertRaises(ValueError):
                self.check(DATA_HEADER + line + "\n", counts={"sys_post": 1, "sys_notice": 0})

    def test_declared_and_inventory_counts_both_still_match_actual_rows(self):
        content = DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'first');\n"
        with self.assertRaisesRegex(ValueError, "行数"):
            self.check(content)
        with self.assertRaisesRegex(ValueError, "行数"):
            self.check(content, counts={"sys_post": 1, "sys_notice": 0},
                       inventory_counts={"sys_post": 2, "sys_notice": 0})

    def test_json_duplicate_and_nonfinite_numbers_are_not_hidden_by_counting(self):
        self.catalog["sys_post"]["name"] = "JSON"
        for literal in ('{"a":1,"a":2}', '{"a":NaN}', '{"a":Infinity}'):
            with self.subTest(literal=literal), self.assertRaises(ValueError):
                self.check(DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, '" + literal + "');\n",
                           counts={"sys_post": 1, "sys_notice": 0})

    def test_declaration_only_checks_bound_metadata_without_parsing_rows(self):
        value, inventory = self.declaration(DATA_HEADER, counts={"sys_post": 2, "sys_notice": 0})
        with patch.object(export, "rows") as rows, patch.object(source, "parse_row") as parse:
            verify_dump_declarations(self.root, value, inventory, self.models)
        rows.assert_not_called()
        parse.assert_not_called()

    def test_declared_counts_reject_bool_negative_fraction_and_non_dictionary(self):
        for count in (True, False, -1, 0.5, "0", None):
            with self.subTest(count=count), self.assertRaises(ValueError):
                self.check(DATA_HEADER, counts={"sys_post": count, "sys_notice": 0})
        value, inventory = self.declaration(DATA_HEADER)
        value["databases"][0]["tables"] = ["sys_post", "sys_notice"]
        with self.assertRaises(ValueError):
            verify_dump_declarations(self.root, value, inventory, self.models)

    def test_actual_count_mismatch_fails_before_relationship_validation(self):
        value, inventory = self.declaration(DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'first');\n")
        verify_dump_declarations(self.root, value, inventory, self.models)
        tools = ExternalTools({"source": value["source"]}, self.root)
        with patch.object(export, "declared_tenants", return_value={"system"}), \
                patch.object(export, "validate_relations") as relations, self.assertRaisesRegex(ValueError, "实际行数"):
            export.validate_dump_state(tools, self.models, value["databases"], {})
        relations.assert_not_called()

    def test_illegal_trailing_row_is_not_hidden_by_matching_prior_count(self):
        content = DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'first');\n"
        for suffix in ("DELETE FROM `sys_post`;\n", "INSERT INTO `unknown` (`id`) VALUES (2);\n", "-- tail\n"):
            with self.subTest(suffix=suffix), self.assertRaises(ValueError):
                self.check(content + suffix, counts={"sys_post": 1, "sys_notice": 0})

    def test_changed_file_after_final_row_is_rejected_by_original_binding(self):
        original = export.rows
        def changed(path, catalog):
            yield from original(path, catalog)
            path.write_bytes(path.read_bytes().replace(b"first", b"other"))
        with patch.object(export, "rows", side_effect=changed), self.assertRaisesRegex(ValueError, "证据已变化"):
            self.check(DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'first');\n",
                       counts={"sys_post": 1, "sys_notice": 0})


if __name__ == "__main__":
    unittest.main()
