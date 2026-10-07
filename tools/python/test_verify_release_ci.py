import sys
import hashlib
import io
import json
import subprocess
import unittest
import zipfile
from pathlib import Path, PurePosixPath
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
from release_evidence import BUSINESS_OUTPUTS, EvidenceError, Requirement
from verify_release_ci import (
    ARTIFACT_ARCHIVE_MAX_BYTES,
    api,
    archive_evidence_entries,
    collect,
    coordinated_evidence,
    download_archive,
    pages,
    pair_receipts,
    requirements,
    validate_remote_tags,
)
from workspace_directory import WorkspaceDirectory
import verify_release_ci as verifier


TEMP = Path(__file__).resolve().parents[2] / ".local-tests/python-unit"


def protocol_environment(mode, **values):
    return {
        "RYFRAME_RELEASE_PROTOCOL_VERSION": "1",
        "RYFRAME_RELEASE_MODE": mode,
        **{f"RYFRAME_RELEASE_{key}": str(value) for key, value in values.items()},
    }


class ReleaseCollectorTests(unittest.TestCase):
    def test_every_release_workflow_is_bound_to_the_coordinated_tag(self):
        class Args:
            backend_repository = "owner/backend"
            frontend_repository = "owner/frontend"
            backend_sha = "a" * 40
            frontend_sha = "b" * 40
            tag = "v0.13.0"

        required = requirements(Args())
        self.assertEqual(len(required), 2)
        self.assertEqual({item.tag for item in required}, {Args.tag})

    def test_api_uses_remaining_total_deadline(self):
        with patch("verify_release_ci.API_DEADLINE", 110), \
                patch("verify_release_ci.time.monotonic", return_value=105), \
                patch("verify_release_ci.subprocess.run") as run:
            run.return_value.stdout = b'{"ok":true}'
            self.assertEqual(api("repos/owner/backend"), {"ok": True})
            self.assertEqual(run.call_args.kwargs["timeout"], 5)

    def test_expired_deadline_does_not_start_another_request(self):
        with patch("verify_release_ci.API_DEADLINE", 100), \
                patch("verify_release_ci.time.monotonic", return_value=101), \
                patch("verify_release_ci.subprocess.run") as run:
            with self.assertRaisesRegex(EvidenceError, "总截止时间"):
                api("repos/owner/backend")
            run.assert_not_called()

    def setUp(self):
        self.requirement = Requirement("owner/backend", "ci.yml", "a" * 40, ("Required",))
        self.run = {"id": 123, "run_number": 5, "run_attempt": 2, "head_sha": "a" * 40,
                    "status": "completed", "conclusion": "success", "event": "push", "head_branch": "main"}
        self.job = {"id": 42, "name": "Required", "run_id": 123,
                    "head_sha": "a" * 40, "status": "completed", "conclusion": "success"}

    def responses(self, current=None, newest=None):
        return [{"workflow_runs": [self.run]}, self.run, {"jobs": [self.job]},
                current or self.run, {"workflow_runs": [newest or self.run]}]

    def test_attempt_specific_request_accepts_real_job_shape(self):
        with patch("verify_release_ci.api", side_effect=self.responses()) as api:
            result = collect(self.requirement)
            self.assertEqual(result["attempt"], 2)
            self.assertIn("/runs/123/attempts/2/jobs?", api.call_args_list[2].args[0])

    def test_attempt_change_and_new_run_during_collection_restart_wait(self):
        for current, newest in (
            ({**self.run, "run_attempt": 3, "status": "queued"}, None),
            (None, {**self.run, "id": 124, "run_number": 6, "status": "queued"}),
        ):
            with patch("verify_release_ci.api", side_effect=self.responses(current, newest)):
                self.assertIsNone(collect(self.requirement))

    def test_wrong_run_id_fails_before_loading_jobs(self):
        with patch("verify_release_ci.api", side_effect=[{"workflow_runs": [self.run]}, {**self.run, "id": 999}]):
            with self.assertRaises(EvidenceError):
                collect(self.requirement)

    def test_pages_do_not_truncate_jobs(self):
        with patch("verify_release_ci.api", side_effect=[{"jobs": [self.job] * 100}, {"jobs": [self.job]}]):
            self.assertEqual(len(pages("repos/owner/backend/actions/runs/123/jobs", "jobs")), 101)

    def pairs(self, evidence, *, frontend_sha="a" * 40):
        empty = hashlib.sha256(b"").hexdigest()
        fixture_hash = hashlib.sha256(b"business fixture").hexdigest()
        sources = {
            "backend": {"head": "a" * 40, "patch_sha256": empty, "files": []},
            "frontend": {"head": frontend_sha, "patch_sha256": empty, "files": []},
        }
        generated = {}
        for name in ("backend", "frontend"):
            if name == "frontend":
                generated[name] = sources[name]
                continue
            generated[name] = {
                "head": sources[name]["head"],
                "patch_sha256": hashlib.sha256(f"generated-{name}".encode()).hexdigest(),
                "files": [
                    {
                        "path": path,
                        "sha256": fixture_hash
                        if path == "crates/order-business/src/resources/mod.rs"
                        else hashlib.sha256(path.encode()).hexdigest(),
                    }
                    for path in BUSINESS_OUTPUTS[name]
                ],
            }
        fixture = {
            "format_version": 1,
            "fixture": "business",
            "status": "ready",
            "fixture_sha256": fixture_hash,
            "sources": sources,
            "paths": {
                "backend": "/home/runner/work/backend/.local-tests/business-fixture/backend",
                "frontend": "/home/runner/work/backend/.local-tests/business-fixture/frontend",
            },
            "generated": generated,
        }
        pair = {
            "format_version": 1,
            **evidence,
            "backend_sha": "a" * 40,
            "frontend_sha": frontend_sha,
            "sources": sources,
        }
        return {"core": pair, "business": {**pair, "fixture": fixture}}

    @staticmethod
    def raw(value):
        return (json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n").encode()

    @staticmethod
    def binding(path, raw):
        return {
            "path": path,
            "bytes": len(raw),
            "sha256": hashlib.sha256(raw).hexdigest(),
        }

    def full_stack_archive(self, receipt, fixture):
        pair = {
            name: receipt[name]
            for name in (
                "format_version",
                "backend_sha",
                "frontend_sha",
                "run_id",
                "attempt",
                "sources",
            )
        }
        pair_raw = self.raw(pair)
        receipt_root = "/home/runner/work/_temp/ryframe-full-stack"
        embedded_pair = {
            **pair,
            "receipt": self.binding(f"{receipt_root}/source-pair.json", pair_raw),
        }
        if fixture == "core":
            backend_root = "/home/runner/work/backend"
            source = {
                "format_version": 1,
                "fixture": "core",
                "backend_root": backend_root,
                "source_pair": embedded_pair,
                "original": {"backend": pair["sources"]["backend"]},
            }
            fixture_raw = None
        else:
            fixture_receipt = receipt["fixture"]
            fixture_raw = self.raw(fixture_receipt)
            backend_root = fixture_receipt["paths"]["backend"]
            fixture_root = str(PurePosixPath(backend_root).parent)
            definition = {
                "bytes": len(b"business fixture"),
                "sha256": fixture_receipt["fixture_sha256"],
            }
            source = {
                "format_version": 1,
                "fixture": "business",
                "fixture_receipt": self.binding(f"{fixture_root}/fixture.json", fixture_raw),
                "fixture_definition": {
                    "model": {
                        "path": f"{backend_root}/tools/python/fixtures/order-business/src/resources/mod.rs",
                        **definition,
                    },
                },
                "roots": fixture_receipt["paths"],
                "source_pair": embedded_pair,
                "original": fixture_receipt["sources"],
                "generated": fixture_receipt["generated"],
            }
        artifacts = {
            role: {
                "path": f"/home/runner/work/target/ci/full-stack/{role}",
                "bytes": len(role),
                "sha256": hashlib.sha256(role.encode()).hexdigest(),
            }
            for role in ("ryframe", "ryframe-worker", "ryframe-reset", "ryframe-migrate")
        }
        build = {
            "format_version": 1,
            "kind": "full-stack-build",
            "backend_root": backend_root,
            "source": source,
            "artifacts": artifacts,
        }
        build_raw = self.raw(build)
        runtime = {
            "format_version": 1,
            "backend_root": backend_root,
            "scope_id": f"ci-{pair['run_id']}-{pair['attempt']}",
            "configuration_sha256": "d" * 64,
            "worker_ready_url": "http://127.0.0.1:9091/readyz",
            "artifacts": {
                name: {
                    "path": artifacts[role]["path"],
                    "sha256": artifacts[role]["sha256"],
                }
                for name, role in (("api", "ryframe"), ("worker", "ryframe-worker"))
            },
        }
        runtime_raw = self.raw(runtime)
        runtime_evidence = {
            "format_version": 1,
            "kind": "full-stack-runtime",
            "source": source,
            "build_evidence": self.binding(f"{receipt_root}/build-evidence.json", build_raw),
            "runtime": self.binding(f"{receipt_root}/runtime.json", runtime_raw),
        }
        evidence = {
            "source-pair.json": pair_raw,
            "build-evidence.json": build_raw,
            "runtime.json": runtime_raw,
            "runtime-evidence.json": self.raw(runtime_evidence),
        }
        if fixture_raw is not None:
            evidence["fixture.json"] = fixture_raw
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            for name, raw in evidence.items():
                archive.writestr(f"logs/{name}", raw)
        return buffer.getvalue()

    def test_recheck_after_artifact_rejects_new_failure(self):
        required = [self.requirement] * 2
        evidence = {"run_id": 123, "attempt": 2}
        receipt = self.pairs(evidence)
        with patch("verify_release_ci.collect", return_value=evidence):
            def failed(_):
                if len(seen) < 2:
                    seen.append(evidence)
                    return evidence
                raise EvidenceError("最新重跑失败")
            seen = []
            with self.assertRaisesRegex(EvidenceError, "最新重跑失败"):
                coordinated_evidence(
                    required,
                    5,
                    lambda *_: receipt,
                    failed,
                    lambda _: None,
                    lambda: 0,
                    fixture_sha256=receipt["business"]["fixture"]["fixture_sha256"],
                    frontend_sha="a" * 40,
                )

    def test_recheck_is_bounded_and_success_records_pair(self):
        required = [self.requirement] * 2
        evidence = {"run_id": 123, "attempt": 2}
        receipt = self.pairs(evidence)
        fixture_sha256 = receipt["business"]["fixture"]["fixture_sha256"]
        result = coordinated_evidence(
            required,
            1,
            lambda *_: receipt,
            lambda _: evidence,
            fixture_sha256=fixture_sha256,
            frontend_sha="a" * 40,
        )
        self.assertEqual(result, {"runs": [evidence] * 2, "source_pairs": receipt})
        responses = iter([evidence] * 2 + [None] * 2)
        times = iter([0, 0, 0, 2])
        with self.assertRaisesRegex(EvidenceError, "复核期间"):
            coordinated_evidence(
                required,
                1,
                lambda *_: receipt,
                lambda _: next(responses),
                lambda _: None,
                lambda: next(times),
                fixture_sha256=fixture_sha256,
                frontend_sha="a" * 40,
            )

        with self.assertRaisesRegex(EvidenceError, "业务 crate fixture 摘要"):
            coordinated_evidence(required, 1)

    def test_full_stack_receipts_are_read_from_backend_extended_ci(self):
        backend_sha = "a" * 40
        required = [
            Requirement("owner/backend", "ci.yml", backend_sha, ("Required",), "v0.13.1"),
            Requirement("owner/backend", "extended-ci.yml", backend_sha, ("Real API",), "v0.13.1"),
        ]
        ci_run = {"run_id": 101, "attempt": 1, "workflow": "ci"}
        extended_run = {"run_id": 202, "attempt": 1, "workflow": "extended-ci"}
        frontend_sha = "b" * 40
        receipt = self.pairs(extended_run, frontend_sha=frontend_sha)
        collected = iter((ci_run, extended_run, ci_run, extended_run))
        observed = []

        def read_pairs(run, *_args):
            observed.append(run)
            return receipt

        result = coordinated_evidence(
            required,
            1,
            read_pairs,
            lambda _requirement: next(collected),
            fixture_sha256=receipt["business"]["fixture"]["fixture_sha256"],
            frontend_sha=frontend_sha,
        )

        self.assertEqual(observed, [extended_run])
        self.assertEqual(result["runs"], [ci_run, extended_run])

    def test_remote_tags_bind_annotated_objects_and_dereferenced_commits(self):
        required = [
            Requirement("owner/backend", "ci.yml", "a" * 40, (), "v0.13.0"),
            Requirement("owner/frontend", "ci.yml", "b" * 40, (), "v0.13.0"),
        ]
        tag_oids = ("c" * 40, "d" * 40)

        def reference(repository, oid):
            return {
                "ref": "refs/tags/v0.13.0",
                "object": {"type": "tag", "sha": oid},
                "repository": repository,
            }

        def tag(oid, commit):
            return {"sha": oid, "object": {"type": "commit", "sha": commit}}

        responses = [
            reference("backend", tag_oids[0]),
            reference("frontend", tag_oids[1]),
            tag(tag_oids[0], required[0].sha),
            tag(tag_oids[1], required[1].sha),
            reference("backend", tag_oids[0]),
            reference("frontend", tag_oids[1]),
        ]
        with patch("verify_release_ci.api", side_effect=responses):
            evidence = validate_remote_tags(required, tag_oids)
        self.assertEqual(
            [(item["tag_oid"], item["commit"]) for item in evidence],
            list(zip(tag_oids, (required[0].sha, required[1].sha), strict=True)),
        )

        cases = {
            "wrong_oid": [reference("backend", "e" * 40)],
            "wrong_commit": [
                reference("backend", tag_oids[0]),
                reference("frontend", tag_oids[1]),
                tag(tag_oids[0], "e" * 40),
            ],
            "moved_during_check": [
                reference("backend", tag_oids[0]),
                reference("frontend", tag_oids[1]),
                tag(tag_oids[0], required[0].sha),
                tag(tag_oids[1], required[1].sha),
                reference("backend", "e" * 40),
            ],
        }
        for name, response in cases.items():
            with self.subTest(name=name), patch(
                "verify_release_ci.api", side_effect=response
            ), self.assertRaises(EvidenceError):
                validate_remote_tags(required, tag_oids)

    def test_remote_tags_support_nested_annotations_and_reject_unsafe_shapes(self):
        required = [
            Requirement("owner/backend", "ci.yml", "a" * 40, (), "v0.13.0"),
            Requirement("owner/frontend", "ci.yml", "b" * 40, (), "v0.13.0"),
        ]
        tag_oids = ("c" * 40, "d" * 40)
        nested_oid = "e" * 40

        def reference(oid, kind="tag"):
            return {
                "ref": "refs/tags/v0.13.0",
                "object": {"type": kind, "sha": oid},
            }

        def tag(oid, kind, target):
            return {"sha": oid, "object": {"type": kind, "sha": target}}

        nested = [
            reference(tag_oids[0]),
            reference(tag_oids[1]),
            tag(tag_oids[0], "tag", nested_oid),
            tag(nested_oid, "commit", required[0].sha),
            tag(tag_oids[1], "commit", required[1].sha),
            reference(tag_oids[0]),
            reference(tag_oids[1]),
        ]
        with patch("verify_release_ci.api", side_effect=nested):
            evidence = validate_remote_tags(required, tag_oids)
        self.assertEqual([item["commit"] for item in evidence], ["a" * 40, "b" * 40])

        cases = {
            "lightweight_tag": [reference(tag_oids[0], "commit")],
            "cycle": [
                reference(tag_oids[0]),
                reference(tag_oids[1]),
                tag(tag_oids[0], "tag", nested_oid),
                tag(nested_oid, "tag", tag_oids[0]),
            ],
            "frontend_moved_after_dereference": [
                reference(tag_oids[0]),
                reference(tag_oids[1]),
                tag(tag_oids[0], "commit", required[0].sha),
                tag(tag_oids[1], "commit", required[1].sha),
                reference(tag_oids[0]),
                reference("f" * 40),
            ],
        }
        for name, responses in cases.items():
            with self.subTest(name=name), patch(
                "verify_release_ci.api", side_effect=responses
            ), self.assertRaises(EvidenceError):
                validate_remote_tags(required, tag_oids)

    def test_remote_tag_dereference_depth_is_bounded(self):
        required = [
            Requirement("owner/backend", "ci.yml", "a" * 40, (), "v0.13.0"),
            Requirement("owner/frontend", "ci.yml", "b" * 40, (), "v0.13.0"),
        ]
        tag_oids = ("c" * 40, "d" * 40)
        chain = [f"{value:040x}" for value in range(1, 9)]
        responses = [
            {
                "ref": "refs/tags/v0.13.0",
                "object": {"type": "tag", "sha": tag_oids[0]},
            },
            {
                "ref": "refs/tags/v0.13.0",
                "object": {"type": "tag", "sha": tag_oids[1]},
            },
        ]
        current = tag_oids[0]
        for target in chain:
            responses.append(
                {"sha": current, "object": {"type": "tag", "sha": target}}
            )
            current = target
        with patch("verify_release_ci.api", side_effect=responses), self.assertRaisesRegex(
            EvidenceError, "解引用层数超限"
        ):
            validate_remote_tags(required, tag_oids)

    def test_final_tag_check_runs_after_latest_attempt_recheck(self):
        required = [self.requirement] * 2
        tag_requirements = [
            self.requirement,
            Requirement("owner/frontend", "ci.yml", "b" * 40, (), "v0.13.5"),
        ]
        evidence = {"run_id": 123, "attempt": 2}
        receipts = self.pairs(evidence)
        fixture_sha256 = receipts["business"]["fixture"]["fixture_sha256"]
        events = []
        observed_tag_repositories = []

        def collect(_requirement):
            events.append("ci")
            return evidence

        def read_pairs(*_arguments):
            events.append("pairs")
            return receipts

        def read_tags(tagged_repositories, _tag_oids):
            events.append("tags")
            observed_tag_repositories.extend(
                requirement.repository for requirement in tagged_repositories
            )
            return [{"status": "current"}]

        result = coordinated_evidence(
            required,
            1,
            read_pairs,
            collect,
            fixture_sha256=fixture_sha256,
            frontend_sha="a" * 40,
            tag_oids=("c" * 40, "d" * 40),
            tag_requirements=tag_requirements,
            read_tags=read_tags,
        )
        self.assertEqual(events, ["ci"] * 2 + ["pairs"] + ["ci"] * 2 + ["tags"])
        self.assertEqual(observed_tag_repositories, ["owner/backend", "owner/frontend"])
        self.assertEqual(result["remote_tags"], [{"status": "current"}])

    def test_pair_artifacts_require_both_current_attempts_and_keep_artifact_identity(self):
        evidence = {"repository": "owner/backend", "run_id": 123, "attempt": 2}
        receipts = self.pairs(evidence)
        artifacts = [
            {"id": 10, "name": "ryframe-full-stack-123-2"},
            {"id": 11, "name": "ryframe-full-stack-123-2-business"},
        ]
        archives = {}
        for artifact, kind in zip(artifacts, ("core", "business"), strict=True):
            archives[artifact["id"]] = self.full_stack_archive(receipts[kind], kind)
            artifact["size_in_bytes"] = len(archives[artifact["id"]])

        def download(artifact, _endpoint, destination):
            destination.write_bytes(archives[artifact["id"]])

        def temporary(**_):
            return WorkspaceDirectory(TEMP, prefix="release-evidence-")

        with patch("verify_release_ci.pages", return_value=artifacts), patch(
            "verify_release_ci.download_archive", side_effect=download
        ), patch("verify_release_ci.tempfile.TemporaryDirectory", side_effect=temporary):
            result = pair_receipts(
                evidence,
                "a" * 40,
                "a" * 40,
                receipts["business"]["fixture"]["fixture_sha256"],
            )
            self.assertEqual(
                result["business"]["artifact"],
                {
                    "id": 11,
                    "name": artifacts[1]["name"],
                    "size_in_bytes": artifacts[1]["size_in_bytes"],
                },
            )
            self.assertEqual(
                set(result["core"]["archive_evidence"]),
                {"build_evidence", "runtime", "runtime_evidence"},
            )
        for invalid in (
            artifacts[:1],
            [artifacts[0], {**artifacts[1], "expired": True}],
            [artifacts[0], {**artifacts[1], "name": "ryframe-full-stack-123-1-business"}],
            [*artifacts, artifacts[1]],
        ):
            with self.subTest(artifacts=invalid), patch(
                "verify_release_ci.pages", return_value=invalid
            ), patch("verify_release_ci.download_archive", side_effect=download), patch(
                "verify_release_ci.tempfile.TemporaryDirectory", side_effect=temporary
            ):
                with self.assertRaises(EvidenceError):
                    pair_receipts(
                        evidence,
                        "a" * 40,
                        "a" * 40,
                        receipts["business"]["fixture"]["fixture_sha256"],
                    )

        oversize = [artifacts[0], {**artifacts[1], "size_in_bytes": ARTIFACT_ARCHIVE_MAX_BYTES + 1}]
        with patch("verify_release_ci.pages", return_value=oversize), patch(
            "verify_release_ci.download_archive"
        ) as download:
            with self.assertRaisesRegex(EvidenceError, "硬上限"):
                pair_receipts(
                    evidence,
                    "a" * 40,
                    "a" * 40,
                    receipts["business"]["fixture"]["fixture_sha256"],
                )
            download.assert_not_called()

    def test_artifact_download_streams_to_disk_with_shared_deadline(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        content = b"controlled archive"
        with WorkspaceDirectory(TEMP, prefix="release-stream-") as temporary:
            destination = Path(temporary) / "evidence.zip"

            def run(_arguments, **options):
                options["stdout"].write(content)
                return subprocess.CompletedProcess([], 0, b"", b"")

            with patch("verify_release_ci.API_DEADLINE", 110), patch(
                "verify_release_ci.time.monotonic", return_value=105
            ), patch("verify_release_ci.subprocess.run", side_effect=run) as process:
                download_archive(
                    {"size_in_bytes": len(content)},
                    "repos/owner/backend/actions/artifacts/10/zip",
                    destination,
                )
            self.assertEqual(destination.read_bytes(), content)
            self.assertEqual(process.call_args.kwargs["timeout"], 5)
            self.assertNotIn("capture_output", process.call_args.kwargs)
            self.assertIsNotNone(process.call_args.kwargs["stdout"])

    def test_artifact_metadata_limit_fails_before_download(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        with WorkspaceDirectory(TEMP, prefix="release-limit-") as temporary, patch(
            "verify_release_ci.subprocess.run"
        ) as process:
            for size in (None, 0, True, ARTIFACT_ARCHIVE_MAX_BYTES + 1):
                destination = Path(temporary) / f"{size}.zip"
                with self.subTest(size=size), self.assertRaisesRegex(
                    EvidenceError, "硬上限"
                ):
                    download_archive(
                        {"size_in_bytes": size},
                        "repos/owner/backend/actions/artifacts/10/zip",
                        destination,
                    )
            process.assert_not_called()

    def test_artifact_actual_size_must_match_metadata(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        with WorkspaceDirectory(TEMP, prefix="release-size-") as temporary:
            destination = Path(temporary) / "evidence.zip"

            def run(_arguments, **options):
                options["stdout"].write(b"short")
                return subprocess.CompletedProcess([], 0, b"", b"")

            with patch("verify_release_ci.subprocess.run", side_effect=run), self.assertRaisesRegex(
                EvidenceError, "实际大小"
            ):
                download_archive(
                    {"size_in_bytes": 99},
                    "repos/owner/backend/actions/artifacts/10/zip",
                    destination,
                )

    def test_collection_failure_keeps_a_failed_receipt(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        with WorkspaceDirectory(TEMP, prefix="release-failure-") as temporary:
            output = Path(temporary) / "failure.json"
            environment = protocol_environment(
                "ci-evidence",
                BACKEND_REPOSITORY="owner/backend",
                FRONTEND_REPOSITORY="owner/frontend",
                BACKEND_SHA="a" * 40,
                FRONTEND_SHA="b" * 40,
                BACKEND_TAG_OID="c" * 40,
                FRONTEND_TAG_OID="d" * 40,
                TAG="v0.13.0",
                TIMEOUT_SECONDS="5",
                OUTPUT_PATH=output,
            )
            try:
                with patch("sys.argv", ["verify_release_ci.py"]), patch.dict(
                    "os.environ", environment, clear=True
                ), patch(
                    "verify_release_ci.coordinated_evidence",
                    side_effect=OSError("stream failed"),
                ), self.assertRaisesRegex(OSError, "stream failed"):
                    verifier.main()
            finally:
                verifier.API_DEADLINE = None
            receipt = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(receipt["status"], "failed")
            self.assertEqual(receipt["backend_sha"], "a" * 40)
            self.assertEqual(receipt["frontend_sha"], "b" * 40)
            self.assertEqual(receipt["backend_tag_oid"], "c" * 40)
            self.assertEqual(receipt["frontend_tag_oid"], "d" * 40)

    def test_record_pair_captures_both_clean_source_snapshots(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        with WorkspaceDirectory(TEMP, prefix="release-pair-") as temporary:
            root = Path(temporary)
            backend, frontend = root / "backend", root / "frontend"
            backend.mkdir()
            frontend.mkdir()
            output = root / "runtime/source-pair.json"
            receipt = {
                "format_version": 1,
                "backend_sha": "a" * 40,
                "frontend_sha": "b" * 40,
                "run_id": 123,
                "attempt": 2,
                "sources": {},
            }
            environment = protocol_environment(
                "ci-record-pair",
                OUTPUT_PATH=output,
                BACKEND_DIR=backend,
                FRONTEND_DIR=frontend,
            )
            environment.update(
                GITHUB_RUN_ID="123",
                GITHUB_RUN_ATTEMPT="2",
            )
            with patch("sys.argv", ["verify_release_ci.py"]), patch.dict(
                "os.environ", environment, clear=True
            ), patch(
                "verify_release_ci.source_pair_receipt", return_value=receipt
            ) as capture:
                verifier.main()
            capture.assert_called_once_with(backend, frontend, 123, 2)
            self.assertEqual(json.loads(output.read_text(encoding="utf-8")), receipt)
            original = output.read_bytes()
            with patch("sys.argv", ["verify_release_ci.py"]), patch.dict(
                "os.environ", environment, clear=True
            ), patch(
                "verify_release_ci.source_pair_receipt", return_value=receipt
            ), self.assertRaises(FileExistsError):
                verifier.main()
            self.assertEqual(output.read_bytes(), original)

    def test_verify_pair_is_read_only_and_rechecks_both_roots(self):
        receipt_path = Path("pair.json")
        environment = protocol_environment(
            "ci-verify-pair",
            INPUT_PATH=receipt_path,
            BACKEND_DIR="backend",
            FRONTEND_DIR="frontend",
        )
        environment.update(GITHUB_RUN_ID="12", GITHUB_RUN_ATTEMPT="3")
        with patch("sys.argv", ["verify_release_ci.py"]), patch.dict(
            "os.environ", environment, clear=True
        ), patch(
            "verify_release_ci.verify_source_pair_receipt"
        ) as verify:
            verifier.main()
        verify.assert_called_once_with(
            receipt_path,
            Path("backend"),
            Path("frontend"),
            12,
            3,
        )

    def test_private_protocol_rejects_partial_tag_object_identity(self):
        TEMP.mkdir(parents=True, exist_ok=True)
        with WorkspaceDirectory(TEMP, prefix="release-args-") as temporary:
            environment = protocol_environment(
                "ci-evidence",
                BACKEND_REPOSITORY="owner/backend",
                FRONTEND_REPOSITORY="owner/frontend",
                BACKEND_SHA="a" * 40,
                FRONTEND_SHA="b" * 40,
                BACKEND_TAG_OID="c" * 40,
                TAG="v0.13.0",
                TIMEOUT_SECONDS="5",
                OUTPUT_PATH=Path(temporary) / "evidence.json",
            )
            with patch("sys.argv", ["verify_release_ci.py"]), patch.dict(
                "os.environ", environment, clear=True
            ), patch("sys.stderr", io.StringIO()), patch(
                "verify_release_ci.coordinated_evidence"
            ) as collect, self.assertRaises(SystemExit) as error:
                verifier.main()
            self.assertEqual(error.exception.code, 2)
            collect.assert_not_called()

    def test_private_program_rejects_argv_before_collecting_evidence(self):
        environment = protocol_environment(
            "ci-evidence",
            BACKEND_REPOSITORY="owner/backend",
            FRONTEND_REPOSITORY="owner/frontend",
            BACKEND_SHA="a" * 40,
            FRONTEND_SHA="b" * 40,
            TAG="v0.13.0",
            TIMEOUT_SECONDS="5",
            OUTPUT_PATH="D:/evidence.json",
        )
        with patch(
            "sys.argv", ["verify_release_ci.py", "--output", "D:/other.json"]
        ), patch.dict("os.environ", environment, clear=True), patch(
            "sys.stderr", io.StringIO()
        ), patch("verify_release_ci.coordinated_evidence") as collect, self.assertRaises(
            SystemExit
        ) as error:
            verifier.main()
        self.assertEqual(error.exception.code, 2)
        collect.assert_not_called()

    def test_archive_reader_rejects_oversize_receipt_before_validation(self):
        buffer = io.BytesIO()
        with zipfile.ZipFile(buffer, "w") as archive:
            archive.writestr("logs/source-pair.json", b"x" * (16 * 1024 + 1))
        with zipfile.ZipFile(io.BytesIO(buffer.getvalue())) as archive, self.assertRaisesRegex(
            EvidenceError, "大小"
        ):
            archive_evidence_entries(archive)


if __name__ == "__main__":
    unittest.main()
