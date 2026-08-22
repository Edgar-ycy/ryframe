use std::{fmt, path::Path};

/// 面向开发者的资源生成错误。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceError {
    pub resource: Option<String>,
    pub field: Option<String>,
    pub file: Option<String>,
    pub message: String,
    pub suggestion: String,
}

impl ResourceError {
    pub fn new(message: impl Into<String>, suggestion: impl Into<String>) -> Self {
        Self {
            resource: None,
            field: None,
            file: None,
            message: message.into(),
            suggestion: suggestion.into(),
        }
    }

    pub fn file(
        path: impl AsRef<Path>,
        message: impl Into<String>,
        suggestion: impl Into<String>,
    ) -> Self {
        Self::new(message, suggestion).with_file(path.as_ref().to_string_lossy())
    }

    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }

    pub fn with_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into().replace('\\', "/"));
        self
    }
}

impl fmt::Display for ResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut context = Vec::new();
        if let Some(resource) = &self.resource {
            context.push(format!("资源 {resource}"));
        }
        if let Some(field) = &self.field {
            context.push(format!("字段 {field}"));
        }
        if let Some(file) = &self.file {
            context.push(format!("文件 {file}"));
        }
        if !context.is_empty() {
            write!(formatter, "{}：", context.join("，"))?;
        }
        write!(formatter, "{}；修复建议：{}", self.message, self.suggestion)
    }
}

impl std::error::Error for ResourceError {}
