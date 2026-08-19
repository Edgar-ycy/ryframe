use crate::TenantDataError;

/// 一次 Session 所需的租户数据访问类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDataAccess {
    Read,
    Write,
}

/// 控制面可持久化的租户数据状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TenantDataState {
    Provisioning,
    Active,
    Maintenance,
    Failed,
}

impl TenantDataState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::Active => "active",
            Self::Maintenance => "maintenance",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "provisioning" => Some(Self::Provisioning),
            "active" => Some(Self::Active),
            "maintenance" => Some(Self::Maintenance),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// 仅由控制库提供的强一致租户运行时快照。
///
/// 认证和系统管理可读取该快照而不连接租户业务数据目标。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantRuntimeSnapshot {
    tenant_id: String,
    authorization_epoch: u64,
    runtime_epoch: u64,
    placement_generation: i64,
    business_data_state: TenantDataState,
}

impl TenantRuntimeSnapshot {
    pub(crate) fn new(
        tenant_id: String,
        authorization_epoch: u64,
        runtime_epoch: u64,
        placement_generation: i64,
        business_data_state: TenantDataState,
    ) -> Self {
        Self {
            tenant_id,
            authorization_epoch,
            runtime_epoch,
            placement_generation,
            business_data_state,
        }
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub const fn runtime_epoch(&self) -> u64 {
        self.runtime_epoch
    }

    pub const fn authorization_epoch(&self) -> u64 {
        self.authorization_epoch
    }

    pub const fn placement_generation(&self) -> i64 {
        self.placement_generation
    }

    pub const fn business_data_state(&self) -> TenantDataState {
        self.business_data_state
    }
}

/// 控制库强一致读取到的租户数据 placement。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TenantDataPlacement {
    tenant_id: String,
    target_key: String,
    generation: i64,
    switch_token: String,
    state: TenantDataState,
}

impl TenantDataPlacement {
    pub fn new(
        tenant_id: impl Into<String>,
        target_key: impl Into<String>,
        generation: i64,
        switch_token: impl Into<String>,
        state: TenantDataState,
    ) -> Result<Self, TenantDataError> {
        let tenant_id = tenant_id.into();
        ryframe_adapters::validate_tenant_identifier(&tenant_id)
            .map_err(|error| TenantDataError::InvalidTenantId(error.message().into()))?;
        let target_key = target_key.into();
        let switch_token = switch_token.into();
        if target_key.trim().is_empty()
            || generation <= 0
            || switch_token.trim().is_empty()
            || switch_token.len() > 64
        {
            return Err(TenantDataError::InvalidPlacement {
                tenant_id,
                reason: "target_key、placement_generation 或 switch_token 无效".into(),
            });
        }
        Ok(Self {
            tenant_id,
            target_key,
            generation,
            switch_token,
            state,
        })
    }

    pub fn tenant_id(&self) -> &str {
        &self.tenant_id
    }

    pub fn target_key(&self) -> &str {
        &self.target_key
    }

    pub const fn generation(&self) -> i64 {
        self.generation
    }

    pub fn switch_token(&self) -> &str {
        &self.switch_token
    }

    pub const fn state(&self) -> TenantDataState {
        self.state
    }
}
