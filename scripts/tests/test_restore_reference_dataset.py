import subprocess
import unittest
from pathlib import Path


class ReferenceDatasetTests(unittest.TestCase):
    def test_offline_dataset_driver_contracts(self):
        root = Path(__file__).resolve().parents[2]
        result = subprocess.run(["node", "--test", "scripts/tests/restore_reference_dataset.test.mjs",
                                 "scripts/tests/restore_reference_existing.test.mjs"],
                                cwd=root, text=True, encoding="utf-8", capture_output=True,
                                timeout=60, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
