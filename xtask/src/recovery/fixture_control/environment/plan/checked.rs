//! 单次读取窗口持有原计划及三份输入；完整来源只在窗口两端核验。
use super::{LIMIT, calculate, guards, input_path};
use crate::{
    Result,
    recovery::{
        clone::native::target_verify::guard,
        fixture_control::{file_binding, read_json, source_pair::execution},
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

struct Document {
    path: PathBuf,
    binding: Value,
    value: Value,
    limit: u64,
}

impl Document {
    fn read(path: PathBuf, limit: u64) -> Result<Self> {
        let binding = file_binding(&path, "夹具输入")?;
        let value = read_json(&path, "夹具输入", limit)?;
        let document = Self {
            path,
            binding,
            value,
            limit,
        };
        document.verify()?;
        Ok(document)
    }

    fn verify(&self) -> Result<()> {
        super::check(
            file_binding(&self.path, "夹具输入")? == self.binding
                && read_json(&self.path, "夹具输入", self.limit)? == self.value
                && file_binding(&self.path, "夹具输入")? == self.binding,
            "夹具环境输入在读取窗口期间发生变化",
        )
    }
}

pub(super) struct Inputs {
    review: Document,
    publication: execution::Window,
    maintenance: Document,
    side: String,
    protection: guard::Inputs,
}

impl Inputs {
    pub(super) fn load(root: &Path, args: &Value) -> Result<Self> {
        let review = input_path(root, &args["review"])?;
        let publication = input_path(root, &args["execution_source"])?;
        let maintenance = input_path(root, &args["maintenance_build"])?;
        let protection =
            guard::Inputs::open([review.clone(), publication.clone(), maintenance.clone()])?;
        let publication = match execution::Window::open(root, &publication) {
            Ok(value) => value,
            Err(error) => {
                return match protection.check() {
                    Ok(()) => Err(error),
                    Err(after) => Err(format!("{error}；输入后验同时失败：{after}").into()),
                };
            }
        };
        let documents: Result<(Document, Document, String)> = (|| {
            Ok((
                Document::read(review, LIMIT)?,
                Document::read(maintenance, LIMIT)?,
                args["side"].as_str().ok_or("侧缺失")?.to_owned(),
            ))
        })();
        let files = protection.check();
        let (review, maintenance, side) = match documents {
            Ok(value) => {
                if let Err(error) = files {
                    finish([Err(error), publication.verify(), protection.check()])?;
                    return Err("夹具输入窗口未完成文件后验".into());
                }
                value
            }
            Err(error) => {
                finish([Err(error), publication.verify(), files, protection.check()])?;
                return Err("夹具输入窗口未完成输入读取".into());
            }
        };
        Ok(Self {
            review,
            publication,
            maintenance,
            side,
            protection,
        })
    }

    pub(super) fn calculate(&self) -> Result<Value> {
        calculate(
            &self.review.value,
            self.publication.publication(),
            &self.maintenance.value,
            &self.review.binding,
            self.publication.binding(),
            &self.maintenance.binding,
            &self.side,
        )
    }

    pub(super) fn verify(&self, expected: Option<&Value>) -> Result<()> {
        let consumed = (|| {
            self.protection.check()?;
            for document in [&self.review, &self.maintenance] {
                document.verify()?;
            }
            Ok(())
        })();
        let source = self.publication.verify();
        let plan = self.calculate().and_then(|value| {
            super::check(
                expected.is_none_or(|expected| value == *expected),
                "夹具环境计划或完整输入绑定已变化",
            )
        });
        finish([consumed, source, plan, self.protection.check()])
    }
}

pub(in crate::recovery::fixture_control) struct CheckedPlan {
    inputs: Inputs,
    plan: Value,
}

impl CheckedPlan {
    pub(in crate::recovery::fixture_control) fn load(root: &Path, plan: &Value) -> Result<Self> {
        let root = super::backend(&json!(root))?;
        let inputs = Inputs::load(
            &root,
            &json!({
            "review":plan["review"]["path"],"execution_source":plan["execution_source"]["path"],
            "maintenance_build":plan["maintenance_build"]["path"],"side":plan["side"]}),
        )?;
        let result = (|| {
            super::check(
                inputs.calculate()? == *plan,
                "夹具环境计划或完整输入绑定已变化",
            )?;
            guards::preflight_binding(&inputs.review.value)?;
            guards::maintenance(&inputs.maintenance.value)
        })();
        if let Err(error) = result {
            finish([Err(error), inputs.verify(Some(plan))])?;
        }
        Ok(Self {
            inputs,
            plan: plan.clone(),
        })
    }

    pub(in crate::recovery::fixture_control) fn projection(&self) -> Value {
        json!({"inputs":[self.inputs.review.path,self.inputs.review.value,
            self.inputs.publication.path(),self.inputs.publication.publication(),
            self.inputs.maintenance.path,self.inputs.maintenance.value]})
    }

    pub(in crate::recovery::fixture_control) fn verify(&self) -> Result<()> {
        finish([
            self.inputs.verify(Some(&self.plan)),
            guards::preflight_binding(&self.inputs.review.value).map(|_| ()),
            guards::maintenance(&self.inputs.maintenance.value),
            self.inputs.protection.check(),
        ])
    }
}

fn finish(checks: impl IntoIterator<Item = Result<()>>) -> Result<()> {
    let mut result = Ok(());
    for check in checks {
        result = match (result, check) {
            (Ok(()), value) | (value, Ok(())) => value,
            (Err(error), Err(after)) => Err(format!("{error}；输入后验同时失败：{after}").into()),
        };
    }
    result
}
