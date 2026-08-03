from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path
from unittest import mock


MODULE_PATH = Path(__file__).resolve().parents[1] / "validate_release.py"
SPEC = importlib.util.spec_from_file_location("validate_release", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
validate_release = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = validate_release
SPEC.loader.exec_module(validate_release)


COMMIT_A = "a" * 40
COMMIT_B = "b" * 40


class ReleaseIdentityTests(unittest.TestCase):
    def test_accepts_only_canonical_stable_tags(self) -> None:
        identity = validate_release.release_identity("v0.5.0")

        self.assertEqual(identity.tag, "v0.5.0")
        self.assertEqual(identity.version, "0.5.0")
        self.assertEqual(identity.stable_tag, "v0.5.0")

    def test_rejects_prerelease_and_ambiguous_tags(self) -> None:
        for tag in (
            "0.5.0",
            "v0.5.0-rc.1",
            "v0.5.0-beta.1",
            "v0.5",
            "v01.5.0",
        ):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                validate_release.release_identity(tag)


class ChangelogNotesTests(unittest.TestCase):
    @staticmethod
    def changelog(content: str) -> mock.Mock:
        path = mock.Mock(spec=Path)
        path.read_text.return_value = content
        return path

    def test_extracts_exact_section_and_stops_at_next_version(self) -> None:
        path = self.changelog(
            "# Changelog\r\n\r\n"
            "## [v0.5.0] - 2026-07-18\r\n\r\n"
            "### Changed  \r\n\r\n"
            "- Published manifest-only releases.  \r\n\r\n"
            "## [v0.4.0]\r\n\r\n- Older change.\r\n"
        )

        section = validate_release.changelog_section(path, "v0.5.0", "test")

        self.assertEqual(
            section,
            "## [v0.5.0] - 2026-07-18\n\n"
            "### Changed\n\n- Published manifest-only releases.",
        )
        self.assertNotIn("Older change", section)

    def test_rejects_missing_or_content_free_section(self) -> None:
        cases = (
            "## [v0.5.1]\n\n- Different version.\n",
            "## [v0.5.0]\n",
            "## [v0.5.0]\n\n### Changed\n",
        )
        for content in cases:
            with self.subTest(content=content), self.assertRaises(ValueError):
                validate_release.changelog_section(
                    self.changelog(content), "v0.5.0", "test"
                )

    def test_requires_annotated_tag_notes_to_match_stable_section(self) -> None:
        path = self.changelog("## [v0.5.0]\n\n### Changed\n\n- Real update.\n")
        notes = "## [v0.5.0]  \r\n\r\n### Changed\r\n\r\n- Real update.  \r\n"
        with mock.patch.object(
            validate_release, "git_text", side_effect=("tag", notes)
        ):
            section = validate_release.validate_annotated_tag_notes(
                Path("repository"), "v0.5.0", path, "v0.5.0", "test"
            )

        self.assertIn("- Real update.", section)

    def test_rejects_lightweight_or_generic_tag_notes(self) -> None:
        path = self.changelog("## [v0.5.0]\n\n### Changed\n\n- Real update.\n")
        with mock.patch.object(
            validate_release, "git_text", return_value="commit"
        ), self.assertRaisesRegex(ValueError, "annotated tag"):
            validate_release.validate_annotated_tag_notes(
                Path("repository"), "v0.5.0", path, "v0.5.0", "test"
            )

        with mock.patch.object(
            validate_release,
            "git_text",
            side_effect=("tag", "RyFrame v0.5.0"),
        ), self.assertRaisesRegex(ValueError, "exact v0.5.0 CHANGELOG"):
            validate_release.validate_annotated_tag_notes(
                Path("repository"), "v0.5.0", path, "v0.5.0", "test"
            )


class FixedSourceTests(unittest.TestCase):
    def test_repository_and_commit_inputs_are_strict(self) -> None:
        self.assertEqual(
            validate_release.repository_slug("example-org/ryframe-vue3", "frontend"),
            "example-org/ryframe-vue3",
        )
        self.assertEqual(validate_release.commit_sha(COMMIT_A, "frontend"), COMMIT_A)
        for value in ("ryframe-vue3", "owner/repo/extra", "owner/ repo"):
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_release.repository_slug(value, "frontend")
        for value in ("abc", "A" * 40, "g" * 40):
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_release.commit_sha(value, "frontend")

    def test_fixed_tag_commit_must_match(self) -> None:
        with mock.patch.object(validate_release, "git_commit", return_value=COMMIT_A), mock.patch.object(
            validate_release, "git_tag_object", return_value=COMMIT_B
        ):
            ref = validate_release.validate_repository_ref(
                Path("frontend"), "v0.5.0", COMMIT_A, "frontend"
            )
        self.assertEqual(ref.commit, COMMIT_A)
        self.assertEqual(ref.tag_object, COMMIT_B)

        with mock.patch.object(validate_release, "git_commit", return_value=COMMIT_B):
            with self.assertRaisesRegex(ValueError, "expected fixed commit"):
                validate_release.validate_repository_ref(
                    Path("frontend"), "v0.5.0", COMMIT_A, "frontend"
                )

    def test_frontend_version_and_contract_are_validated(self) -> None:
        frontend = MODULE_PATH.parent
        with mock.patch.object(
            validate_release, "validate_package_version"
        ) as package_version, mock.patch.object(
            validate_release, "validate_openapi", return_value="c" * 64
        ) as openapi, mock.patch.object(
            validate_release, "validate_annotated_tag_notes"
        ), mock.patch.object(
            validate_release,
            "validate_repository_ref",
            return_value=validate_release.RepositoryRef("", COMMIT_B, COMMIT_A),
        ):
            ref, openapi_hash = validate_release.validate_frontend(
                frontend, "v0.5.0", "0.5.0", "v0.5.0", COMMIT_A
            )

        self.assertEqual(ref.commit, COMMIT_A)
        self.assertEqual(openapi_hash, "c" * 64)
        package_version.assert_called_once_with(frontend, "0.5.0")
        openapi.assert_called_once_with(
            frontend / "openapi" / "openapi.json", "0.5.0", "frontend"
        )

    def test_frontend_package_version_mismatch_is_rejected(self) -> None:
        with mock.patch.object(
            validate_release, "json_object", return_value={"version": "0.5.1"}
        ), self.assertRaisesRegex(ValueError, "package.json version"):
            validate_release.validate_package_version(Path("frontend"), "0.5.0")

    def test_frontend_contract_source_must_pin_the_release_backend(self) -> None:
        source = {
            "schema_version": 1,
            "backend_repository": "example/ryframe",
            "backend_commit": COMMIT_A,
            "openapi_path": "openapi/openapi.json",
            "sha256": "c" * 64,
        }
        with mock.patch.object(validate_release, "json_object", return_value=source):
            validate_release.validate_frontend_contract_source(
                Path("frontend"), "example/ryframe", COMMIT_A, "c" * 64
            )

        for field, value in (
            ("backend_repository", "other/ryframe"),
            ("backend_commit", COMMIT_B),
            ("sha256", "d" * 64),
        ):
            mismatched = {**source, field: value}
            with self.subTest(field=field), mock.patch.object(
                validate_release, "json_object", return_value=mismatched
            ), self.assertRaisesRegex(ValueError, field):
                validate_release.validate_frontend_contract_source(
                    Path("frontend"), "example/ryframe", COMMIT_A, "c" * 64
                )


class ManifestTests(unittest.TestCase):
    def test_manifest_is_deterministic_and_records_both_fixed_sources(self) -> None:
        identity = validate_release.release_identity("v0.5.0")
        backend = validate_release.RepositoryRef("example/ryframe", COMMIT_A, COMMIT_A)
        frontend = validate_release.RepositoryRef(
            "example/ryframe-vue3", COMMIT_B, COMMIT_B
        )
        contract_hash = "c" * 64
        manifest = validate_release.release_manifest(
            identity, backend, frontend, contract_hash, contract_hash
        )

        first = validate_release.serialize_manifest(manifest)
        second = validate_release.serialize_manifest(manifest)
        self.assertEqual(first, second)
        written = json.loads(first)

        self.assertEqual(written["release"], {"tag": "v0.5.0", "version": "0.5.0"})
        self.assertEqual(written["backend"]["commit"], COMMIT_A)
        self.assertEqual(written["backend"]["version"], "0.5.0")
        self.assertEqual(written["frontend"]["commit"], COMMIT_B)
        self.assertEqual(written["frontend"]["version"], "0.5.0")
        self.assertEqual(written["contract"]["openapi_sha256"], contract_hash)
        self.assertEqual(written["schema_version"], 1)
        self.assertNotIn("artifacts", written)

    def test_manifest_rejects_different_contract_hashes(self) -> None:
        identity = validate_release.release_identity("v0.5.0")
        backend = validate_release.RepositoryRef("example/ryframe", COMMIT_A, COMMIT_A)
        frontend = validate_release.RepositoryRef(
            "example/ryframe-vue3", COMMIT_B, COMMIT_B
        )
        with self.assertRaisesRegex(ValueError, "OpenAPI hashes differ"):
            validate_release.release_manifest(
                identity, backend, frontend, "a" * 64, "b" * 64
            )


if __name__ == "__main__":
    unittest.main()
