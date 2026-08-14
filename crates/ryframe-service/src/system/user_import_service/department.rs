struct ImportCandidate {
    row_number: i32,
    data: UserImportData,
    department_id: i64,
}

struct DepartmentDirectory {
    by_path: HashMap<String, Vec<DepartmentTarget>>,
}

#[derive(Clone)]
struct DepartmentTarget {
    id: i64,
    hierarchy_valid: bool,
    enabled: bool,
}

#[derive(Clone)]
struct DepartmentPathState {
    path: String,
    hierarchy_valid: bool,
    enabled: bool,
}

struct DepartmentIssue {
    code: &'static str,
    message: String,
}

impl DepartmentDirectory {
    fn from_departments(departments: Vec<dept::Model>) -> Self {
        let by_id = departments
            .into_iter()
            .map(|department| (department.id, department))
            .collect::<HashMap<_, _>>();
        let mut cache = HashMap::new();
        let mut by_path: HashMap<String, Vec<DepartmentTarget>> = HashMap::new();

        for id in by_id.keys().copied() {
            let mut visiting = HashSet::new();
            let Ok(state) = resolve_department_path(id, &by_id, &mut cache, &mut visiting, 0)
            else {
                continue;
            };
            // 与行解析共用同一长度边界，避免模板发放 Worker 必然拒绝的路径。
            if state.path.len() > DEPARTMENT_PATH_MAX_BYTES {
                continue;
            }
            by_path
                .entry(state.path)
                .or_default()
                .push(DepartmentTarget {
                    id,
                    hierarchy_valid: state.hierarchy_valid,
                    enabled: state.enabled,
                });
        }

        Self { by_path }
    }

    fn resolve(
        &self,
        value: Option<&str>,
        actor: &ActorContext,
    ) -> Result<DepartmentTarget, DepartmentIssue> {
        let path = value
            .map(str::trim)
            .filter(|path| !path.is_empty())
            .ok_or_else(|| DepartmentIssue {
                code: "department_required",
                message: "部门完整路径不能为空".into(),
            })?;
        if path.len() > DEPARTMENT_PATH_MAX_BYTES {
            return Err(DepartmentIssue {
                code: "department_path_too_long",
                message: format!("部门完整路径不能超过 {DEPARTMENT_PATH_MAX_BYTES} 字节"),
            });
        }
        let Some(matches) = self.by_path.get(path) else {
            return Err(DepartmentIssue {
                code: "department_not_found",
                message: "部门完整路径不存在或不属于当前租户".into(),
            });
        };
        if matches.len() != 1 {
            return Err(DepartmentIssue {
                code: "department_ambiguous",
                message: "部门完整路径对应多个部门，请先整理重复的部门层级".into(),
            });
        }
        let department = matches[0].clone();
        if !department.hierarchy_valid {
            return Err(DepartmentIssue {
                code: "department_invalid_hierarchy",
                message: "部门层级数据无效，请先修复部门树".into(),
            });
        }
        if !department.enabled {
            return Err(DepartmentIssue {
                code: "department_disabled",
                message: "部门或其上级部门已停用".into(),
            });
        }
        if !department_is_visible(actor, department.id) {
            return Err(DepartmentIssue {
                code: "department_out_of_scope",
                message: "部门超出申请人的当前数据范围".into(),
            });
        }
        Ok(department)
    }

    fn available_paths(&self, actor: &ActorContext) -> AppResult<Vec<String>> {
        let mut paths = self
            .by_path
            .iter()
            .filter(|(_, matches)| {
                matches.len() == 1
                    && matches[0].hierarchy_valid
                    && matches[0].enabled
                    && department_is_visible(actor, matches[0].id)
            })
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        paths.sort_unstable();
        Ok(paths)
    }
}

fn resolve_department_path(
    id: i64,
    by_id: &HashMap<i64, dept::Model>,
    cache: &mut HashMap<i64, Result<DepartmentPathState, ()>>,
    visiting: &mut HashSet<i64>,
    depth: usize,
) -> Result<DepartmentPathState, ()> {
    if let Some(cached) = cache.get(&id) {
        return cached.clone();
    }
    if depth >= DEPARTMENT_HIERARCHY_MAX_DEPTH || !visiting.insert(id) {
        return Err(());
    }
    let result = (|| {
        let department = by_id.get(&id).ok_or(())?;
        let name = department.name.trim();
        if name.is_empty() {
            return Err(());
        }
        match department.parent_id {
            None => Ok(DepartmentPathState {
                path: name.to_owned(),
                hierarchy_valid: department.ancestors == "0",
                enabled: department.is_enabled(),
            }),
            Some(parent_id) => {
                let parent = by_id.get(&parent_id).ok_or(())?;
                let parent_state = resolve_department_path(
                    parent_id,
                    by_id,
                    cache,
                    visiting,
                    depth.saturating_add(1),
                )?;
                Ok(DepartmentPathState {
                    path: format!("{}{DEPARTMENT_PATH_SEPARATOR}{name}", parent_state.path),
                    hierarchy_valid: parent_state.hierarchy_valid
                        && department.ancestors == format!("{},{}", parent.ancestors, parent.id),
                    enabled: parent_state.enabled && department.is_enabled(),
                })
            }
        }
    })();
    visiting.remove(&id);
    cache.insert(id, result.clone());
    result
}
