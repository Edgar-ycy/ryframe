"""完整转储消费边界与受管 typed NDJSON 传输；不实现 SQL 解码。"""
import copy
from contextlib import redirect_stderr
from decimal import Decimal
import hashlib
from io import StringIO
import json
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

import devex_clone_rows as source
import devex_clone_export as export
from artifact_digests import filesystem_path
from devex_clone_export_verify import verify_dump_declarations
from restore_build import file_digest
from restore_reference_io import DATA_HEADER, ExternalTools
from restore_reference_fixture import environment


class DumpRowTests(unittest.TestCase):
    def setUp(self):
        backend, plan = environment(self)
        self.root = backend / ".local-tests"
        self.configuration = plan["source"]["s3"]["configuration"]
        self.path = self.root / "control.sql"
        self.catalog = {"sys_post": {"id": "BIGINT", "name": "TEXT"}, "sys_notice": {"id": "BIGINT"}}
        self.models = (self.catalog, {}, self.catalog, {})

    def declaration(self, content, *, counts=None, inventory_counts=None):
        self.path.write_text(content, encoding="utf-8")
        counts = {"sys_post": 0, "sys_notice": 0} if counts is None else counts
        inventory_counts = counts if inventory_counts is None else inventory_counts
        value = {"source": {"scope_id": "source-scope", "s3": {"configuration": self.configuration},
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

    def test_empty_dump_still_requires_exact_header_and_all_empty_tables(self):
        self.check(DATA_HEADER)
        for content in ("", "-- invalid\n", DATA_HEADER + "\n", DATA_HEADER + "-- comment\n"):
            with self.subTest(content=content), self.assertRaises(ValueError):
                self.check(content)
        with self.assertRaises(ValueError):
            self.check(DATA_HEADER, counts={"sys_post": 0})

    def test_declared_and_inventory_counts_both_still_match_actual_rows(self):
        content = DATA_HEADER + "INSERT INTO `sys_post` (`id`, `name`) VALUES (1, 'first');\n"
        with self.assertRaisesRegex(ValueError, "行数"):
            self.check(content)
        with self.assertRaisesRegex(ValueError, "行数"):
            self.check(content, counts={"sys_post": 1, "sys_notice": 0},
                       inventory_counts={"sys_post": 2, "sys_notice": 0})

    def test_declaration_only_checks_bound_metadata_without_parsing_rows(self):
        value, inventory = self.declaration(DATA_HEADER, counts={"sys_post": 2, "sys_notice": 0})
        with patch.object(export, "rows", side_effect=AssertionError("declarations must not read SQL")) as rows:
            verify_dump_declarations(self.root, value, inventory, self.models)
        rows.assert_not_called()

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

    def wire_transport(self, payload, *, alter=lambda _operation, value: value, failed_read=False):
        catalog = {"sys_post": {"id": "BIGINT", "price": "DECIMAL", "payload": "JSON",
                                 "raw": "BLOB", "empty": "TEXT", "name": "TEXT"},
                   "sys_notice": {"id": "BIGINT"}}
        self.path.write_text(DATA_HEADER + "INSERT INTO `sys_post` (`id`, `price`, `payload`, `raw`, `empty`, `name`) VALUES (9007199254740993,90071992547409.123456,'{}',0x00ff,NULL,'中文');\n", encoding="utf-8")
        catalog_sha = hashlib.sha256(json.dumps(catalog, ensure_ascii=False, sort_keys=True,
                                               separators=(",", ":")).encode()).hexdigest()
        data = payload.encode("utf-8")
        stream = {"format_version": 1, "kind": "ryframe-sql-row-stream-v1",
                  "backend": filesystem_path(self.root.parent.resolve()),
                  "root": filesystem_path((self.root / "sql-row-streams/sql-rows-wire").resolve()),
                  "source": {"path": filesystem_path(self.path.resolve()), **file_digest(self.path), "device": 1, "inode": 2},
                  "catalog_sha256": catalog_sha, "only_table": None,
                  "tables": {"sys_post": 1, "sys_notice": 0}, "source_rows": 1, "rows": 1,
                  "artifact": {"file": "rows.jsonl", "bytes": len(data),
                               "sha256": hashlib.sha256(data).hexdigest(), "device": 1, "inode": 3},
                  "manifest": {"file": "manifest.json", "bytes": 1, "sha256": "a" * 64, "device": 1, "inode": 4}}
        chunks, offset = {}, 0
        for text in (payload[:17], payload[17:len(payload)//2], payload[len(payload)//2:]):
            encoded = text.encode("utf-8")
            chunks[offset] = {"offset": offset, "next_offset": offset + len(encoded),
                              "text": text, "chunk_sha256": hashlib.sha256(encoded).hexdigest(),
                              "eof": offset + len(encoded) == len(data)}
            offset += len(encoded)
        events = []

        def invoke(command, request, _runner, *, may_write):
            self.assertEqual(command, "__source-evidence")
            operation = request["operation"]
            self.assertFalse(may_write)
            self.assertEqual(request["write"], operation != "reference-sql-stream-read")
            events.append(operation)
            if operation == "reference-sql-stream-create":
                self.assertEqual(request["arguments"]["expected"], file_digest(self.path))
                value = copy.deepcopy(stream)
            elif operation == "reference-sql-stream-read":
                if failed_read:
                    return subprocess.CompletedProcess([], 1), {"ok": True, "result": {"value": {}}}
                value = copy.deepcopy(chunks[request["arguments"]["offset"]])
            elif operation == "reference-sql-stream-close":
                value = True
            else:
                raise AssertionError("意外的行流操作")
            return subprocess.CompletedProcess([], 0), {"ok": True, "result": {"value": alter(operation, value)}}
        return catalog, catalog_sha, invoke, events

    def test_typed_stream_preserves_exact_json_and_cells_across_chunks(self):
        payload = '{"table":"sys_post","row":{"id":{"kind":"number","value":"9007199254740993"},"price":{"kind":"number","value":"90071992547409.123456"},"payload":{"kind":"json","value":{"big":123456789012345678901234567890,"small":0.12345678901234567890123456789}},"raw":{"kind":"bytes","value":"00ff"},"empty":{"kind":"null"},"name":{"kind":"text","value":"中文"}}}\n'
        catalog, catalog_sha, invoke, events = self.wire_transport(payload)
        with patch.object(source.native, "ROOT", self.root.parent), \
                patch.object(source, "plan_hash", return_value=catalog_sha), \
                patch.object(source.native, "_invoke", side_effect=invoke):
            decoded = list(source.rows(self.path, catalog))
        self.assertEqual(decoded, [("sys_post", {
            "id": Decimal("9007199254740993"), "price": Decimal("90071992547409.123456"),
            "payload": {"big": Decimal("123456789012345678901234567890"),
                        "small": Decimal("0.12345678901234567890123456789")},
            "raw": b"\x00\xff", "empty": None, "name": "中文"})])
        self.assertIs(type(decoded[0][1]["payload"]["big"]), Decimal)
        self.assertIs(type(decoded[0][1]["payload"]["small"]), Decimal)
        self.assertEqual(events[-1], "reference-sql-stream-close")

    def test_stream_rejects_corruption_and_transport_failure_without_closing_evidence(self):
        payload = '{"table":"sys_post","row":{"id":{"kind":"number","value":"1"},"price":{"kind":"number","value":"0.125000"},"payload":{"kind":"json","value":{"a":1}},"raw":{"kind":"bytes","value":"00"},"empty":{"kind":"null"},"name":{"kind":"text","value":"中文"}}}\n'
        for risk in ("offset", "next_offset", "chunk_sha", "eof", "whole_sha", "empty_table",
                     "source_sha", "catalog_sha", "metadata_bool", "root", "rows", "transport"):
            def alter(operation, value):
                if operation == "reference-sql-stream-create":
                    if risk == "whole_sha": value["artifact"]["sha256"] = "0" * 64
                    if risk == "empty_table": del value["tables"]["sys_notice"]
                    if risk == "source_sha": value["source"]["sha256"] = "0" * 64
                    if risk == "catalog_sha": value["catalog_sha256"] = "0" * 64
                    if risk == "metadata_bool": value["source"]["device"] = True
                    if risk == "root": value["root"] = str(self.root.parent / "outside-stream-root")
                    if risk == "rows": value["tables"]["sys_post"] = value["source_rows"] = value["rows"] = 2
                if operation == "reference-sql-stream-read":
                    if risk == "offset": value["offset"] += 1
                    if risk == "next_offset": value["next_offset"] += 1
                    if risk == "chunk_sha": value["chunk_sha256"] = "0" * 64
                    if risk == "eof": value["eof"] = not value["eof"]
                return value
            with self.subTest(risk=risk):
                catalog, catalog_sha, invoke, events = self.wire_transport(payload, alter=alter, failed_read=risk == "transport")
                with patch.object(source.native, "ROOT", self.root.parent), \
                        patch.object(source, "plan_hash", return_value=catalog_sha), \
                        patch.object(source.native, "_invoke", side_effect=invoke), self.assertRaisesRegex(ValueError, "保留"):
                    list(source.rows(self.path, catalog))
                self.assertNotIn("reference-sql-stream-close", events)
                self.assertTrue(self.path.is_file())
        for malformed in (payload.replace('"a":1', '"a":1,"a":2'),
                          payload.replace('"a":1', '"a":NaN'), payload.replace('"a":1', '"a":Infinity'),
                          payload.rstrip("\n")):
            with self.subTest(wire=malformed):
                catalog, catalog_sha, invoke, events = self.wire_transport(malformed)
                with patch.object(source.native, "ROOT", self.root.parent), \
                        patch.object(source, "plan_hash", return_value=catalog_sha), \
                        patch.object(source.native, "_invoke", side_effect=invoke), self.assertRaises(ValueError):
                    list(source.rows(self.path, catalog))
                self.assertNotIn("reference-sql-stream-close", events)
        catalog, catalog_sha, invoke, events = self.wire_transport(payload)
        diagnostic = StringIO()
        with patch.object(source.native, "ROOT", self.root.parent), \
                patch.object(source, "plan_hash", return_value=catalog_sha), \
                patch.object(source.native, "_invoke", side_effect=invoke), redirect_stderr(diagnostic):
            pending = source.rows(self.path, catalog)
            next(pending)
            pending.close()
        self.assertNotIn("reference-sql-stream-close", events)
        self.assertIn("sql-rows-wire", diagnostic.getvalue())


if __name__ == "__main__":
    unittest.main()
