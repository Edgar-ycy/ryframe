from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "check_architecture.py"
SPEC = importlib.util.spec_from_file_location("check_architecture", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
CHECK_ARCHITECTURE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECK_ARCHITECTURE)


class ArchitectureSecurityTest(unittest.TestCase):
    def test_current_sources_do_not_expose_unsigned_replay_headers(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_unsigned_replay_contract(errors)
        self.assertEqual(errors, [])

    def test_feature_registry_matches_workspace_metadata(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_feature_registry(errors)
        self.assertEqual(errors, [])

    def test_feature_registry_rejects_an_unregistered_feature(self) -> None:
        metadata = {
            "workspace_members": ["package-a"],
            "packages": [
                {
                    "id": "package-a",
                    "name": "package-a",
                    "features": {"default": [], "fast": []},
                }
            ],
        }
        registry = {
            "version": 1,
            "packages": [
                {"package": "package-a", "minimal": [], "maximal": ["default"]}
            ],
        }

        violations = CHECK_ARCHITECTURE.feature_registry_violations(metadata, registry)
        self.assertTrue(
            any("does not exactly cover features" in violation for violation in violations)
        )

    def test_api_prefix_has_one_runtime_source(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_api_prefix_contract(errors)
        self.assertEqual(errors, [])

    def test_backend_openapi_api_prefix_matches_exact_canonical_contract(self) -> None:
        document = {
            "x-ryframe-api-prefix": {"version": 1, "value": "/api/v1"}
        }
        self.assertEqual(
            CHECK_ARCHITECTURE.openapi_api_prefix_violations(document, "/api/v1"),
            [],
        )

    def test_backend_openapi_api_prefix_rejects_missing_or_drifted_contract(self) -> None:
        invalid_documents = (
            {},
            {"x-ryframe-api-prefix": {"version": 1, "value": "/api/v2"}},
            {
                "x-ryframe-api-prefix": {
                    "version": 1,
                    "value": "/api/v1",
                    "legacy": True,
                }
            },
        )
        for document in invalid_documents:
            with self.subTest(document=document):
                self.assertTrue(
                    CHECK_ARCHITECTURE.openapi_api_prefix_violations(
                        document, "/api/v1"
                    )
                )

    def test_backend_openapi_json_success_responses_use_unified_envelopes(self) -> None:
        document = {
            "paths": {
                "/api/v1/version": {
                    "get": {
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "$ref": "#/components/schemas/ApiResponse_ApiVersionInfo"
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
                "/api/v1/common/file/download": {
                    "get": {
                        "responses": {
                            "200": {
                                "content": {
                                    "application/octet-stream": {
                                        "schema": {"type": "string"}
                                    }
                                }
                            }
                        }
                    }
                },
                "/livez": {
                    "get": {
                        "responses": {
                            "200": {
                                "content": {
                                    "application/json": {
                                        "schema": {
                                            "$ref": "#/components/schemas/LivenessResponse"
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
        }

        self.assertEqual(
            CHECK_ARCHITECTURE.openapi_json_success_envelope_violations(
                document, "/api/v1"
            ),
            [],
        )

    def test_backend_openapi_raw_json_success_response_is_rejected(self) -> None:
        document = {
            "paths": {
                "/api/v1/version": {
                    "get": {
                        "responses": {
                            "200": {
                                "content": {
                                    "application/problem+json": {
                                        "schema": {
                                            "$ref": "#/components/schemas/ApiVersionInfo"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        violations = CHECK_ARCHITECTURE.openapi_json_success_envelope_violations(
            document, "/api/v1"
        )
        self.assertEqual(len(violations), 1)
        self.assertIn("bypasses the unified envelope", violations[0])

    def test_response_envelope_uses_bounded_production_buffering(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_response_envelope_boundary(errors)
        self.assertEqual(errors, [])

    def test_response_envelope_requires_injected_api_edge_localization(self) -> None:
        response_source = (
            ROOT / "crates/ryframe-middleware/src/response_envelope.rs"
        ).read_text(encoding="utf-8")
        app_source = (ROOT / "crates/ryframe/src/app.rs").read_text(encoding="utf-8")
        violations = CHECK_ARCHITECTURE.response_envelope_policy_violations(
            response_source.replace(
                "localizer.translate(locale, message_key)",
                "message_key.to_owned()",
                1,
            ),
            app_source,
        )

        self.assertIn("response envelope is missing localized response message", violations)

    def test_removed_response_message_constructors_cannot_return(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_response_envelope_boundary(errors)

        self.assertFalse(
            [error for error in errors if "removed response message constructor" in error]
        )

    def test_embedded_swagger_ui_contract_remains_local(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_embedded_swagger_ui(errors)
        self.assertEqual(errors, [])

    def test_unbounded_or_unmounted_response_envelope_is_rejected(self) -> None:
        response_source = (
            ROOT / "crates/ryframe-middleware/src/response_envelope.rs"
        ).read_text(encoding="utf-8")
        app_source = (ROOT / "crates/ryframe/src/app.rs").read_text(encoding="utf-8")
        unbounded = response_source.replace(
            "to_bytes(body, API_JSON_RESPONSE_LIMIT_BYTES).await",
            "to_bytes(body, usize::MAX).await",
            1,
        )
        violations = CHECK_ARCHITECTURE.response_envelope_policy_violations(
            unbounded,
            app_source.replace(
                "ryframe_middleware::api_response_envelope_middleware",
                "ryframe_middleware::removed_response_envelope_middleware",
                1,
            ),
        )

        self.assertIn(
            "response envelope uses unbounded production body buffering",
            violations,
        )
        self.assertIn(
            "application does not mount the API response envelope middleware",
            violations,
        )

        parts_violations = CHECK_ARCHITECTURE.response_envelope_policy_violations(
            response_source.replace(
                "fn error_response_from_parts(",
                "fn error_response_without_original_parts(",
                1,
            ).replace(
                "Response::from_parts(parts, Body::from(body))",
                "Response::new(Body::from(body))",
                1,
            ),
            app_source,
        )
        self.assertIn(
            "response envelope is missing response parts preservation helper",
            parts_violations,
        )
        self.assertIn(
            "response envelope is missing response parts reuse",
            parts_violations,
        )

    def test_unsigned_replay_header_contract_is_rejected(self) -> None:
        self.assertTrue(
            CHECK_ARCHITECTURE.exposes_unsigned_replay_contract(
                'headers.get("X-Nonce")'
            )
        )
        self.assertTrue(
            CHECK_ARCHITECTURE.exposes_unsigned_replay_contract(
                "headers.get('x-timestamp')"
            )
        )

    def test_standard_message_signature_fields_are_not_blocked(self) -> None:
        self.assertFalse(
            CHECK_ARCHITECTURE.exposes_unsigned_replay_contract(
                'headers.get("Signature-Input"); headers.get("Content-Digest");'
            )
        )

    def test_removed_compatibility_surfaces_cannot_return(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_removed_compatibility_surfaces(errors)
        self.assertEqual(errors, [])

    def test_removed_operation_log_job_cannot_return(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_removed_oper_log_job(errors)
        self.assertEqual(errors, [])

    def test_removed_operation_log_job_symbols_are_rejected(self) -> None:
        legacy_sources = {
            "constant.rs": "pub const " + "OPER_LOG_" + "JOB_TYPE: &str = \"removed\";",
            "handler.rs": "pub struct " + "OperLog" + "JobHandler;",
            "enqueue.rs": "queue." + "enqueue_oper" + "_log(payload).await;",
            "type.rs": "let kind = \"system." + "oper_log." + "record\";",
        }
        violations = CHECK_ARCHITECTURE.removed_oper_log_job_violations(legacy_sources)
        self.assertEqual(len(violations), 4)

    def test_removed_configuration_crypto_cannot_return(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_secret_source_policy(errors)
        self.assertEqual(errors, [])

    def test_removed_configuration_crypto_symbols_are_rejected(self) -> None:
        for source in (
            "mod config_crypto;",
            "ConfigCrypto::from_env()",
            'std::env::var("CONFIG_MASTER_KEY")',
        ):
            with self.subTest(source=source):
                self.assertTrue(
                    CHECK_ARCHITECTURE.exposes_removed_config_crypto(source)
                )

    def test_production_configuration_secret_values_are_detected(self) -> None:
        config = {
            "database": {
                "primary": {"password": ""},
                "replicas": [{"connection": {"password": "replica-secret"}}],
            },
            "auth": {"jwt_secret": "change-me-in-production"},
        }
        self.assertEqual(
            CHECK_ARCHITECTURE.configured_secret_paths(config),
            ["database.replicas[0].connection.password"],
        )

    def test_removed_repository_wrapper_cannot_return(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_removed_repository_wrapper(errors)
        self.assertEqual(errors, [])

    def test_removed_database_cluster_constructors_cannot_return(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_database_and_storage_topology(errors)
        self.assertFalse(
            [error for error in errors if "restores a removed constructor" in error]
        )

    def test_api_public_dto_boundary_remains_transport_owned(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_public_dto_boundary(errors)
        self.assertEqual(errors, [])

    def test_public_dto_compatibility_shortcuts_are_rejected(self) -> None:
        source = """
pub type PublicDto = ServiceDto;
impl From<ServiceDto> for PublicDto {
    fn from(value: ServiceDto) -> Self {
        let ServiceDto { id, .. } = value;
        match id {
            _ => todo!(),
        }
    }
}
"""
        violations = CHECK_ARCHITECTURE.public_dto_conversion_violations(source)
        self.assertIn(
            "public DTO conversions must not use a rest-pattern destructure",
            violations,
        )
        self.assertIn(
            "public DTO enum conversions must not use wildcard match arms",
            violations,
        )
        self.assertIn(
            "public DTO boundary must not expose compatibility type aliases",
            violations,
        )

    def test_messaging_runtime_policy_remains_config_driven(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_messaging_runtime_policy(errors)
        self.assertEqual(errors, [])

    def test_logging_retention_policy_remains_bounded(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_logging_retention_policy(errors)
        self.assertEqual(errors, [])

    def test_file_runtime_is_sha256_and_upload_status_only(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_file_digest_runtime_policy(errors)
        self.assertEqual(errors, [])

    def test_message_time_precision_matches_database_clock(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_message_time_precision(errors)
        self.assertEqual(errors, [])

    def test_legacy_file_compatibility_code_is_rejected_from_runtime(self) -> None:
        source = """
let legacy_md5 = format!("{:x}", md5::compute(bytes));
query.filter(sys_file::Column::FileMd5.eq(legacy_md5));
query.filter(sys_file::Column::DelFlag.eq("3"));
"""
        violations = CHECK_ARCHITECTURE.file_runtime_policy_violations(source)
        self.assertIn("md5 implementation", violations)
        self.assertIn("legacy MD5 variable", violations)
        self.assertIn("legacy MD5 entity access", violations)
        self.assertIn("legacy upload reservation literal", violations)

    def test_null_legacy_column_write_is_also_rejected_from_runtime(self) -> None:
        self.assertIn(
            "legacy MD5 entity access",
            CHECK_ARCHITECTURE.file_runtime_policy_violations("file_md5: None,"),
        )

    def test_unvalidated_pagination_cannot_return(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_validated_pagination_boundary(errors)
        self.assertEqual(errors, [])

    def test_raw_service_pagination_parameters_are_rejected(self) -> None:
        source = """
pub async fn list_page(&self, page: u64, page_size: usize) {}
fn slice_rows(rows: Vec<Row>, offset: i64) {}
"""
        self.assertEqual(
            CHECK_ARCHITECTURE.raw_pagination_parameters(source),
            [
                ("list_page", "page", "u64"),
                ("list_page", "page_size", "usize"),
                ("slice_rows", "offset", "i64"),
            ],
        )

    def test_validated_page_value_is_not_reported_as_raw_pagination(self) -> None:
        source = """
pub async fn list_page(&self, page: ValidatedPageQuery) {}
"""
        self.assertEqual(CHECK_ARCHITECTURE.raw_pagination_parameters(source), [])

    def test_readiness_handlers_only_read_background_snapshots(self) -> None:
        errors: list[str] = []
        CHECK_ARCHITECTURE.check_readiness_snapshot_boundary(errors)
        self.assertEqual(errors, [])

    def test_direct_network_io_in_readiness_handler_is_rejected(self) -> None:
        source = """
async fn worker_readyz() {
    redis.ping().await;
    file_service.check_storage().await;
}
"""
        self.assertTrue(
            CHECK_ARCHITECTURE.readiness_handler_performs_network_io(
                source, "worker_readyz"
            )
        )


if __name__ == "__main__":
    unittest.main()
