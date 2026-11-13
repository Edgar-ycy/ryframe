"""完整资源前后像的精确比较；库存、存储与对象读取使用现有边界的离线替身。"""

from contextlib import nullcontext
from pathlib import Path
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import restore_reference_images as images


class RestoreImagesControlTests(unittest.TestCase):
    def test_control_uses_coordinator_bridges_with_the_original_target_storage(self):
        backend = Path(__file__).resolve().parents[2]
        work = backend / ".local-tests/reference-control-fixture"
        bridges = [{"path": str(work / "source-bridge.json"), "sha256": "a" * 64}]
        manifest = {"target_storage_run": {"path": str(work / "storage")}}
        subject = object.__new__(images.RestoreImages)
        subject.backend, subject.plan = backend, {"work_dir": str(work)}
        execution = work / "product-backend"
        target = {"execution_backend": {"path": str(execution)}}
        subject.arm = {"request": {"build_bridges": bridges}, "manifest": manifest, "target": target}
        with (
            patch.object(images, "execution_backend", return_value=(execution, {})) as selected,
            patch.object(images, "artifact_sources", return_value=nullcontext()) as audited,
            patch.object(images, "target_storage_control", return_value=nullcontext()) as controlled,
            subject.control() as active,
        ):
            self.assertIs(active, subject)
            selected.assert_called_once_with(backend, target)
            audited.assert_called_once_with(backend, bridges)
            controlled.assert_called_once_with(backend, work, manifest)


if __name__ == "__main__":
    unittest.main()
