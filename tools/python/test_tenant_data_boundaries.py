import shutil
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from tenant_data_boundaries import validate_tenant_data_boundaries
from workspace_directory import WorkspaceDirectory

ROOT = Path(__file__).resolve().parents[2]
CATALOG = "crates/ryframe-tenant-db/src/generated/catalog.rs"


class TenantDataBoundaryTests(unittest.TestCase):
    def setUp(self):
        local = ROOT / ".local-tests/python-unit"
        local.mkdir(parents=True, exist_ok=True)
        self.directory = WorkspaceDirectory(local, "tenant-data-boundaries-")
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for relative in (
            CATALOG, "crates/ryframe-tenant-db/src/generated/mod.rs",
            "crates/ryframe-tenant-db/src/migration/catalog.rs",
            "crates/ryframe-generator/src/resource/render/tenant_catalog.rs",
            "crates/ryframe-generator/src/resource/render/slice/migration.rs",
        ):
            target = self.root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / relative, target)

    def errors(self):
        errors = []
        validate_tenant_data_boundaries(self.root, errors)
        return errors

    def test_current_catalog_is_valid_but_missing_generated_module_is_rejected(self):
        self.assertEqual(self.errors(), [])
        (self.root / CATALOG).unlink()
        self.assertTrue(any("源码缺失" in error for error in self.errors()))

    def test_missing_unknown_or_duplicate_business_table_is_rejected(self):
        directory = self.root / "catalog/resources"
        directory.mkdir(parents=True, exist_ok=True)
        (directory / "business.toml").write_text('[storage]\nkind="tenant_data"\n[database]\ntable="biz_order"\n')
        for content, valid in (('', False), ('table: "biz_unknown"', False),
                               ('table: "biz_order"', True), ('table: "biz_order"\ntable: "biz_order"', False)):
            with self.subTest(content=content):
                (self.root / CATALOG).write_text(content)
                self.assertEqual(not self.errors(), valid)

    def test_generated_repository_cannot_reach_raw_target(self):
        path = self.root / "crates/ryframe-tenant-db/src/generated/business/repository.rs"
        path.parent.mkdir(parents=True)
        path.write_text('router.open_target("arbitrary");')
        self.assertTrue(any("绕过受控会话" in error for error in self.errors()))

    def test_catalog_must_be_compiled_and_bound_to_the_current_fingerprint(self):
        for relative, fragment in (
            ("crates/ryframe-tenant-db/src/generated/mod.rs", "pub mod catalog;"),
            ("crates/ryframe-tenant-db/src/migration/catalog.rs", "computed != tenant_data_schema_fingerprint()"),
        ):
            path = self.root / relative
            source = path.read_text(encoding="utf-8")
            path.write_text(source.replace(fragment, ""), encoding="utf-8")
            self.assertTrue(any(fragment in error for error in self.errors()))
            path.write_text(source, encoding="utf-8")


if __name__ == "__main__":
    unittest.main()
