use std::{collections::BTreeSet, path::Path};

use super::{
    BusinessGenerateReport, BusinessPackage,
    catalog::load_descriptors,
    metadata::business_packages,
    model::descriptor_to_ir,
    writer::{BusinessAsset, install_assets},
};
use crate::{ResourceError, resource::business_frontend_assets};

/// 从全部已注册业务模型生成页面，沿用标准 CRUD 模板，不修改框架生成目录的入口。
pub(crate) fn generate_business_frontend(
    workspace: &Path,
    frontend: &Path,
    catalog_target: &Path,
) -> Result<BusinessGenerateReport, ResourceError> {
    let mut resources = Vec::new();
    let mut names = BTreeSet::new();
    for package in business_packages(workspace)? {
        for descriptor in load_descriptors(&package, Some(catalog_target))? {
            if !names.insert(descriptor.name.clone()) {
                return Err(ResourceError::new(
                    format!("业务前端资源名称冲突：{}；本批未写入", descriptor.name),
                    "为 ResourceModel 设置不同的 name，不覆盖其他模块或框架页面",
                ));
            }
            resources.push(descriptor_to_ir(&package, &descriptor)?);
        }
    }
    let assets = business_frontend_assets(&resources)?
        .into_iter()
        .map(|asset| BusinessAsset::new(asset.resource, asset.path, asset.content))
        .collect();
    let frontend = BusinessPackage {
        name: "business-frontend".into(),
        module: "business".into(),
        root: frontend.into(),
        workspace_root: workspace.into(),
    };
    install_assets(&frontend, &names, assets, true)
}
