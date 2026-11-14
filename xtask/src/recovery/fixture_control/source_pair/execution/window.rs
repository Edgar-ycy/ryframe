//! 具体执行来源读取窗口；权威读取留在所有者线程，其余窗口并行观察四根。
use super::{file_binding, input, read_json, root_receipt_observed, source};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    recovery::{clone::native::target_verify::guard, runtime::dataset_authority::reads},
};
use serde_json::Value;
use std::{
    error::Error,
    path::{Path, PathBuf},
    thread::{self, ScopedJoinHandle},
};

struct Publication {
    path: PathBuf,
    binding: Value,
    value: Value,
    protection: guard::Inputs,
}

struct PublishedRoot {
    name: &'static str,
    path: PathBuf,
    files: Vec<source::FileRecord>,
    receipt: Value,
}

pub(in crate::recovery::fixture_control) struct Window {
    publication: Publication,
    fixture: input::Fixture,
    roots: [PublishedRoot; 2],
}

impl Publication {
    fn open(path: &Path, expected: Option<&Value>) -> Result<Self> {
        let protection = guard::Inputs::open([path.to_owned()])?;
        let result: Result<(Value, Value)> = (|| {
            if path.file_name().and_then(|v| v.to_str()) != Some("execution-source.json") {
                return Err("执行源码清单必须命名为 execution-source.json".into());
            }
            let binding = file_binding(path, "执行源码清单")?;
            check(
                expected.is_none_or(|value| *value == binding),
                "执行源码清单绑定漂移；重新核对来源，禁止改写旧绑定",
            )?;
            let value = read_json(path, "执行源码清单", 16 * 1024 * 1024)?;
            super::exact(
                &value,
                &["format_version", "kind", "original", "published"],
                "执行源码清单",
            )?;
            check(
                value["format_version"] == 1
                    && value["kind"] == "reference-fixture-execution-source",
                "执行源码清单版本或类型无效",
            )?;
            Ok((binding, value))
        })();
        let after = protection.check();
        let (binding, value) = match result {
            Ok(value) => {
                after?;
                value
            }
            Err(error) => return Err(failure(error, after)),
        };
        Ok(Self {
            path: path.to_owned(),
            binding,
            value,
            protection,
        })
    }

    fn verify(&self) -> Result<()> {
        let binding = file_binding(&self.path, "执行源码清单")
            .and_then(|value| check(value == self.binding, "执行源码清单在来源复核期间发生变化"));
        let document = read_json(&self.path, "执行源码清单", 16 * 1024 * 1024)
            .and_then(|value| check(value == self.value, "执行来源复核期间清单 JSON 变化"));
        finish([self.protection.check(), binding, document])
    }
}

impl Window {
    pub(in crate::recovery::fixture_control) fn open(root: &Path, path: &Path) -> Result<Self> {
        validate_local_test_path(path, root, LocalTestPathKind::ExistingFile)?;
        Self::bound(path, None)
    }

    pub(super) fn bound(path: &Path, expected: Option<&Value>) -> Result<Self> {
        let publication = Publication::open(path, expected)?;
        let setup: Result<(input::Fixture, [PublishedRoot; 2])> = (|| {
            let roots = published_roots(&publication)?;
            let backend = Path::new(
                publication.value["original"]["paths"]["backend"]
                    .as_str()
                    .ok_or("原后端路径缺失")?,
            );
            Ok((input::Fixture::open(backend)?, roots))
        })();
        let documents = publication.verify();
        let (fixture, roots) = match setup {
            Ok(value) => value,
            Err(error) => return Err(failure(error, documents)),
        };
        let window = Self {
            publication,
            fixture,
            roots,
        };
        let before = window.observe();
        let files = window.fixture.verify();
        if let Err(error) = finish([documents, before, files]) {
            return Err(failure(error, window.verify()));
        }
        Ok(window)
    }

    pub(in crate::recovery::fixture_control) fn path(&self) -> &Path {
        &self.publication.path
    }
    pub(in crate::recovery::fixture_control) fn binding(&self) -> &Value {
        &self.publication.binding
    }
    /// 仅供本次窗口内纯计算；调用方必须末端 verify 后才能接受结果。
    pub(in crate::recovery::fixture_control) fn publication(&self) -> &Value {
        &self.publication.value
    }

    pub(in crate::recovery::fixture_control) fn verify(&self) -> Result<()> {
        let roots = self.observe();
        let fixture = self.fixture.verify();
        let publication = self.publication.verify();
        finish([roots, fixture, publication])
    }

    #[cfg(windows)]
    pub(super) fn check_documents(&self) -> Result<()> {
        finish([
            self.publication.protection.checkpoint(),
            self.fixture.check_identity(),
        ])
    }

    fn observe(&self) -> Result<()> {
        if !reads::is_active() {
            return self.observe_parallel();
        }
        let original = self.fixture.observe().and_then(|value| {
            check(
                value.receipt == self.publication.value["original"],
                "原 fixture 绑定或生成源码已漂移；禁止沿用执行来源",
            )
        });
        // 先取得全部结果，再收集错误；GB/GF 失败不能跳过 PB/PF。
        let backend = self.roots[0].observe();
        let frontend = self.roots[1].observe();
        finish([original, backend, frontend])
    }

    fn observe_parallel(&self) -> Result<()> {
        let sources = self.fixture.sources();
        let generated = &sources;
        let published_backend = &self.roots[0];
        let published_frontend = &self.roots[1];
        let (generated_backend, generated_frontend, backend, frontend) = thread::scope(|scope| {
            let gb = thread::Builder::new()
                .name("execution-generated-backend".into())
                .spawn_scoped(scope, move || generated_observation(generated, "backend"));
            let gf = thread::Builder::new()
                .name("execution-generated-frontend".into())
                .spawn_scoped(scope, move || generated_observation(generated, "frontend"));
            let pb = thread::Builder::new()
                .name("execution-published-backend".into())
                .spawn_scoped(scope, move || published_observation(published_backend));
            let pf = thread::Builder::new()
                .name("execution-published-frontend".into())
                .spawn_scoped(scope, move || published_observation(published_frontend));
            // 创建或观察失败也逐一 join，不能让后续根丢失错误或越过保护窗口。
            let gb = joined("生成来源 backend", gb);
            let gf = joined("生成来源 frontend", gf);
            let pb = joined("发布来源 backend", pb);
            let pf = joined("发布来源 frontend", pf);
            (gb, gf, pb, pf)
        });
        let original = self
            .fixture
            .combine(generated_backend, generated_frontend)
            .and_then(|value| {
                check(
                    value.receipt == self.publication.value["original"],
                    "原 fixture 绑定或生成源码已漂移；禁止沿用执行来源",
                )
            });
        finish([original, backend, frontend])
    }
}

fn generated_observation(
    sources: &input::Sources<'_>,
    name: &'static str,
) -> std::result::Result<input::Side, String> {
    match std::panic::catch_unwind(|| sources.side(name)) {
        Ok(result) => result.map_err(|error| format!("生成来源 {name}：{error}")),
        Err(_) => Err(format!("生成来源 {name}：来源观察线程发生 panic")),
    }
}

fn published_observation(root: &PublishedRoot) -> std::result::Result<(), String> {
    match std::panic::catch_unwind(|| root.observe()) {
        Ok(result) => result.map_err(|error| format!("发布来源 {}：{error}", root.name)),
        Err(_) => Err(format!("发布来源 {}：来源观察线程发生 panic", root.name)),
    }
}

fn joined<T>(
    label: &str,
    handle: std::io::Result<ScopedJoinHandle<'_, std::result::Result<T, String>>>,
) -> Result<T> {
    match handle {
        Ok(handle) => match handle.join() {
            Ok(result) => result.map_err(Into::into),
            Err(_) => Err(format!("{label}：来源观察线程异常结束").into()),
        },
        Err(error) => Err(format!("{label}：无法创建来源观察线程：{error}").into()),
    }
}

impl PublishedRoot {
    fn observe(&self) -> Result<()> {
        let actual = root_receipt_observed(&self.path, &self.files, self.name, false)
            .map_err(|error| format!("发布来源 {}：{error}", self.name))?;
        if actual != self.receipt {
            let changed = ["path", "head", "tree", "inventory", "fingerprints"]
                .into_iter()
                .filter(|field| self.receipt[*field] != actual[*field])
                .collect::<Vec<_>>();
            let changed = if changed.is_empty() {
                "结构".into()
            } else {
                changed.join("、")
            };
            return Err(format!(
                "发布来源 {}：发布源码 HEAD、tree、完整清单或来源指纹发生变化；变化字段：{changed}",
                self.name
            )
            .into());
        }
        Ok(())
    }
}

fn published_roots(publication: &Publication) -> Result<[PublishedRoot; 2]> {
    super::exact(
        &publication.value["published"],
        &["backend", "frontend"],
        "发布源码组合",
    )?;
    let parent = publication.path.parent().ok_or("执行源码清单缺少父目录")?;
    let make = |name: &'static str| -> Result<PublishedRoot> {
        let inventory =
            source::validate_inventory(&publication.value["original"]["current"][name])?;
        let files = if name == "frontend" {
            super::assets::expected(
                &inventory.files,
                &publication.value["original"]["contract_assets"],
            )?
        } else {
            inventory.files
        };
        Ok(PublishedRoot {
            name,
            path: parent.join(name),
            files,
            receipt: publication.value["published"][name].clone(),
        })
    };
    Ok([make("backend")?, make("frontend")?])
}

pub(super) fn check(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(message.into())
    }
}

pub(super) fn finish(checks: impl IntoIterator<Item = Result<()>>) -> Result<()> {
    let mut result = Ok(());
    for checked in checks {
        result = match (result, checked) {
            (Ok(()), value) | (value, Ok(())) => value,
            (Err(error), Err(after)) => {
                Err(format!("{error}；执行来源后验同时失败：{after}").into())
            }
        };
    }
    result
}

pub(super) fn failure(error: Box<dyn Error>, after: Result<()>) -> Box<dyn Error> {
    match after {
        Ok(()) => error,
        Err(after) => format!("{error}；执行来源后验同时失败：{after}").into(),
    }
}
