import io
import sys
import unittest
from pathlib import Path
from unittest.mock import patch


sys.path.insert(0, str(Path(__file__).resolve().parent))

from release_private_protocol import (
    EVIDENCE_MODE,
    SOURCE_MODE,
    ProtocolError,
    load_protocol,
)
import validate_release


def source_environment(**overrides):
    values = {
        "RYFRAME_RELEASE_PROTOCOL_VERSION": "1",
        "RYFRAME_RELEASE_MODE": SOURCE_MODE,
        "RYFRAME_RELEASE_TAG": "v0.12.1",
        "RYFRAME_RELEASE_FRONTEND_DIR": "D:/项目/前端 空格",
        "RYFRAME_RELEASE_BACKEND_REPOSITORY": "owner/backend",
        "RYFRAME_RELEASE_BACKEND_COMMIT": "a" * 40,
        "RYFRAME_RELEASE_FRONTEND_REPOSITORY": "owner/frontend",
        "RYFRAME_RELEASE_FRONTEND_COMMIT": "b" * 40,
        "RYFRAME_RELEASE_MANIFEST_PATH": "D:/项目/证据/清单.json",
    }
    values.update(overrides)
    return values


class ReleasePrivateProtocolTests(unittest.TestCase):
    def test_rejects_every_command_line_argument(self):
        with self.assertRaisesRegex(ProtocolError, "不接受命令行参数"):
            load_protocol(
                {SOURCE_MODE},
                arguments=["--tag", "v0.12.1"],
                environment=source_environment(),
            )

    def test_rejects_missing_version_or_field(self):
        cases = (
            {"RYFRAME_RELEASE_PROTOCOL_VERSION": None},
            {"RYFRAME_RELEASE_MANIFEST_PATH": None},
        )
        for removals in cases:
            environment = source_environment()
            for key in removals:
                environment.pop(key)
            with self.subTest(key=next(iter(removals))), self.assertRaises(
                ProtocolError
            ):
                load_protocol(
                    {SOURCE_MODE},
                    arguments=[],
                    environment=environment,
                )

    def test_rejects_unknown_mode_and_cross_mode_field(self):
        unknown_mode = source_environment(RYFRAME_RELEASE_MODE="future-mode")
        with self.assertRaisesRegex(ProtocolError, "协议模式"):
            load_protocol(
                {SOURCE_MODE},
                arguments=[],
                environment=unknown_mode,
            )
        unknown_field = source_environment(RYFRAME_RELEASE_OUTPUT_PATH="D:/out.json")
        with self.assertRaisesRegex(ProtocolError, "未知字段"):
            load_protocol(
                {SOURCE_MODE},
                arguments=[],
                environment=unknown_field,
            )

    def test_rejects_line_break_but_preserves_utf8_path(self):
        protocol = load_protocol(
            {SOURCE_MODE},
            arguments=[],
            environment=source_environment(),
        )
        self.assertEqual(
            protocol.value("MANIFEST_PATH"),
            "D:/项目/证据/清单.json",
        )
        with self.assertRaisesRegex(ProtocolError, "换行"):
            load_protocol(
                {SOURCE_MODE},
                arguments=[],
                environment=source_environment(
                    RYFRAME_RELEASE_MANIFEST_PATH="D:/证据\n/清单.json"
                ),
            )

    def test_validate_release_uses_protocol_and_rejects_argv_with_code_two(self):
        with patch("sys.argv", ["validate_release.py", "--tag"]), patch.dict(
            "os.environ", source_environment(), clear=True
        ), patch("sys.stderr", io.StringIO()):
            self.assertEqual(validate_release.main(), 2)

        with patch("sys.argv", ["validate_release.py"]), patch.dict(
            "os.environ", source_environment(), clear=True
        ):
            request = validate_release.source_request()
        self.assertEqual(request.frontend_dir, Path("D:/项目/前端 空格"))
        self.assertEqual(request.manifest_path, Path("D:/项目/证据/清单.json"))

    def test_expected_mode_cannot_consume_another_script_protocol(self):
        environment = {
            "RYFRAME_RELEASE_PROTOCOL_VERSION": "1",
            "RYFRAME_RELEASE_MODE": EVIDENCE_MODE,
            "RYFRAME_RELEASE_BACKEND_REPOSITORY": "owner/backend",
            "RYFRAME_RELEASE_FRONTEND_REPOSITORY": "owner/frontend",
            "RYFRAME_RELEASE_BACKEND_SHA": "a" * 40,
            "RYFRAME_RELEASE_FRONTEND_SHA": "b" * 40,
            "RYFRAME_RELEASE_TAG": "v0.12.1",
            "RYFRAME_RELEASE_TIMEOUT_SECONDS": "5",
            "RYFRAME_RELEASE_OUTPUT_PATH": "D:/evidence.json",
        }
        with self.assertRaisesRegex(ProtocolError, "协议模式"):
            load_protocol(
                {SOURCE_MODE},
                arguments=[],
                environment=environment,
            )


if __name__ == "__main__":
    unittest.main()
