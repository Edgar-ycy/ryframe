import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from check_removed_identity import check
from test_release_stage import temporary_workspace


class RemovedIdentityTests(unittest.TestCase):
    def test_product_definitions_fail_without_scanning_history_and_future_notes(self):
        parent = Path.cwd() / ".local-tests"
        parent.mkdir(exist_ok=True)
        with temporary_workspace(parent) as root:
            frontend = root / "frontend"
            files = {
                "Cargo.toml": "[workspace]\n",
                "crates/api/Cargo.toml": "[dependencies]\n",
                "crates/api/src/lib.rs": "pub fn users() {}\n",
                "crates/api/src/response.rs": "pub struct StandardEnvelope;\n",
                "crates/api/build_support/catalog.rs": "pub const TAGS: &[&str] = &[];\n",
                "openapi/openapi.json": "{}",
                "sql/ryframe_config.sql": "CREATE TABLE sys_product (id BIGINT);",
                "config/default.toml": "[auth]\n",
                "catalog/access.toml": 'permissions = ["user:list"]',
                "frontend/package.json": "{}",
                "frontend/openapi/openapi.json": "{}",
                "frontend/src/index.ts": "export const capabilityCatalog = []",
                "frontend/scripts/generate.mjs": "export const domains = ['core']",
                "frontend/scripts/headers.ts": "export const headers = ['Accept']",
                "frontend/vite.config.ts": "export default {}",
                "CHANGELOG.md": "service_accounts 历史功能",
                "AGENTS.md": "ServiceAccount 未来可能重新设计",
                "scripts/tests/removed.txt": "sys_service_account 删除断言",
            }
            for relative, content in files.items():
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(content, encoding="utf-8")
            self.assertEqual(check(root, frontend), [])
            cases = {
                "crates/api/Cargo.toml": 'service-accounts = "1"',
                "crates/api/src/lib.rs": "pub struct ServiceDelegation;",
                "crates/api/src/response.rs": "pub struct PrebuiltApiEnvelope; // Agent 查询",
                "crates/api/build_support/catalog.rs": 'const TAG: &str = "服务账号";',
                "config/default.toml": "[service_accounts]",
                "sql/ryframe_config.sql": "CREATE TABLE sys_service_credential (id BIGINT);",
                "openapi/openapi.json": '{"/api/v1/agent/v1/query": {}}',
                "catalog/access.toml": 'key = "service-accounts"',
                "frontend/src/index.ts": 'export { ServiceAccessAudit } from "./audit"',
                "frontend/scripts/generate.mjs": "export const key = 'RyFrameApiKey'",
                "frontend/scripts/headers.ts": "export const key = 'X-RyFrame-Delegation'",
            }
            for relative, content in cases.items():
                with self.subTest(relative=relative):
                    target = root / relative
                    target.write_text(content, encoding="utf-8")
                    self.assertTrue(check(root, frontend))
                    target.write_text(files[relative], encoding="utf-8")

            source = root / "crates/api/src/lib.rs"
            for symbol in (
                "agent_query",
                "agent_capability",
                "agent_directory",
                "agent_limiter",
                "AgentPageQuery",
                "AgentCapabilityResponse",
                "AgentUserResponse",
                "AgentDepartmentResponse",
                "AgentPostResponse",
                "AgentDictionaryItemResponse",
                "AgentDictionaryResponse",
            ):
                with self.subTest(symbol=symbol):
                    source.write_text(f"pub fn {symbol}() {{}}", encoding="utf-8")
                    self.assertTrue(check(root, frontend))
                    source.write_text(files["crates/api/src/lib.rs"], encoding="utf-8")

            removed_paths = [
                "crates/ryframe-application/src/agent/mod.rs",
                "crates/ryframe-application/src/ports/service_accounts/mod.rs",
                "crates/ryframe-application/src/system/service_account/mod.rs",
                "crates/ryframe-application/src/service_identity_secret.rs",
                "crates/ryframe-api/src/dto/agent_dto.rs",
                "crates/ryframe-api/src/handlers/agent_handler.rs",
                "crates/ryframe-db/src/application_ports/agent/mod.rs",
                "crates/ryframe-db/src/application_ports/service_accounts/mod.rs",
                "crates/ryframe-db/src/repositories/service_authorization_repo.rs",
                "frontend/src/api/generated/operations/agent.ts",
                "frontend/src/api/generated/schema/agent.ts",
                "frontend/src/features/service-accounts/manifest.ts",
                "frontend/src/views/system/service-accounts/index.vue",
            ]
            for relative in removed_paths:
                with self.subTest(relative=relative):
                    target = root / relative
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_text("", encoding="utf-8")
                    self.assertTrue(check(root, frontend))
                    target.unlink()


if __name__ == "__main__":
    unittest.main()
