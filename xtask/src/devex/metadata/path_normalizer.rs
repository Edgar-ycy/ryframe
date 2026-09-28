use std::path::Path;

#[derive(Debug, Clone)]
pub(crate) struct PathNormalizer {
    replacements: Vec<(String, &'static str)>,
}

impl PathNormalizer {
    pub(crate) fn new(backend: &Path, frontend: &Path, devex: &Path) -> Self {
        let mut replacements = vec![
            (normalized_path(devex), "$DEVEX"),
            (normalized_path(frontend), "$FRONTEND"),
            (normalized_path(backend), "$BACKEND"),
        ];
        replacements.sort_by_key(|entry| std::cmp::Reverse(entry.0.len()));
        Self { replacements }
    }

    pub(crate) fn normalize(&self, value: &str) -> String {
        let mut value = value.replace('\\', "/");
        for (path, replacement) in &self.replacements {
            value = value.replace(path, replacement);
        }
        value
    }

    pub(super) fn normalize_environment_value(&self, value: &str) -> String {
        let normalized = self.normalize(value);
        if Path::new(&normalized).is_absolute() {
            let name = Path::new(&normalized)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("executable");
            format!("$EXTERNAL_PATH/{name}")
        } else {
            normalized
        }
    }
}

fn normalized_path(path: &Path) -> String {
    path.to_string_lossy()
        .trim_end_matches(['/', '\\'])
        .replace('\\', "/")
}
