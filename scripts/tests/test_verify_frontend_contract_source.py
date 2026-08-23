from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import shutil
import stat
import subprocess
import tempfile
import unittest
import uuid
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator


SCRIPT = Path(__file__).resolve().parents[1] / "verify_frontend_contract_source.py"
SPEC = importlib.util.spec_from_file_location("verify_frontend_contract_source", SCRIPT)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


REPOSITORY = "Edgar-ycy/ryframe"
OPENAPI = b'{"openapi":"3.1.0","info":{"title":"RyFrame API"}}\n'


class FrontendContractSourceTests(unittest.TestCase):
    @contextmanager
    def repository(self) -> Iterator[tuple[Path, Path, str, str]]:
        root = Path(tempfile.gettempdir()) / f"ryframe-contract-source-{uuid.uuid4().hex}"
        backend = root / "backend"
        frontend = root / "frontend"
        try:
            backend.mkdir(parents=True)
            frontend.mkdir()
            self.git(backend, "init", "--initial-branch=main")
            (backend / "openapi").mkdir()
            (backend / "openapi/openapi.json").write_bytes(OPENAPI)
            self.git(backend, "add", "openapi/openapi.json")
            self.commit(backend, "contract")
            source = self.git(backend, "rev-parse", "HEAD")
            (backend / "README.md").write_text("head\n", encoding="utf-8")
            self.git(backend, "add", "README.md")
            self.commit(backend, "head")
            head = self.git(backend, "rev-parse", "HEAD")
            self.write_frontend(frontend, source, OPENAPI)
            yield backend, frontend, source, head
        finally:
            if root.exists():
                def remove_readonly(function, path: str, _error) -> None:
                    os.chmod(path, stat.S_IWRITE)
                    function(path)

                shutil.rmtree(root, onexc=remove_readonly)

    def git(self, repository: Path, *arguments: str) -> str:
        completed = subprocess.run(
            ["git", "-C", str(repository), *arguments],
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
        )
        return completed.stdout.strip()

    def commit(self, repository: Path, message: str) -> None:
        self.git(
            repository,
            "-c",
            "user.name=RyFrame CI",
            "-c",
            "user.email=ci@example.invalid",
            "commit",
            "--quiet",
            "-m",
            message,
        )

    def write_frontend(
        self,
        frontend: Path,
        source: str,
        openapi: bytes,
        **overrides: object,
    ) -> None:
        (frontend / "openapi").mkdir(exist_ok=True)
        (frontend / "openapi/openapi.json").write_bytes(openapi)
        metadata: dict[str, object] = {
            "schema_version": 1,
            "backend_repository": REPOSITORY,
            "backend_commit": source,
            "openapi_path": "openapi/openapi.json",
            "openapi_version": "3.1.0",
            "sha256": hashlib.sha256(openapi).hexdigest(),
        }
        metadata.update(overrides)
        (frontend / "openapi/source.json").write_text(
            json.dumps(metadata, indent=2) + "\n",
            encoding="utf-8",
        )

    def verify(
        self,
        backend: Path,
        frontend: Path,
        head: str,
        candidate: bytes = OPENAPI,
    ) -> str:
        candidate_path = frontend / "candidate.json"
        candidate_path.write_bytes(candidate)
        return MODULE.verify_contract_source(
            backend_worktree=backend,
            backend_head=head,
            expected_repository=REPOSITORY,
            source_metadata=frontend / "openapi/source.json",
            frontend_openapi=frontend / "openapi/openapi.json",
            candidate_openapi=candidate_path,
        )

    def test_accepts_head_or_ancestor_with_identical_openapi(self) -> None:
        with self.repository() as (backend, frontend, source, head):
            self.assertEqual(self.verify(backend, frontend, head), source)
            self.write_frontend(frontend, head, OPENAPI)
            self.assertEqual(self.verify(backend, frontend, head), head)

    def test_rejects_source_or_candidate_byte_drift(self) -> None:
        with self.repository() as (backend, frontend, source, head):
            drift = b'{"openapi":"3.1.0","info":{"title":"Drift"}}\n'
            with self.assertRaisesRegex(MODULE.ContractSourceError, "候选 OpenAPI"):
                self.verify(backend, frontend, head, drift)

            (frontend / "openapi/openapi.json").write_bytes(drift)
            self.write_frontend(frontend, source, drift)
            with self.assertRaisesRegex(MODULE.ContractSourceError, "前端正式 OpenAPI"):
                self.verify(backend, frontend, head, drift)

    def test_rejects_non_ancestor_and_missing_source(self) -> None:
        with self.repository() as (backend, frontend, source, head):
            tree = self.git(backend, "rev-parse", f"{source}^{{tree}}")
            side = self.git(
                backend,
                "-c",
                "user.name=RyFrame CI",
                "-c",
                "user.email=ci@example.invalid",
                "commit-tree",
                tree,
                "-m",
                "side",
            )
            self.write_frontend(frontend, side, OPENAPI)
            with self.assertRaisesRegex(MODULE.ContractSourceError, "不是当前后端 HEAD 的祖先"):
                self.verify(backend, frontend, head)

            self.write_frontend(frontend, "f" * 40, OPENAPI)
            with self.assertRaisesRegex(MODULE.ContractSourceError, "Git 命令失败"):
                self.verify(backend, frontend, head)

    def test_rejects_wrong_checkout_head(self) -> None:
        with self.repository() as (backend, frontend, source, _head):
            with self.assertRaisesRegex(MODULE.ContractSourceError, "工作树 HEAD"):
                self.verify(backend, frontend, source)

    def test_rejects_repository_path_hash_version_and_extra_fields(self) -> None:
        cases = (
            ({"backend_repository": "other/repository"}, "后端仓库不匹配"),
            ({"backend_repository": "https://example.invalid/repository"}, "owner/repository"),
            ({"backend_commit": "A" * 40}, "小写 40 位"),
            ({"openapi_path": "../openapi.json"}, "路径穿越"),
            ({"openapi_path": "openapi\\openapi.json"}, "路径穿越"),
            ({"openapi_path": "other/openapi.json"}, "必须是 openapi/openapi.json"),
            ({"sha256": "0" * 64}, "摘要不匹配"),
            ({"sha256": "A" * 64}, "小写 64 位"),
            ({"openapi_version": "3.0.0"}, "版本与来源元数据不一致"),
            ({"schema_version": 2}, "schema_version 必须为 1"),
            ({"unexpected": True}, "字段不匹配"),
        )
        for overrides, message in cases:
            with self.subTest(overrides=overrides), self.repository() as (
                backend,
                frontend,
                source,
                head,
            ):
                self.write_frontend(frontend, source, OPENAPI, **overrides)
                with self.assertRaisesRegex(MODULE.ContractSourceError, message):
                    self.verify(backend, frontend, head)


if __name__ == "__main__":
    unittest.main()
