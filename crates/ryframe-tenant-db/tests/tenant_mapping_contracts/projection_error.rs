use super::map_error;
use crate::TenantDataError;
use ryframe_kernel::AppError;

#[test]
fn projection_missing_or_behind_is_retryable_data_unavailability() {
    for actual in [None, Some(8)] {
        let error = map_error(TenantDataError::DataScopeProjectionStale {
            tenant_id: "projection-tenant".into(),
            expected: 9,
            actual,
        });
        let AppError::TenantDataTargetUnavailable(message, retry_after) = error else {
            panic!("投影落后必须报告业务数据暂不可用，不能要求刷新产品会话");
        };
        assert_eq!(retry_after, 5);
        assert!(message.contains("projection-tenant"));
        assert!(message.contains("expected=9"));
        assert!(message.contains(&format!("actual={actual:?}")));
        assert!(!message.contains("刷新会话"));
    }
}
