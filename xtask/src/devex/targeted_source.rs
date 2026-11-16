use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Result,
    ci::resource_gate::{ResourceGateMode, analyze, decision_for, load_repository},
    cli::ResourceGateOptions,
};

use super::model::{BaselineContract, PairedArm};

const MANIFEST_LIMIT: u64 = 64 * 1024;
const RESOURCE: &str = "catalog/resources/post.toml";
const OPENAPI: &str = "openapi/openapi.json";
const FRONTEND_SOURCE: &str = "openapi/source.json";
const BEFORE: &str = "Post";
const AFTER: &str = "Job Position";
const BACKEND_PATHS: &[&str] = &[
    "catalog/access.generated.toml",
    "catalog/resources/.ownership.toml",
    RESOURCE,
    "crates/ryframe-api/src/generated/post/openapi.rs",
    OPENAPI,
];
const FRONTEND_PATHS: &[&str] = &[
    OPENAPI,
    FRONTEND_SOURCE,
    "src/api/generated/crudResources.ts",
    "src/generated/resources/post/fields.ts",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetedPair {
    pub(super) b0: TargetedArm,
    pub(super) b1: TargetedArm,
    pub(super) steps: Vec<String>,
    pub(super) manifest_sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetedArm {
    pub(super) backend_base: String,
    pub(super) backend_head: String,
    pub(super) frontend_base: String,
    pub(super) frontend_head: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format_version: u16,
    scenario: String,
    before: String,
    after: String,
    timed_samples: u32,
    arms: Arms,
    passed: bool,
    at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arms {
    b0: ArmReceipt,
    b1: ArmReceipt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ArmReceipt {
    backend: Checkout,
    frontend: Checkout,
    openapi_sha256: String,
    source_receipt_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkout {
    path: PathBuf,
    base: String,
    head: String,
    tree: String,
    paths: Vec<String>,
    patch_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontendSource {
    schema_version: u16,
    backend_repository: String,
    backend_commit: String,
    openapi_path: String,
    openapi_version: String,
    sha256: String,
}

impl TargetedPair {
    pub(super) fn valid_report_shape(&self) -> bool {
        is_hex(&self.manifest_sha256, 64)
            && !self.steps.is_empty()
            && self.steps.iter().all(|step| !step.trim().is_empty())
            && [&self.b0, &self.b1]
                .into_iter()
                .all(TargetedArm::valid_commits)
    }

    pub(super) fn arm(&self, arm: PairedArm) -> &TargetedArm {
        match arm {
            PairedArm::Baseline => &self.b0,
            PairedArm::Candidate => &self.b1,
        }
    }

    pub(super) fn load(
        path: &Path,
        baseline: (&Path, &Path),
        candidate: (&Path, &Path),
    ) -> Result<Self> {
        let bytes = read_manifest(path)?;
        let manifest: Manifest = serde_json::from_slice(&bytes)?;
        manifest.verify()?;
        let Arms { b0, b1 } = manifest.arms;

        let adaptation = BaselineContract::StableReadinessB0CommonToolchainV1
            .registered_frontend_adaptation()
            .ok_or("资源门禁缺少已登记的 B0 前端适配")?;
        if b0.backend.base != BaselineContract::STABLE_READINESS_B0_ADAPTER_COMMIT
            || b0.frontend.base != adaptation.commit
        {
            return Err("资源门禁 B0 父提交与已登记适配不一致".into());
        }
        let (b0, steps) = b0.verify(baseline, "B0")?;
        let (b1, candidate_steps) = b1.verify(candidate, "B1")?;
        if steps != candidate_steps {
            return Err("资源门禁两侧定向步骤不一致".into());
        }
        if read_manifest(path)? != bytes {
            return Err("核验期间资源门禁来源收据已变化".into());
        }
        Ok(Self {
            b0,
            b1,
            steps,
            manifest_sha256: sha256(&bytes),
        })
    }
}

impl Manifest {
    fn verify(&self) -> Result<()> {
        if self.format_version != 1
            || self.scenario != "post resource.labels.en"
            || self.before != BEFORE
            || self.after != AFTER
            || self.timed_samples != 0
            || !self.passed
        {
            return Err("资源门禁来源收据版本、场景或准备结果无效".into());
        }
        chrono::DateTime::parse_from_rfc3339(&self.at).map_err(|_| "资源门禁来源收据时间无效")?;
        Ok(())
    }
}

impl ArmReceipt {
    fn verify(
        self,
        (backend, frontend): (&Path, &Path),
        label: &str,
    ) -> Result<(TargetedArm, Vec<String>)> {
        if !is_hex(&self.openapi_sha256, 64) || !is_hex(&self.source_receipt_sha256, 64) {
            return Err(format!("{label} 资源门禁 OpenAPI 或来源收据摘要无效").into());
        }
        self.backend.verify(backend, BACKEND_PATHS)?;
        self.frontend.verify(frontend, FRONTEND_PATHS)?;
        verify_label(backend, &self.backend)?;
        self.verify_openapi(backend, frontend, false)?;
        self.verify_openapi(backend, frontend, true)?;

        let arm = TargetedArm {
            backend_base: self.backend.base,
            backend_head: self.backend.head,
            frontend_base: self.frontend.base,
            frontend_head: self.frontend.head,
        };
        let steps = arm.targeted_steps(backend, label)?;
        verify_clean_head(backend, &arm.backend_head)?;
        verify_clean_head(frontend, &arm.frontend_head)?;
        Ok((arm, steps))
    }

    fn verify_openapi(&self, backend: &Path, frontend: &Path, head: bool) -> Result<()> {
        let (backend_commit, frontend_commit) = if head {
            (&self.backend.head, &self.frontend.head)
        } else {
            (&self.backend.base, &self.frontend.base)
        };
        let backend_doc = blob(backend, backend_commit, OPENAPI)?;
        let frontend_doc = blob(frontend, frontend_commit, OPENAPI)?;
        let source: FrontendSource =
            serde_json::from_slice(&blob(frontend, frontend_commit, FRONTEND_SOURCE)?)?;
        let hash = sha256(&backend_doc);
        if backend_doc != frontend_doc
            || source.schema_version != 1
            || source.backend_repository != "Edgar-ycy/ryframe"
            || source.openapi_path != OPENAPI
            || source.openapi_version != "3.1.0"
            || source.sha256 != hash
            || !is_hex(&source.backend_commit, 40)
            || (head && (source.backend_commit != *backend_commit || self.openapi_sha256 != hash))
        {
            return Err("资源门禁前后端 OpenAPI 字节或来源不一致".into());
        }
        if !head {
            git(
                backend,
                &[
                    "merge-base",
                    "--is-ancestor",
                    &source.backend_commit,
                    backend_commit,
                ],
            )?;
            if blob(backend, &source.backend_commit, OPENAPI)? != backend_doc {
                return Err("资源门禁原前端来源提交的 OpenAPI 字节不一致".into());
            }
        }
        Ok(())
    }
}

impl TargetedArm {
    fn valid_commits(&self) -> bool {
        self.backend_base != self.backend_head
            && self.frontend_base != self.frontend_head
            && [
                &self.backend_base,
                &self.backend_head,
                &self.frontend_base,
                &self.frontend_head,
            ]
            .into_iter()
            .all(|hash| is_hex(hash, 40))
    }

    pub(super) fn candidate_arguments(&self) -> [&str; 6] {
        [
            "--base-sha",
            &self.backend_base,
            "--head-sha",
            &self.backend_head,
            "--frontend-ref",
            &self.frontend_head,
        ]
    }

    fn targeted_steps(&self, backend: &Path, label: &str) -> Result<Vec<String>> {
        verify_shared_git_environment()?;
        let options = ResourceGateOptions {
            base_sha: Some(self.backend_base.clone()),
            head_sha: Some(self.backend_head.clone()),
            frontend_ref: Some(self.frontend_head.clone()),
            targeted: true,
            ..ResourceGateOptions::default()
        };
        let process_root = super::support::child_path(backend);
        let loaded = load_repository(Path::new(&process_root), &options)
            .map_err(|error| format!("{label} 无法读取资源门禁范围：{error}"))?;
        if loaded.base != self.backend_base || loaded.head != self.backend_head {
            return Err(format!("{label} 资源门禁解析出的 Git 范围不一致").into());
        }
        let change_set = analyze(&loaded.input);
        let decision = decision_for(&change_set, true);
        if !decision.recognized
            || decision.mode != ResourceGateMode::Targeted
            || decision.fallback.is_some()
            || decision.steps.is_empty()
        {
            let reason = decision.fallback.as_deref().unwrap_or("缺少定向步骤");
            return Err(format!("{label} 资源门禁无法使用定向计划：{reason}").into());
        }
        if change_set.source_resources.len() != 1 || !change_set.source_resources.contains("post") {
            return Err(format!("{label} 资源门禁范围须只包含 post 资源事实源").into());
        }
        Ok(decision.steps)
    }
}

impl Checkout {
    fn verify(&self, root: &Path, paths: &[&str]) -> Result<()> {
        if !self.path.is_absolute() || !root.is_absolute() {
            return Err("资源门禁收据和实际 Git 工作树须使用绝对路径".into());
        }
        let canonical_root = root.canonicalize()?;
        let toplevel = git_text(root, &["rev-parse", "--show-toplevel"])?;
        if self.path.canonicalize()? != canonical_root
            || Path::new(toplevel.trim()).canonicalize()? != canonical_root
        {
            return Err("资源门禁收据路径与实际 Git 工作树根不一致".into());
        }
        if !is_hex(&self.base, 40)
            || !is_hex(&self.head, 40)
            || !is_hex(&self.tree, 40)
            || !is_hex(&self.patch_sha256, 64)
            || !self
                .paths
                .iter()
                .map(String::as_str)
                .eq(paths.iter().copied())
        {
            return Err("资源门禁 Git 提交、树、补丁或路径登记无效".into());
        }
        verify_unmodified_git_history(root)?;
        verify_clean_head(root, &self.head)?;
        let parents = git_text(root, &["rev-list", "--parents", "-n", "1", &self.head])?;
        if parents.trim() != format!("{} {}", self.head, self.base)
            || git_text(root, &["rev-parse", "HEAD^{tree}"])?.trim() != self.tree
        {
            return Err("资源门禁测量提交的唯一父提交或树不一致".into());
        }
        self.verify_diff(root, paths)
    }

    fn verify_diff(&self, root: &Path, paths: &[&str]) -> Result<()> {
        let diff = git(
            root,
            &[
                "diff",
                "--name-status",
                "-z",
                "--no-renames",
                &self.base,
                &self.head,
                "--",
            ],
        )?;
        let mut expected = Vec::new();
        for path in paths {
            expected.extend_from_slice(b"M\0");
            expected.extend_from_slice(path.as_bytes());
            expected.push(0);
        }
        if diff != expected {
            return Err("资源门禁测量提交的实际修改路径或状态与登记不一致".into());
        }
        let patch = git(
            root,
            &[
                "diff",
                "--binary",
                "--full-index",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                "--src-prefix=a/",
                "--dst-prefix=b/",
                &self.base,
                &self.head,
                "--",
            ],
        )?;
        if sha256(&patch) != self.patch_sha256 {
            return Err("资源门禁测量提交的完整补丁摘要不一致".into());
        }
        Ok(())
    }
}

fn verify_label(root: &Path, checkout: &Checkout) -> Result<()> {
    let original = String::from_utf8(blob(root, &checkout.base, RESOURCE)?)?;
    let changed = String::from_utf8(blob(root, &checkout.head, RESOURCE)?)?;
    let mut parsed: toml::Value = toml::from_str(&original)?;
    let label = parsed
        .get_mut("resource")
        .and_then(|resource| resource.get_mut("labels"))
        .and_then(|labels| labels.get_mut("en"))
        .ok_or("资源门禁测量源缺少 post resource.labels.en")?;
    if label.as_str() != Some(BEFORE) {
        return Err("资源门禁测量源 post 英文标签前像不一致".into());
    }
    *label = toml::Value::String(AFTER.to_owned());
    let expected_line = format!("\nen = \"{BEFORE}\"\n");
    let changed_line = format!("\nen = \"{AFTER}\"\n");
    if parsed != toml::from_str::<toml::Value>(&changed)?
        || original.replacen(&expected_line, &changed_line, 1) != changed
    {
        return Err("资源门禁测量源须只改变 post 英文标签".into());
    }
    Ok(())
}

fn verify_unmodified_git_history(root: &Path) -> Result<()> {
    let replacements = git_text(
        root,
        &["for-each-ref", "--format=%(refname)", "refs/replace"],
    )?;
    let grafts = git_text(
        root,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/grafts",
        ],
    )?;
    if !replacements.is_empty() || Path::new(grafts.trim()).try_exists()? {
        return Err("资源门禁工作树包含 Git 替换引用或 graft".into());
    }
    Ok(())
}

fn verify_clean_head(root: &Path, expected: &str) -> Result<()> {
    let head = git_text(root, &["rev-parse", "--verify", "HEAD"])?;
    let status = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    let index = git(root, &["ls-files", "-v", "-z"])?;
    if head.trim() != expected
        || !status.is_empty()
        || index.split(|byte| *byte == 0).any(|entry| {
            entry
                .first()
                .is_some_and(|byte| *byte == b'S' || byte.is_ascii_lowercase())
        })
    {
        return Err(format!("资源门禁测量工作树 HEAD 不符或不干净：{}", root.display()).into());
    }
    Ok(())
}

fn read_manifest(path: &Path) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("资源门禁来源收据不能使用重解析点".into());
        }
    }
    if !metadata.is_file() || metadata.len() > MANIFEST_LIMIT {
        return Err("资源门禁来源收据须为限长常规文件".into());
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MANIFEST_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MANIFEST_LIMIT {
        return Err("资源门禁来源收据超过大小上限".into());
    }
    Ok(bytes)
}

fn is_hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn blob(root: &Path, commit: &str, path: &str) -> Result<Vec<u8>> {
    git(root, &["cat-file", "blob", &format!("{commit}:{path}")])
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    Ok(String::from_utf8(git(root, args)?)?)
}

fn verify_shared_git_environment() -> Result<()> {
    for (name, _) in env::vars_os() {
        let name = name.to_string_lossy().to_ascii_uppercase();
        if matches!(
            name.as_str(),
            "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                | "GIT_ATTR_NOSYSTEM"
                | "GIT_ATTR_SOURCE"
                | "GIT_CEILING_DIRECTORIES"
                | "GIT_COMMON_DIR"
                | "GIT_CONFIG"
                | "GIT_DIR"
                | "GIT_DISCOVERY_ACROSS_FILESYSTEM"
                | "GIT_EXEC_PATH"
                | "GIT_EXTERNAL_DIFF"
                | "GIT_GRAFT_FILE"
                | "GIT_GLOB_PATHSPECS"
                | "GIT_ICASE_PATHSPECS"
                | "GIT_IMPLICIT_WORK_TREE"
                | "GIT_INDEX_FILE"
                | "GIT_INDEX_VERSION"
                | "GIT_LITERAL_PATHSPECS"
                | "GIT_NAMESPACE"
                | "GIT_NOGLOB_PATHSPECS"
                | "GIT_OBJECT_DIRECTORY"
                | "GIT_REPLACE_REF_BASE"
                | "GIT_SHALLOW_FILE"
                | "GIT_WORK_TREE"
        ) || name.starts_with("GIT_CONFIG_")
            || name.starts_with("GIT_DIFF_")
            || name.starts_with("GIT_REDIRECT_")
            || name.starts_with("GIT_TEST_")
        {
            return Err(format!("资源门禁计划核验不允许继承 {name}").into());
        }
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    for (name, _) in env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command
        .args(["--no-pager", "-c", "core.fsmonitor=false"])
        .args(args)
        .current_dir(root)
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(format!(
            "无法核对资源门禁 Git 证据：git {} ({})",
            args[0], output.status
        )
        .into())
    }
}
