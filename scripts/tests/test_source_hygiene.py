from importlib.util import module_from_spec, spec_from_file_location
from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = spec_from_file_location(
    "check_source_hygiene", ROOT / "scripts" / "check_source_hygiene.py"
)
assert SPEC is not None and SPEC.loader is not None
CHECKER = module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKER)


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
