use ryframe_common::AppError;
use validator::ValidationError;

pub(super) fn validate_password_complexity(value: &str) -> Result<(), ValidationError> {
    ryframe_auth::password::validate_complexity(value).map_err(|error| {
        let message = match error {
            AppError::Validation(message) => message,
            _ => "密码不符合安全策略".into(),
        };
        let mut validation_error = ValidationError::new("password_complexity");
        validation_error.message = Some(message.into());
        validation_error
    })
}
