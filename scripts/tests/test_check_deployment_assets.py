from __future__ import annotations

import importlib.util
import io
import shutil
import tarfile
import unittest
import uuid
from collections.abc import Iterator
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import DEFAULT, patch


SCRIPT = Path(__file__).resolve().parents[1] / "check_deployment_assets.py"
TEMP_ROOT = SCRIPT.parents[1] / "target" / "script-tests"
TEMP_ROOT.mkdir(parents=True, exist_ok=True)
SPEC = importlib.util.spec_from_file_location("check_deployment_assets", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


@contextmanager
def test_workspace() -> Iterator[Path]:
    path = TEMP_ROOT / f"deployment-assets-{uuid.uuid4().hex}"
    path.mkdir(parents=True)
    try:
        yield path
    finally:
        shutil.rmtree(path)


def write_archive(path: Path, extra: dict[str, tuple[bytes, int]] | None = None) -> None:
    files = {
        f"usr/local/bin/{name}": (b"product binary", 0o755)
        for name in MODULE.EXPECTED_BINARIES
    }
    files.update(extra or {})
    with tarfile.open(path, mode="w") as archive:
        for name, (content, mode) in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(content)
            info.mode = mode
            archive.addfile(info, io.BytesIO(content))


class CheckDeploymentAssetsTests(unittest.TestCase):
    def test_cli_runs_static_and_image_checks_in_distinct_phases(self) -> None:
        static_checks = [
            "check_dockerfile",
            "check_online_generator",
            "check_compose_fixture",
            "check_alert_runbooks",
            "check_pinned_actions",
        ]
        with (
            patch.object(MODULE.sys, "argv", [str(SCRIPT)]),
            patch.object(MODULE, "inspect_image", return_value=[]) as inspect_image,
            patch.multiple(
                MODULE,
                **{name: DEFAULT for name in static_checks},
            ) as static_mocks,
        ):
            self.assertEqual(MODULE.main(), 0)
            inspect_image.assert_not_called()
            for check in static_mocks.values():
                check.assert_called_once()

        commit = "a" * 40
        with (
            patch.object(
                MODULE.sys,
                "argv",
                [str(SCRIPT), "--image", "ryframe:test", "--expected-commit", commit],
            ),
            patch.object(MODULE, "inspect_image", return_value=[]) as inspect_image,
            patch.multiple(
                MODULE,
                **{name: DEFAULT for name in static_checks},
            ) as static_mocks,
        ):
            self.assertEqual(MODULE.main(), 0)
            inspect_image.assert_called_once_with("ryframe:test", commit)
            for check in static_mocks.values():
                check.assert_not_called()

    def test_rust_toolchain_versions_stay_aligned(self) -> None:
        cases = [
            ("1.98.0", "1.98", "1.98.0", None),
            ("stable", "1.98", "1.98.0", "完整版本号"),
            ("1.98.0-beta.1", "1.98", "1.98.0", "完整版本号"),
            ("1.98.0", "1.97", "1.98.0", "主次版本一致"),
            ("1.98.0", "1.98", "1.97.1", "完全一致"),
            ("1.98.0", "1.98", "1.98.1", "完全一致"),
        ]
        for channel, minimum, image, expected in cases:
            with self.subTest(channel=channel, minimum=minimum, image=image):
                contents = {
                    "rust-toolchain.toml": f'[toolchain]\nchannel = "{channel}"',
                    "Cargo.toml": f'[workspace.package]\nrust-version = "{minimum}"',
                }
                violations: list[str] = []
                with patch.object(MODULE, "read", side_effect=lambda path: contents[path.name]):
                    MODULE.check_rust_toolchain(
                        f"ARG RUST_IMAGE=rust:{image}-bookworm@sha256:abc\n", violations
                    )
                if expected is None:
                    self.assertEqual(violations, [])
                else:
                    self.assertTrue(any(expected in item for item in violations))

    def test_distroless_archive_passes_without_shell(self) -> None:
        with test_workspace() as directory:
            archive = directory / "filesystem.tar"
            write_archive(archive)

            self.assertEqual(MODULE.inspect_image_archive(archive), [])

    def test_archive_rejects_runtime_tools_and_product_tooling(self) -> None:
        with test_workspace() as directory:
            archive = directory / "filesystem.tar"
            write_archive(
                archive,
                {
                    "bin/sh": (b"shell", 0o755),
                    "opt/ryframe/ryframe-reset": (b"reset", 0o755),
                    "usr/local/bin/ryframe": (b"/api/v1/tools/gen", 0o755),
                },
            )

            violations = MODULE.inspect_image_archive(archive)
            self.assertTrue(any("不得包含 shell" in item for item in violations))
            self.assertTrue(any("generator/reset 可疑路径" in item for item in violations))
            self.assertTrue(any("在线生成器端点" in item for item in violations))

    def test_archive_rejects_missing_or_non_executable_binary(self) -> None:
        with test_workspace() as directory:
            archive = directory / "filesystem.tar"
            write_archive(
                archive,
                {
                    "usr/local/bin/ryframe-worker": (b"worker", 0o644),
                    "usr/local/bin/unexpected": (b"unexpected", 0o755),
                },
            )

            violations = MODULE.inspect_image_archive(archive)
            self.assertTrue(any("条目必须精确" in item for item in violations))
            self.assertTrue(any("ryframe-worker 必须" in item for item in violations))


if __name__ == "__main__":
    unittest.main()
