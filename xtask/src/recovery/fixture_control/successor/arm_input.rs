//! 在独立登记与目标存储锁中消费单侧 B1 测量 ARM 请求。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{
    arm::INDEPENDENT_B1_KIND,
    arm_target::{self, IndependentB1Review, Target},
};
use crate::{
    Result,
    cli::FixtureSuccessorArmInput,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{
        clone::native::{
            cache::{
                binding::source::relationship,
                handoff::locks::{Storage, registered_target},
            },
            digest, input, json as native,
            lifecycle::lock::{ProcessGuard, RunLock, owner_observation},
            resources::common,
            target_verify::guard::Inputs,
        },
        fixture_control::portable_path,
        inputs::document::{self, Document},
        source::evidence::comparison::published::{self, Verified},
    },
};

const REQUEST_FIELDS: &[&str] = &[
    "format_version",
    "kind",
    "source_registration",
    "review_successor",
    "source_export",
    "source_export_result",
    "source_rebind",
    "source_generation",
    "id",
    "initialized",
    "target_registration",
    "target_initialized_files",
    "target_environment",
    "copy_directory",
    "build_bridges",
    "independent_b1_review",
    "independent_b1_collision_projection",
];

pub(in crate::recovery::fixture_control) fn run(
    backend: &Path,
    options: &FixtureSuccessorArmInput,
) -> Result<Value> {
    let directory = new_directory(backend, &options.run_dir)?;
    let bound = Bound::read(backend, &options.request)?;
    let initial = bound.observe(false)?;
    let storage = registered_target(
        backend,
        &initial.target.registration,
        &initial.target.generation,
    )?;
    bound.check_paths(&directory, &storage, &initial.review)?;
    common::ensure_new_directory(backend, &directory, "独立 B1 ARM 登记目录")?;
    let metadata = fs::symlink_metadata(&directory)?;
    let (device, inode) = common::device_inode_for(&directory, &metadata)?;
    let identity = json!({"path":directory, "device":device, "inode":inode});
    let run = Registration {
        directory,
        identity,
    };
    match run.execute(&bound, &initial) {
        Ok(result) => Ok(result),
        Err(error) => finish(Err(error), run.record_failure(&bound)),
    }
}

#[derive(PartialEq)]
struct Observation {
    target: Target,
    review: IndependentB1Review,
}

struct Bound<'a> {
    backend: &'a Path,
    request: Document,
    source: Verified,
    work: PathBuf,
    copy: PathBuf,
    inputs: Inputs,
}

impl<'a> Bound<'a> {
    fn read(backend: &'a Path, path: &Path) -> Result<Self> {
        let request = document::read(path, backend, "独立 B1 ARM 私有请求")?;
        validate_request(&request.value)?;
        let value = &request.value;
        let source = published::verified(
            backend,
            &value["source_export_result"],
            Some(&value["review_successor"]),
        )?;
        verify_source(value, &source)?;
        let target =
            common::bound_file(backend, &value["target_registration"], "独立 B1 目标登记")?;
        if target
            .file_name()
            .is_none_or(|name| name != "registration.json")
        {
            return Err("独立 B1 ARM 必须使用原目标 registration.json".into());
        }
        let work = target.parent().ok_or("独立 B1 目标缺少工作区")?.to_owned();
        let copy = new_directory(backend, value_path(&value["copy_directory"])?)?;
        let export = common::bound_file(backend, &source.identity["export"], "原共享导出原件")?;
        let inputs = Inputs::open([request.path.clone(), export])?;
        let bound = Self {
            backend,
            request,
            source,
            work,
            copy,
            inputs,
        };
        bound.check()?;
        Ok(bound)
    }

    fn check(&self) -> Result<()> {
        self.inputs.check()?;
        self.request.assert_unchanged()?;
        self.source.check(self.backend)?;
        common::bound_file(
            self.backend,
            &self.source.identity["export"],
            "原共享导出原件",
        )?;
        if new_directory(
            self.backend,
            value_path(&self.request.value["copy_directory"])?,
        )? != self.copy
        {
            return Err("独立 B1 ARM 原复制路径的目录定位变化".into());
        }
        Ok(())
    }

    fn observe(&self, live: bool) -> Result<Observation> {
        self.check()?;
        let request = &self.request.value;
        let bridges = native::array(&request["build_bridges"], "独立 B1 ARM 构建桥接")?;
        let (target, review) = arm_target::observe_independent_b1(
            self.backend,
            &self.work,
            &request["review_successor"],
            &self.source.source["request"]["source"],
            bridges,
            live,
        )?;
        if target.side != "candidate"
            || target.registration != request["target_registration"]
            || target.initialized != request["initialized"]
            || target.files != request["target_initialized_files"]
            || target.environment != request["target_environment"]
            || review.binding != request["independent_b1_review"]
            || review.collision_projection != request["independent_b1_collision_projection"]
        {
            return Err("独立 B1 ARM 请求与当前初始化、审阅或碰撞投影不同".into());
        }
        self.check()?;
        Ok(Observation { target, review })
    }

    fn check_paths(
        &self,
        directory: &Path,
        storage: &Path,
        review: &IndependentB1Review,
    ) -> Result<()> {
        require_disjoint_paths(directory, &self.copy)?;
        let source_run = value_path(&self.source.source["directory"])?;
        let collision = &self.source.source["review_successor"]["collision_projection"];
        for output in [directory, self.copy.as_path()] {
            for path in [source_run, &self.work, storage, &self.request.path] {
                require_disjoint_paths(output, path)?;
            }
            for role in ["predecessor", "successor"] {
                for item in native::array(&collision[role]["paths"], "原关系受保护路径")? {
                    require_disjoint_paths(output, value_path(&item["identity"])?)?;
                }
            }
            for item in native::array(&review.collision_projection["paths"], "B1 目标受保护路径")?
            {
                if item["owner"] != "measurement.future" {
                    require_disjoint_paths(output, value_path(&item["identity"])?)?;
                }
            }
        }
        Ok(())
    }
}

struct Registration {
    directory: PathBuf,
    identity: Value,
}

impl Registration {
    fn execute(&self, bound: &Bound<'_>, initial: &Observation) -> Result<Value> {
        common::directory_identity(bound.backend, &self.identity)?;
        let manifest = publish(
            bound.backend,
            &self.directory.join("manifest.json"),
            &json!({
                "format_version":1, "kind":"devex-clone-independent-b1-arm-run",
                "directory":self.identity, "id":bound.request.value["id"],
                "request":document::binding(&bound.request),
                "request_canonical_sha256":digest::plan_hash(&bound.request.value)?,
                "copy_directory":bound.request.value["copy_directory"],
            }),
        )?;
        let process = ProcessGuard::acquire(&self.directory)?;
        let local = RunLock::claim(&self.directory)?;
        let check = || {
            process.check()?;
            local.verify()?;
            common::directory_identity(bound.backend, &self.identity)?;
            manifest.assert_unchanged()
        };
        let result = self.under_storage(bound, initial, &manifest, &check);
        let result = finish(result, check());
        let result = finish(result, local.release());
        finish(result, process.check())
    }

    fn under_storage(
        &self,
        bound: &Bound<'_>,
        initial: &Observation,
        manifest: &Document,
        local: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        local()?;
        let storage = Storage::acquire_independent_target(
            bound.backend,
            &self.directory,
            &initial.target.registration,
            &initial.target.generation,
        )?;
        let result = (|| {
            let check = || {
                local()?;
                storage.check()?;
                if bound.observe(true)? != *initial {
                    return Err("独立 B1 ARM 取得目标锁后的观察与原登记不同".into());
                }
                local()?;
                storage.check()
            };
            check()?;
            let (_, owner) =
                owner_observation(&self.directory)?.ok_or("独立 B1 ARM 缺少本地控制锁")?;
            self.publish(bound, initial, manifest, &owner, &check)
        })();
        storage.finish(result)
    }

    fn publish(
        &self,
        bound: &Bound<'_>,
        observed: &Observation,
        manifest: &Document,
        owner: &Value,
        check: &dyn Fn() -> Result<()>,
    ) -> Result<Value> {
        let target = &observed.target;
        let intent = publish(
            bound.backend,
            &self.directory.join("intent.json"),
            &json!({
                "format_version":1, "kind":"devex-clone-independent-b1-arm-intent",
                "registration":document::binding(manifest), "controller":owner,
                "request":document::binding(&bound.request),
                "source_export":bound.source.identity, "source_state":bound.source.state,
                "target_side":target.side, "target_registration":target.registration,
                "initialized":target.initialized, "target_initialized_files":target.files,
                "target_environment":target.environment, "target_storage_run":target.generation,
                "target_storage_state":target.state, "independent_b1_review":observed.review.binding,
                "independent_b1_collision_projection":observed.review.collision_projection,
                "copy_directory":bound.request.value["copy_directory"],
                "remote_writes":0, "restore_qualified":false,
            }),
        )?;
        check()?;
        intent.assert_unchanged()?;
        let result = publish(
            bound.backend,
            &self.directory.join("result.json"),
            &json!({
                "format_version":1, "kind":"devex-clone-independent-b1-arm-result",
                "status":"independent_b1_arm_input_registered",
                "registration":document::binding(manifest), "intent":document::binding(&intent),
                "request":document::binding(&bound.request), "target_side":target.side,
                "remote_writes":0, "restore_qualified":false,
            }),
        )?;
        check()?;
        intent.assert_unchanged()?;
        result.assert_unchanged()?;
        Ok(
            json!({"status":result.value["status"], "receipt":document::binding(&result),
            "remote_writes":0, "restore_qualified":false}),
        )
    }

    fn record_failure(&self, bound: &Bound<'_>) -> Result<()> {
        common::directory_identity(bound.backend, &self.identity)?;
        native::publish_new(
            &self.directory.join("failure.json"),
            &json!({"format_version":1,"kind":"devex-clone-independent-b1-arm-failure",
                "status":"unconfirmed","directory":self.identity,
                "request":document::binding(&bound.request),
                "remote_writes":0,"restore_qualified":false}),
            "独立 B1 ARM 失败收据",
        )
    }
}

fn validate_request(value: &Value) -> Result<()> {
    if value["kind"] != INDEPENDENT_B1_KIND || value["format_version"] != 1 {
        return Err("独立 B1 ARM 消费只接受该用途的版本 1 请求".into());
    }
    native::exact(value, REQUEST_FIELDS, "独立 B1 ARM 请求")?;
    input::name(native::text(&value["id"], "独立 B1 ARM 请求 ID")?)?;
    Ok(())
}

fn verify_source(request: &Value, source: &Verified) -> Result<()> {
    let expected = &source.identity;
    for (requested, verified) in [
        ("source_registration", "source_registration"),
        ("source_rebind", "source_rebind"),
        ("review_successor", "review_successor"),
        ("source_generation", "source_generation"),
        ("source_export_result", "result"),
        ("source_export", "export"),
    ] {
        if expected[verified].is_null() || request[requested] != expected[verified] {
            return Err("独立 B1 ARM 请求与原 h 已发布共享导出不同".into());
        }
    }
    if source.source["seed_target"]["side"] != "seed"
        || source.source["review_successor"]["source_result"] != expected["source_registration"]
        || source.source["manifest"]["build_bridges"] != request["build_bridges"]
    {
        return Err("独立 B1 ARM 请求未绑定共享来源的原角色或构建桥接".into());
    }
    Ok(())
}

fn new_directory(backend: &Path, path: &Path) -> Result<PathBuf> {
    validate_local_test_path(path, backend, LocalTestPathKind::NewDirectory)?;
    let parent = path.parent().ok_or("独立 B1 ARM 输出缺少父目录")?;
    let name = path.file_name().ok_or("独立 B1 ARM 输出缺少目录名")?;
    let path = portable_path(&parent.canonicalize()?).join(name);
    Ok(validate_local_test_path(
        &path,
        backend,
        LocalTestPathKind::NewDirectory,
    )?)
}

fn value_path(value: &Value) -> Result<&Path> {
    Ok(Path::new(native::text(value, "独立 B1 ARM 路径")?))
}

fn require_disjoint_paths(left: &Path, right: &Path) -> Result<()> {
    let left = portable_path(left);
    let right = portable_path(right);
    if relationship::paths_overlap(&left.to_string_lossy(), &right.to_string_lossy()) {
        return Err("独立 B1 ARM 输出路径与现有资源或复制路径重叠".into());
    }
    Ok(())
}

fn publish(backend: &Path, file: &Path, value: &Value) -> Result<Document> {
    native::publish_new(file, value, "独立 B1 ARM 本地收据")?;
    let saved = document::read(file, backend, "独立 B1 ARM 本地收据")?;
    if saved.value != *value {
        return Err("独立 B1 ARM 收据发布回读不同".into());
    }
    Ok(saved)
}

fn finish<T>(result: Result<T>, checked: Result<()>) -> Result<T> {
    match (result, checked) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(check)) => Err(format!("{error}；独立 B1 ARM 收尾也失败：{check}").into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Value {
        let mut value = Value::Object(
            REQUEST_FIELDS
                .iter()
                .map(|key| ((*key).into(), Value::Null))
                .collect(),
        );
        value["format_version"] = json!(1);
        value["kind"] = json!(INDEPENDENT_B1_KIND);
        value["id"] = json!("independent-arm");
        value
    }

    #[test]
    fn independent_b1_arm_input_rejects_old_kinds_and_non_exact_fields() {
        let value = request();
        assert!(validate_request(&value).is_ok());
        for kind in [
            "devex-clone-seed-arm-input",
            "devex-clone-seed-successor-arm-input",
        ] {
            let mut old = value.clone();
            old["kind"] = json!(kind);
            assert!(validate_request(&old).is_err());
        }
        let mut extra = value.clone();
        extra["target_side"] = json!("candidate");
        assert!(validate_request(&extra).is_err());
        let mut missing = value;
        missing
            .as_object_mut()
            .unwrap()
            .remove("independent_b1_review");
        assert!(validate_request(&missing).is_err());
    }

    #[test]
    fn independent_b1_arm_input_requires_every_original_export_and_bridge_binding() {
        let mut value = request();
        let mut source = Verified {
            identity: json!({}),
            source: json!({"seed_target":{"side":"seed"},
                "review_successor":{"source_result":"source_registration"},
                "manifest":{"build_bridges":[]}}),
            state: Value::Null,
        };
        let fields = [
            ("source_registration", "source_registration"),
            ("source_rebind", "source_rebind"),
            ("source_generation", "source_generation"),
            ("review_successor", "review_successor"),
            ("source_export_result", "result"),
            ("source_export", "export"),
        ];
        for (request, original) in fields {
            value[request] = json!(request);
            source.identity[original] = json!(request);
        }
        value["build_bridges"] = json!([]);
        assert!(verify_source(&value, &source).is_ok());
        for field in fields
            .map(|(field, _)| field)
            .into_iter()
            .chain(["build_bridges"])
        {
            let mut wrong = value.clone();
            wrong[field] = Value::Null;
            assert!(verify_source(&wrong, &source).is_err(), "{field}");
        }
    }

    #[test]
    fn independent_b1_arm_input_rejects_reused_or_nested_local_paths() {
        let root = Path::new("bench");
        let occupied = root.join("existing");
        assert!(require_disjoint_paths(&occupied, &occupied).is_err());
        assert!(require_disjoint_paths(&occupied, &occupied.join("new")).is_err());
        assert!(require_disjoint_paths(root, &occupied).is_err());
        assert!(require_disjoint_paths(&root.join("another"), &occupied).is_ok());
        #[cfg(windows)]
        assert!(require_disjoint_paths(&occupied, &root.join("EXISTING")).is_err());
    }
}
