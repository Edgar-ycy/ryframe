from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = spec_from_file_location(
    "check_source_hygiene", ROOT / "scripts" / "check_source_hygiene.py"
)
assert SPEC is not None and SPEC.loader is not None
CHECKER = module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


class RustIdentifierLintPolicyTest(unittest.TestCase):
    def test_all_workspace_members_forbid_non_ascii_identifiers(self) -> None:
        workspace_manifest = tomllib.loads(
            (ROOT / "Cargo.toml").read_text(encoding="utf-8")
        )
        workspace = workspace_manifest["workspace"]

        self.assertEqual(
            workspace["lints"]["rust"]["non_ascii_idents"],
            "forbid",
        )
        for member in workspace["members"]:
            with self.subTest(member=member):
                member_manifest = tomllib.loads(
                    (ROOT / member / "Cargo.toml").read_text(encoding="utf-8")
                )
                self.assertIs(member_manifest.get("lints", {}).get("workspace"), True)


class SourceHygieneCommentLanguageTest(unittest.TestCase):
    def test_javascript_comments_must_contain_chinese_text(self) -> None:
        violations = CHECKER.comment_language_violations(
            "deploy/tests/example.js", "// English explanation\n", ".js"
        )

        self.assertEqual(
            violations,
            [
                "deploy/tests/example.js:1: explanatory comment must contain Chinese text"
            ],
        )

    def test_configuration_and_dockerfile_comments_are_checked(self) -> None:
        self.assertEqual(
            CHECKER.comment_language_violations(
                "deploy/nginx/example.conf", "# 中文说明\n", ".conf"
            ),
            [],
        )
        self.assertEqual(
            CHECKER.comment_language_violations(
                "deploy/Dockerfile", "# syntax=docker/dockerfile:1.7\n", ".dockerfile"
            ),
            [],
        )
        self.assertEqual(
            CHECKER.comment_language_suffix(Path("deploy/Dockerfile")), ".dockerfile"
        )

    def test_english_configuration_comments_are_rejected(self) -> None:
        violations = CHECKER.comment_language_violations(
            "deploy/nginx/example.conf", "# English explanation\n", ".conf"
        )

        self.assertEqual(
            violations,
            [
                "deploy/nginx/example.conf:1: explanatory comment must contain Chinese text"
            ],
        )

    def test_single_english_words_are_not_technical_comment_exceptions(self) -> None:
        violations = CHECKER.comment_language_violations(
            "crates/example.rs", "// Explain\n", ".rs"
        )

        self.assertEqual(
            violations,
            ["crates/example.rs:1: explanatory comment must contain Chinese text"],
        )

    def test_inline_comments_are_checked_but_strings_are_not(self) -> None:
        violations = CHECKER.comment_language_violations(
            "crates/example.rs",
            'let endpoint = "https://example.test"; // English explanation\n'
            'let marker = "/* not a comment */"; // 中文说明\n',
            ".rs",
        )

        self.assertEqual(
            violations,
            ["crates/example.rs:1: explanatory comment must contain Chinese text"],
        )

    def test_rust_raw_strings_and_inline_configuration_comments_are_scanned_safely(self) -> None:
        self.assertEqual(
            CHECKER.comment_language_violations(
                "crates/example.rs",
                'let query = r#"-- not a comment"#; // 中文说明\n',
                ".rs",
            ),
            [],
        )
        self.assertEqual(
            CHECKER.comment_language_violations(
                "config/example.toml",
                'endpoint = "https://example.test/#anchor" # English explanation\n',
                ".toml",
            ),
            ["config/example.toml:1: explanatory comment must contain Chinese text"],
        )

    def test_rust_character_literals_do_not_hide_later_comments(self) -> None:
        self.assertEqual(
            CHECKER.comment_language_violations(
                "crates/example.rs",
                "let quote = b'\"'; // 中文说明\n",
                ".rs",
            ),
            [],
        )


class SourceHygieneIgnoredTestPolicyTest(unittest.TestCase):
    def test_unallowlisted_ignored_test_in_src_is_rejected(self) -> None:
        relative = "crates/example/src/lib.rs"
        source = '#[test]\n#[ignore = "需要外部服务"]\nfn external_test() {}\n'

        self.assertEqual(
            CHECKER.ignored_test_violations(relative, source),
            [f"{relative}: ignored test is not allowlisted (external_test)"],
        )

    def test_src_allowlist_requires_exact_path_and_test_name(self) -> None:
        relative = (
            "crates/ryframe-service/src/system/online_user_service/redis_backend.rs"
        )
        allowed = (
            '#[ignore = "requires Docker Compose Redis service"]\n'
            "async fn stale_touch_cannot_resurrect_or_overwrite_online_user_index() {}\n"
        )
        unexpected = allowed.replace(
            "stale_touch_cannot_resurrect_or_overwrite_online_user_index",
            "another_ignored_test",
        )

        self.assertEqual(CHECKER.ignored_test_violations(relative, allowed), [])
        self.assertEqual(
            CHECKER.ignored_test_violations(relative, unexpected),
            [f"{relative}: ignored test is not allowlisted (another_ignored_test)"],
        )

    def test_export_runtime_allowlist_requires_exact_path_and_test_name(self) -> None:
        relative = "crates/ryframe-service/tests/export_runtime_acceptance_test.rs"
        test_name = (
            "export_runtime_acceptance_covers_scale_takeover_"
            "storage_recovery_and_cleanup"
        )
        allowed = f'#[ignore = "需要隔离 MySQL 与 RustFS"]\nasync fn {test_name}() {{}}\n'

        self.assertEqual(CHECKER.ignored_test_violations(relative, allowed), [])
        self.assertEqual(
            CHECKER.ignored_test_violations(
                "crates/ryframe-service/tests/another_test.rs",
                allowed,
            ),
            [
                "crates/ryframe-service/tests/another_test.rs: "
                f"ignored test is not allowlisted ({test_name})"
            ],
        )
        unexpected = allowed.replace(test_name, "another_export_runtime_acceptance")
        self.assertEqual(
            CHECKER.ignored_test_violations(relative, unexpected),
            [f"{relative}: ignored test is not allowlisted (another_export_runtime_acceptance)"],
        )
