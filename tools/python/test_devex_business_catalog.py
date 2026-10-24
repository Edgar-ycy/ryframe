import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

from devex_business_catalog import business_columns, business_ledger_columns, business_tables


class BusinessCatalogTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = ["crates/order-business"]\n')
        self.package = self.root / "crates/order-business"
        (self.package / "src/generated").mkdir(parents=True)
        (self.package / ".ryframe").mkdir()
        (self.package / "Cargo.toml").write_text(
            '[package]\nname = "order-business"\n[package.metadata.ryframe]\nkind = "business"\nmodule = "order"\n')
        self.table = {
            "table": "biz_order", "copy_order": 1, "tenant_column": "tenant_id",
            "primary_key_cursor_columns": ["tenant_id", "id"],
            "checksum_columns": ["tenant_id", "id", "name"],
            "column_types": ["varchar", "bigint", "varchar"], "has_generated_columns": False,
            "foreign_key_dependencies": [], "foreign_keys": [], "schema_canonical": "order-schema",
        }
        self.write_owned({"format_version": 1, "tables": [self.table]})

    def write_owned(self, value):
        contents = {
            "src/generated/tenant_catalog.json": json.dumps(value),
            "src/generated/resources.json": json.dumps({"format_version": 1, "module": "order",
                "resources": [{"table": "biz_order", "database": "tenant"}]}),
        }
        entries = []
        for relative, content in contents.items():
            (self.package / relative).write_text(content, encoding="utf-8")
            entries.append(f'[[entries]]\nresource = "__catalog__"\npath = "{relative}"\n'
                           f'content_hash = "{hashlib.sha256(content.encode()).hexdigest()}"\n')
        (self.package / ".ryframe/generated.toml").write_text("format_version = 1\n" + "\n".join(entries))

    def test_complete_owned_business_columns_are_included(self):
        self.assertEqual(business_tables(self.root), [self.table])
        self.assertEqual(business_columns(self.root), {"biz_order": {
            "tenant_id": "VARCHAR", "id": "BIGINT", "name": "VARCHAR"}})

    def test_modified_generated_asset_is_rejected(self):
        (self.package / "src/generated/tenant_catalog.json").write_text("{}")
        with self.assertRaisesRegex(ValueError, "受管资产"):
            business_tables(self.root)

    def test_resource_and_table_catalog_must_match(self):
        self.write_owned({"format_version": 1, "tables": []})
        with self.assertRaisesRegex(ValueError, "资源与租户表"):
            business_tables(self.root)
        self.write_owned({"format_version": 1, "tables": [self.table, self.table]})
        with self.assertRaisesRegex(ValueError, "重复声明租户表"):
            business_tables(self.root)

    def test_incomplete_columns_cursor_and_schema_fail_closed(self):
        for field, value in [("checksum_columns", ["id"]), ("column_types", []),
                             ("primary_key_cursor_columns", ["unknown"]), ("schema_canonical", "")]:
            table = copy.deepcopy(self.table)
            table[field] = value
            self.write_owned({"format_version": 1, "tables": [table]})
            with self.subTest(field=field), self.assertRaises(ValueError):
                business_tables(self.root)

    def test_unknown_workspace_path_is_rejected(self):
        (self.root / "Cargo.toml").write_text('[workspace]\nmembers = ["../unowned"]\n')
        with self.assertRaisesRegex(ValueError, "源码根目录"):
            business_tables(self.root)

    def test_business_ledger_columns_follow_formal_sdk_ddl(self):
        backend = Path(__file__).resolve().parents[2]
        self.assertEqual(business_ledger_columns(backend), {
            "scope": "VARCHAR", "origin": "VARCHAR", "tables_json": "JSON", "adoption_hash": "CHAR",
            "module_name": "VARCHAR", "migration_id": "VARCHAR", "checksum": "CHAR",
            "before_fingerprint": "CHAR", "after_fingerprint": "CHAR", "applied_at": "TIMESTAMP",
        })

    def test_business_ledger_rejects_ambiguous_or_incomplete_ddl(self):
        backend = Path(__file__).resolve().parents[2]
        source = (backend / "crates/ryframe-sdk/src/migration/ledger.rs").read_text(encoding="utf-8")
        target = self.root / "crates/ryframe-sdk/src/migration/ledger.rs"
        target.parent.mkdir(parents=True)
        for invalid in [source + source, source.replace("tables_json JSON", "tables_json TEXT"), source.replace("const DDL:", "const UNKNOWN:")]:
            target.write_text(invalid, encoding="utf-8")
            with self.subTest(invalid=invalid[:30]), self.assertRaises(ValueError):
                business_ledger_columns(self.root)


if __name__ == "__main__":
    unittest.main()
