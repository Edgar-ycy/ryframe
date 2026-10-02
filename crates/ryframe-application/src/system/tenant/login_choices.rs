use chrono::{DateTime, Utc};
use ryframe_kernel::{AppError, AppResult};

use super::{TENANT_STATUS_ENABLED, TenantService};

/// 登录前可公开的租户信息，不包含配额、连接或套餐配置。
#[derive(Debug, PartialEq, Eq)]
pub struct LoginTenantChoice {
    pub tenant_id: String,
    pub name: String,
}

pub struct LoginTenantPage {
    pub items: Vec<LoginTenantChoice>,
    pub has_more: bool,
}

impl TenantService {
    pub async fn login_choices(
        &self,
        search: &str,
        page: usize,
        page_size: usize,
        only_tenant: Option<&str>,
    ) -> AppResult<LoginTenantPage> {
        if search.chars().count() > 100 || page == 0 || !(1..=50).contains(&page_size) {
            return Err(AppError::Validation(
                "租户名称最多 100 字，页码从 1 开始，每页最多 50 项".into(),
            ));
        }
        let offset = (page - 1)
            .checked_mul(page_size)
            .ok_or_else(|| AppError::Validation("租户选项页码超出范围".into()))?;
        let search = search.trim().to_lowercase();
        let now = Utc::now();
        let mut choices = self
            .persistence
            .list()
            .await?
            .into_iter()
            .filter(|tenant| login_available(&tenant.status, tenant.expire_at, now))
            .filter(|tenant| only_tenant.is_none_or(|id| id == tenant.tenant_id))
            .filter(|tenant| tenant.name.to_lowercase().contains(&search))
            .map(|tenant| LoginTenantChoice {
                tenant_id: tenant.tenant_id,
                name: tenant.name,
            })
            .collect::<Vec<_>>();
        choices.sort_by(|left, right| {
            (&left.name, &left.tenant_id).cmp(&(&right.name, &right.tenant_id))
        });
        let mut items = choices
            .into_iter()
            .skip(offset)
            .take(page_size + 1)
            .collect::<Vec<_>>();
        let has_more = items.len() > page_size;
        items.truncate(page_size);
        Ok(LoginTenantPage { items, has_more })
    }
}

fn login_available(status: &str, expires: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    status == TENANT_STATUS_ENABLED && expires.is_none_or(|value| value > now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_provisioning_and_expired_tenants_are_not_public() {
        let now = Utc::now();
        assert!(login_available("enabled", None, now));
        assert!(login_available(
            "enabled",
            Some(now + chrono::Duration::seconds(1)),
            now
        ));
        for status in ["disabled", "provisioning", "provisioning_failed"] {
            assert!(!login_available(status, None, now));
        }
        assert!(!login_available("enabled", Some(now), now));
    }
}
