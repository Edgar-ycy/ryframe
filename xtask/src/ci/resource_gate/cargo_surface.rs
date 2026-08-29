use std::collections::BTreeSet;

use crate::Result;

#[derive(Debug, Clone, Copy)]
struct PackageSurface {
    package: &'static str,
    features: &'static [&'static str],
    has_lib: bool,
    bins: &'static [&'static str],
    tests: &'static [&'static str],
}

const PACKAGE_SURFACES: &[PackageSurface] = &[
    PackageSurface {
        package: "ryframe",
        features: &["bin-api"],
        has_lib: true,
        bins: &["ryframe"],
        tests: &["bin_feature_contract", "boot_contract"],
    },
    PackageSurface {
        package: "ryframe-adapters",
        features: &["image-processing", "otel", "redis-api", "spreadsheet"],
        has_lib: true,
        bins: &[],
        tests: &["adapter_contracts"],
    },
    PackageSurface {
        package: "ryframe-api",
        features: &[],
        has_lib: true,
        bins: &["export_openapi"],
        tests: &["api_contracts"],
    },
    PackageSurface {
        package: "ryframe-application",
        features: &["test-support"],
        has_lib: true,
        bins: &[],
        tests: &["application_contracts"],
    },
    PackageSurface {
        package: "ryframe-auth",
        features: &[],
        has_lib: true,
        bins: &[],
        tests: &["security_contracts"],
    },
    PackageSurface {
        package: "ryframe-config",
        features: &[],
        has_lib: true,
        bins: &[],
        tests: &["configuration_contracts"],
    },
    PackageSurface {
        package: "ryframe-db",
        features: &["repositories", "migration"],
        has_lib: true,
        bins: &["export_mysql_snapshot"],
        tests: &["mapping_contracts", "mysql_real_protocol"],
    },
    PackageSurface {
        package: "ryframe-generator",
        features: &[],
        has_lib: true,
        bins: &[],
        tests: &["resource_contracts"],
    },
    PackageSurface {
        package: "ryframe-kernel",
        features: &[],
        has_lib: true,
        bins: &[],
        tests: &["kernel_contracts"],
    },
    PackageSurface {
        package: "ryframe-macro",
        features: &[],
        has_lib: true,
        bins: &[],
        tests: &["patch_route"],
    },
    PackageSurface {
        package: "ryframe-tenant-db",
        features: &["repositories", "migration"],
        has_lib: true,
        bins: &[],
        tests: &["tenant_mapping_contracts"],
    },
    PackageSurface {
        package: "xtask",
        features: &["resource"],
        has_lib: false,
        bins: &["xtask"],
        tests: &["internal"],
    },
];

pub(crate) fn affected_package_args_for_target(
    operation: &str,
    packages: &BTreeSet<String>,
    target_dir: &str,
    jobs: usize,
) -> Result<Vec<String>> {
    if !matches!(operation, "clippy" | "test") {
        return Err(format!("resource gate 不支持 Cargo 操作：{operation}").into());
    }
    if packages.is_empty() {
        return Err("resource gate 受影响 crate 集合为空".into());
    }

    let surfaces = packages
        .iter()
        .map(|package| {
            let mut matching = PACKAGE_SURFACES
                .iter()
                .filter(|surface| surface.package == package);
            let surface = matching
                .next()
                .ok_or_else(|| format!("resource gate 缺少 crate 精确编译面：{package}"))?;
            if matching.next().is_some() {
                return Err(format!("resource gate crate 编译面重复：{package}"));
            }
            Ok(surface)
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let features = surfaces
        .iter()
        .flat_map(|surface| {
            surface
                .features
                .iter()
                .map(move |feature| format!("{}/{feature}", surface.package))
        })
        .collect::<BTreeSet<_>>();
    let bins = surfaces
        .iter()
        .flat_map(|surface| surface.bins.iter().copied())
        .collect::<BTreeSet<_>>();
    let tests = surfaces
        .iter()
        .flat_map(|surface| surface.tests.iter().copied())
        .collect::<BTreeSet<_>>();

    let mut args = vec![
        operation.to_owned(),
        "--locked".to_owned(),
        "--target-dir".to_owned(),
        target_dir.to_owned(),
        "--no-default-features".to_owned(),
    ];
    for package in packages {
        args.extend(["-p".to_owned(), package.clone()]);
    }
    if !features.is_empty() {
        args.extend([
            "--features".to_owned(),
            features.into_iter().collect::<Vec<_>>().join(","),
        ]);
    }
    if surfaces.iter().any(|surface| surface.has_lib) {
        args.push("--lib".to_owned());
    }
    if operation == "clippy" {
        for bin in bins {
            args.extend(["--bin".to_owned(), bin.to_owned()]);
        }
    }
    for test in tests {
        args.extend(["--test".to_owned(), test.to_owned()]);
    }
    args.extend(["--jobs".to_owned(), jobs.max(1).to_string()]);
    if operation == "clippy" {
        args.extend([
            "--".to_owned(),
            "-D".to_owned(),
            "warnings".to_owned(),
            "-D".to_owned(),
            "clippy::redundant_clone".to_owned(),
        ]);
    }
    Ok(args)
}
