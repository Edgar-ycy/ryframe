use super::*;

pub(super) fn parse_runtime_feature_tree_policy(
    value: Option<&Value>,
) -> PolicyResult<RuntimeFeatureTreeGate> {
    let gate = object(value, "runtime_feature_tree_gate")?;
    exact_keys(gate, &["profiles"], "runtime_feature_tree_gate")?;
    let entries = array(gate.get("profiles"), "runtime_feature_tree_gate.profiles")?;
    if entries.is_empty() {
        return Err("runtime_feature_tree_gate.profiles 不得为空".into());
    }
    let mut seen_profiles = BTreeSet::new();
    let mut seen_commands = BTreeSet::new();
    let mut profiles = Vec::with_capacity(entries.len());
    for (index, value) in entries.iter().enumerate() {
        let label = format!("runtime_feature_tree_gate.profiles[{index}]");
        let profile = object(Some(value), &label)?;
        exact_keys(profile, &["name", "command", "constraints"], &label)?;
        let name = text(profile.get("name"), &format!("{label}.name"), 1)?;
        if !package_name(&name) {
            return Err(format!("{label}.name 不是合法的 profile 名称"));
        }
        if !seen_profiles.insert(name.clone()) {
            return Err(format!("{label}.name 重复"));
        }
        let command = parse_feature_command(profile.get("command"), &label)?;
        if !seen_commands.insert(command.clone()) {
            return Err(format!("{label}.command 与其他 profile 重复"));
        }
        let constraints = parse_feature_constraints(profile.get("constraints"), &label)?;
        profiles.push(FeatureProfile {
            name,
            command,
            constraints,
        });
    }
    let present = profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .collect::<BTreeSet<_>>();
    let missing = REQUIRED_FEATURE_TREE_PROFILES
        .into_iter()
        .filter(|name| !present.contains(name))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "runtime_feature_tree_gate 缺少强制 profile：{}",
            missing.join("、")
        ));
    }
    Ok(RuntimeFeatureTreeGate { profiles })
}

fn parse_feature_command(
    value: Option<&Value>,
    profile_label: &str,
) -> PolicyResult<FeatureCommand> {
    let label = format!("{profile_label}.command");
    let command = object(value, &label)?;
    exact_keys(
        command,
        &["package", "no_default_features", "features", "edges"],
        &label,
    )?;
    let package = text(command.get("package"), &format!("{label}.package"), 1)?;
    if !package_name(&package) {
        return Err(format!("{label}.package 不是合法的 Cargo 包名"));
    }
    let no_default_features = command
        .get("no_default_features")
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("{label}.no_default_features 必须是布尔值"))?;
    let features = unique_names(
        command.get("features"),
        &format!("{label}.features"),
        true,
        false,
    )?;
    let edges = unique_names(
        command.get("edges"),
        &format!("{label}.edges"),
        false,
        false,
    )?;
    if edges.iter().map(String::as_str).collect::<BTreeSet<_>>()
        != BTreeSet::from(["normal", "build"])
    {
        return Err(format!("{label}.edges 必须且只能包含 normal、build"));
    }
    Ok(FeatureCommand {
        package,
        no_default_features,
        features,
        edges,
    })
}

fn parse_feature_constraints(
    value: Option<&Value>,
    profile_label: &str,
) -> PolicyResult<FeatureConstraints> {
    let label = format!("{profile_label}.constraints");
    let constraints = object(value, &label)?;
    exact_keys(
        constraints,
        &[
            "required_packages",
            "required_features",
            "forbidden_packages",
            "forbidden_features",
            "max_unique_packages",
        ],
        &label,
    )?;
    let required_packages = unique_names(
        constraints.get("required_packages"),
        &format!("{label}.required_packages"),
        true,
        false,
    )?;
    let forbidden_packages = unique_names(
        constraints.get("forbidden_packages"),
        &format!("{label}.forbidden_packages"),
        true,
        false,
    )?;
    let required_set = required_packages.iter().collect::<BTreeSet<_>>();
    let forbidden_set = forbidden_packages.iter().collect::<BTreeSet<_>>();
    if !required_set.is_disjoint(&forbidden_set) {
        return Err(format!(
            "{profile_label} 的 required/forbidden package 不得重叠"
        ));
    }
    let required_features = feature_contract(
        constraints.get("required_features"),
        &format!("{label}.required_features"),
        true,
    )?;
    let forbidden_features = feature_contract(
        constraints.get("forbidden_features"),
        &format!("{label}.forbidden_features"),
        true,
    )?;
    for (package, required) in &required_features {
        let Some(forbidden) = forbidden_features.get(package) else {
            continue;
        };
        if !required
            .iter()
            .collect::<BTreeSet<_>>()
            .is_disjoint(&forbidden.iter().collect::<BTreeSet<_>>())
        {
            return Err(format!(
                "{profile_label} 的 required/forbidden feature 不得重叠：{package}"
            ));
        }
    }
    let max_unique_packages = match constraints.get("max_unique_packages") {
        Some(Value::Null) => None,
        Some(value) => {
            let maximum = value
                .as_u64()
                .filter(|value| *value >= 1)
                .ok_or_else(|| format!("{label}.max_unique_packages 必须为正整数或 null"))?;
            Some(
                usize::try_from(maximum)
                    .map_err(|_| format!("{label}.max_unique_packages 超出当前平台可表示范围"))?,
            )
        }
        None => return Err(format!("{label}.max_unique_packages 必须为正整数或 null")),
    };
    Ok(FeatureConstraints {
        required_packages,
        required_features,
        forbidden_packages,
        forbidden_features,
        max_unique_packages,
    })
}

fn feature_contract(
    value: Option<&Value>,
    label: &str,
    allow_empty: bool,
) -> PolicyResult<BTreeMap<String, Vec<String>>> {
    let contract = object(value, label)?;
    if !allow_empty && contract.is_empty() {
        return Err(format!("{label} 不得为空"));
    }
    contract
        .iter()
        .map(|(package, features)| {
            if !package_name(package) {
                return Err(format!("{label} 包含非法的 Cargo 包名"));
            }
            Ok((
                package.clone(),
                unique_names(Some(features), &format!("{label}.{package}"), false, true)?,
            ))
        })
        .collect()
}

fn unique_names(
    value: Option<&Value>,
    label: &str,
    allow_empty: bool,
    feature_name: bool,
) -> PolicyResult<Vec<String>> {
    let names = string_array(value, label, allow_empty)?;
    let pattern_ok = if feature_name {
        cargo_feature_name
    } else {
        package_name
    };
    if names.iter().any(|name| !pattern_ok(name)) {
        return Err(format!("{label} 包含非法的 Cargo 名称"));
    }
    let unique = names.iter().collect::<BTreeSet<_>>();
    if unique.len() != names.len() {
        let qualifier = if allow_empty { "" } else { "为空或" };
        return Err(format!("{label} 不得{qualifier}包含重复项"));
    }
    Ok(names)
}
