"""联合发布的仓库身份与远端写入顺序策略回归。"""

from pathlib import Path
import re
import unittest

import yaml


ROOT = Path(__file__).resolve().parents[2]
FRONTEND_REPOSITORY = (
    "${{ vars.RYFRAME_FRONTEND_REPOSITORY || "
    "format('{0}/ryframe-vue3', github.repository_owner) }}"
)


def workflow(name):
    return yaml.safe_load((ROOT / ".github/workflows" / name).read_text(encoding="utf-8"))


def release_writes(steps):
    # 当前 Release 的写入口：发布 Action，以及显式变更方法的 gh API/Release 命令。
    mutation = re.compile(r"(?:--method|-X)\s+(?:DELETE|POST|PUT|PATCH)\b|gh\s+release\s+(?:create|edit|delete|upload|delete-asset)\b")
    return [(index, step) for index, step in enumerate(steps)
            if str(step.get("uses", "")).startswith("softprops/action-gh-release@")
            or mutation.search(step.get("run", ""))]


class ReleaseWorkflowTests(unittest.TestCase):
    def test_release_tag_runs_every_required_workflow_with_artifact_read_access(self):
        for filename in ("ci.yml", "extended-ci.yml"):
            document = workflow(filename)
            triggers = document.get("on", document.get(True))
            with self.subTest(workflow=filename):
                self.assertEqual(triggers["push"]["tags"], ["v*.*.*"])
        release = workflow("release.yml")
        self.assertEqual(release["permissions"]["actions"], "read")
        self.assertEqual(
            release["jobs"]["publish-release"]["permissions"]["actions"],
            "read",
        )

    def test_all_workflows_bind_the_same_configurable_frontend_repository(self):
        for filename in ("ci.yml", "extended-ci.yml", "release.yml"):
            document = workflow(filename)
            with self.subTest(workflow=filename):
                self.assertEqual(document["env"]["FRONTEND_REPOSITORY"], FRONTEND_REPOSITORY)
                checkouts = []
                for job_name, job in document["jobs"].items():
                    self.assertNotIn("FRONTEND_REPOSITORY", job.get("env", {}))
                    for step in job.get("steps", []):
                        self.assertNotIn("FRONTEND_REPOSITORY", step.get("env", {}))
                        if (str(step.get("uses", "")).startswith("actions/checkout@")
                                and step.get("with", {}).get("path") in {"frontend", "ryframe-vue3"}):
                            checkouts.append(step)
                            expected = ("${{ needs.validate-release.outputs.frontend_repository }}"
                                        if job_name == "publish-release" else "${{ env.FRONTEND_REPOSITORY }}")
                            self.assertEqual(step["with"]["repository"], expected)
                self.assertEqual(len(checkouts), {"ci.yml": 4, "extended-ci.yml": 1, "release.yml": 2}[filename])
        release = workflow("release.yml")["jobs"]["validate-release"]
        self.assertEqual(release["outputs"]["frontend_repository"], "${{ steps.release.outputs.frontend_repository }}")
        export = next(step for step in release["steps"] if step.get("id") == "release")
        self.assertIn('echo "frontend_repository=$FRONTEND_REPOSITORY"', export["run"])

    def test_consumer_contract_uses_the_current_backend_repository(self):
        steps = [step for job in workflow("ci.yml")["jobs"].values() for step in job["steps"]]
        bindings = [step["env"]["RYFRAME_CI_BACKEND_REPOSITORY"] for step in steps
                    if "RYFRAME_CI_BACKEND_REPOSITORY" in step.get("env", {})]
        self.assertEqual(bindings, ["${{ github.repository }}"])

    def test_release_validation_uses_the_unified_xtask_entry(self):
        steps = workflow("release.yml")["jobs"]["validate-release"]["steps"]
        by_name = {step["name"]: step for step in steps}
        command = by_name["Validate release inputs"]["run"]
        self.assertIn("cargo xtask check release source", command)
        self.assertIn('--frontend-dir "$GITHUB_WORKSPACE/frontend"', command)
        self.assertNotIn("python tools/python/validate_release.py", command)
        self.assertEqual(
            by_name["固定发布核验 Python 3.12"]["with"]["python-version"],
            "3.12",
        )
        self.assertTrue(
            by_name["安装发布核验 Rust 工具链"]["uses"].startswith(
                "dtolnay/rust-toolchain@"
            )
        )
        dependency = by_name["安装固定并校验哈希的发布核验 Python 依赖"]["run"]
        self.assertIn("--require-hashes", dependency)
        self.assertIn("scripts/requirements-ci.txt", dependency)

    def test_release_ci_and_source_pair_use_typed_xtask_entries(self):
        workflows = {
            name: workflow(name) for name in ("release.yml", "extended-ci.yml")
        }
        all_runs = "\n".join(
            str(step.get("run", ""))
            for document in workflows.values()
            for job in document["jobs"].values()
            for step in job.get("steps", [])
        )
        self.assertNotRegex(
            all_runs,
            r"python\s+(?:backend/)?scripts/(?:validate_release|verify_release_ci)\.py",
        )
        self.assertEqual(all_runs.count("cargo xtask check release source"), 1)
        self.assertEqual(all_runs.count("cargo xtask check release ci"), 4)

        extended = {
            step["name"]: step
            for step in workflows["extended-ci.yml"]["jobs"]["full-stack-e2e"]["steps"]
        }
        record = extended["记录本次全栈源码组合"]
        verify = extended["复核全栈源码组合未变化"]
        self.assertEqual(record["working-directory"], "backend")
        self.assertEqual(verify["working-directory"], "backend")
        self.assertIn("cargo xtask check release ci record-pair", record["run"])
        self.assertIn("--output", record["run"])
        self.assertIn("cargo xtask check release ci verify-pair", verify["run"])
        self.assertIn("--input", verify["run"])
        for step in (record, verify):
            self.assertIn('--frontend-dir "$GITHUB_WORKSPACE/frontend"', step["run"])

    def test_extended_frontend_build_does_not_call_removed_check_script(self):
        steps = {
            step["name"]: step
            for step in workflow("extended-ci.yml")["jobs"]["full-stack-e2e"]["steps"]
        }
        mysql_container = "${{ job.services.mysql.id }}"
        production = steps["构建前端生产产物"]
        self.assertIn("corepack pnpm build", production["run"])
        self.assertNotIn("corepack pnpm check", production["run"])
        self.assertEqual(
            production["env"]["RYFRAME_CI_MYSQL_CONTAINER_ID"],
            mysql_container,
        )

    def test_extended_ci_tracks_python_and_node_restore_changes(self):
        document = workflow("extended-ci.yml")
        triggers = document.get("on", document.get(True))
        paths = set(triggers["push"]["paths"])
        self.assertIn("tools/**", paths)
        self.assertIn("scripts/requirements-ci.txt", paths)
        for product_path in ("Cargo.lock", "Cargo.toml", "crates/**", "config/**",
                             "migrations/**", "openapi/**", "vendor/**", "xtask/**"):
            self.assertIn(product_path, paths)

    def test_every_release_write_requires_final_ci_and_remote_tag_confirmation(self):
        document = workflow("release.yml")
        self.assertEqual(document["permissions"]["contents"], "read")
        publish = document["jobs"]["publish-release"]
        self.assertEqual(publish["needs"], "validate-release")
        self.assertNotIn("if", publish)
        steps = publish["steps"]
        names = [step["name"] for step in steps]
        ci = names.index("发布前复核最新 CI attempt")
        writes = release_writes(steps)
        self.assertEqual({step["name"] for _, step in writes}, {
            "Purge custom assets from target release", "Create source-only GitHub release",
        })
        self.assertEqual(ci + 1, writes[0][0])
        self.assertIn("cargo xtask check release ci", steps[ci]["run"])
        self.assertNotIn("tools/python/verify_release_ci.py", steps[ci]["run"])
        self.assertNotIn("Confirm tag refs immediately before publishing", names)
        self.assertNotIn("ls-remote", "\n".join(str(step.get("run", "")) for step in steps))
        self.assertEqual(
            steps[ci]["env"]["BACKEND_TAG_OID"],
            "${{ needs.validate-release.outputs.backend_tag_oid }}",
        )
        self.assertEqual(
            steps[ci]["env"]["FRONTEND_TAG_OID"],
            "${{ needs.validate-release.outputs.frontend_tag_oid }}",
        )
        for index, step in writes:
            with self.subTest(step=step["name"]):
                self.assertLess(ci, index)
                self.assertNotIn("if", step)
                self.assertFalse(step.get("continue-on-error", False))
        self.assertNotIn("if", steps[ci])
        self.assertFalse(steps[ci].get("continue-on-error", False))
        initial = next(
            step
            for step in document["jobs"]["validate-release"]["steps"]
            if step["name"] == "验证精确源码的日常与扩展 CI"
        )
        common_flags = {
            "--backend-repository",
            "--frontend-repository",
            "--backend-sha",
            "--frontend-sha",
            "--tag",
            "--timeout",
            "--output",
        }
        expected = (
            (initial, common_flags),
            (
                steps[ci],
                common_flags | {"--backend-tag-oid", "--frontend-tag-oid"},
            ),
        )
        for gate, expected_flags in expected:
            self.assertEqual(
                set(re.findall(r"--[a-z-]+", gate["run"])), expected_flags
            )
            self.assertNotIn("||", gate["run"])
            self.assertNotIn("if", gate)
            self.assertFalse(gate.get("continue-on-error", False))
        final_evidence = steps[names.index("保存最终发布证据")]
        self.assertEqual(final_evidence["if"], "${{ always() }}")
        self.assertGreater(names.index("保存最终发布证据"), writes[-1][0])
        for name, job in document["jobs"].items():
            if name != "publish-release":
                self.assertEqual(release_writes(job["steps"]), [])

    def test_source_only_release_format_remains_enforced(self):
        steps = workflow("release.yml")["jobs"]["publish-release"]["steps"]
        create = next(step for step in steps if step["name"] == "Create source-only GitHub release")
        self.assertNotIn("files", create["with"])
        self.assertEqual(create["with"]["tag_name"], "${{ needs.validate-release.outputs.tag }}")
        verify = next(step for step in steps if step["name"] == "Verify published notes and zero custom assets")
        self.assertIn("(.assets | length == 0)", verify["run"])


if __name__ == "__main__":
    unittest.main()
