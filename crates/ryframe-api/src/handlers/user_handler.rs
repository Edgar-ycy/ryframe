use serde_json;
use axum::{extract::{Path, Query, State}, routing::{get, post, put}, Extension, Json, Router};
use axum::extract::Multipart;
use ryframe_auth::jwt::Claims;
use ryframe_common::AppResult;
use ryframe_core::{PageQuery, Repository};
use ryframe_service::system::UserVo;
use validator::Validate;
use crate::dto::user_dto::{CreateUserDto, ResetPasswordDto, UpdateUserDto};
use crate::dto::user_import_dto::{UserImportData, UserExportData};

use super::auth_handler::AppState;

pub fn user_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(list).post(create))
        .route("/{id}", get(detail).put(update).delete(remove))
        .route("/{id}/password", put(reset_password))
        .route("/export", get(export_users))
        .route("/import", post(import_users))
        .route("/import-template", get(download_import_template))
        .with_state(state)
}

async fn list(
    State(state): State<AppState>,
    Extension(claims): Extension<Claims>,
    Query(query): Query<PageQuery>,
) -> AppResult<Json<ryframe_core::PageResult<UserVo>>> {
    // 构建数据权限上下文
    let user_id = claims.sub.parse::<i64>()
        .map_err(|_| ryframe_common::AppError::Authentication("令牌无效".into()))?;

    // 查当前用户信息和部门
    let user = state.user_service.user_repo.find_by_id(&state.db, user_id).await?
        .ok_or_else(|| ryframe_common::AppError::Authentication("用户不存在".into()))?;

    // 查角色
    let roles = state.user_service.role_repo.find_user_roles(&state.db, user_id).await?;

    // 构建数据权限上下文
    let scope_ctx = state.user_service
        .build_data_scope_context(&state.db, user_id, user.dept_id, &roles)
        .await?;

    state.user_service
        .find_by_page_with_data_scope(&state.db, query, &scope_ctx)
        .await
        .map(Json)
}

async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<UserVo>> {
    match state.user_service.find_by_id(&state.db, id).await? {
        Some(user) => Ok(Json(user)),
        None => Err(ryframe_common::AppError::NotFound("用户不存在".into())),
    }
}

async fn create(
    State(state): State<AppState>,
    Json(dto): Json<CreateUserDto>,
) -> AppResult<Json<UserVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.user_service.create(
        &state.db, &dto.username, &dto.password, &dto.nickname,
        dto.email.as_deref().unwrap_or(""),
        dto.phone.as_deref().unwrap_or(""),
        dto.dept_id, dto.role_ids,
    ).await.map(Json)
}

async fn update(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<UpdateUserDto>,
) -> AppResult<Json<UserVo>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.user_service.update(
        &state.db, id, &dto.nickname,
        dto.email.as_deref().unwrap_or(""),
        dto.phone.as_deref().unwrap_or(""),
        dto.dept_id, dto.status, dto.role_ids,
    ).await.map(Json)
}

async fn remove(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Json<serde_json::Value>> {
    state.user_service.delete(&state.db, id).await?;
    Ok(Json(serde_json::json!({"message": "删除成功"})))
}

async fn reset_password(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(dto): Json<ResetPasswordDto>,
) -> AppResult<Json<serde_json::Value>> {
    dto.validate().map_err(|e| ryframe_common::AppError::Validation(e.to_string()))?;
    state.user_service.reset_password(&state.db, id, &dto.password).await?;
    Ok(Json(serde_json::json!({"message": "密码重置成功"})))
}

/// 导出用户数据为 Excel
async fn export_users(
    State(state): State<AppState>,
    Query(_query): Query<PageQuery>,
) -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;
    
    
    // 查询所有用户（不分页）- 需要通过分页查询获取全部
    let query = PageQuery { page: 1, page_size: 10000 };
    let page_result = state.user_service.find_by_page(&state.db, query).await?;
    
    // 转换为导出数据
    let export_data: Vec<UserExportData> = page_result.records
        .into_iter()
        .map(|u| UserExportData {
            user_id: u.id,
            username: u.username,
            nickname: u.nickname,
            email: u.email,
            phone: u.phone,
            sex: "0".to_string(), // user 表没有 sex 字段，使用默认值
            dept_name: u.dept_name,
            status: u.status,
            remark: u.remark,
            created_at: u.created_at.to_rfc3339(),
        })
        .collect();
    
    // 生成 Excel
    let bytes = ExcelExporter::export_to_bytes(
        &export_data,
        "用户数据",
        UserExportData::excel_headers(),
    )?;
    
    // 返回文件
    let response = axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        .header("Content-Disposition", "attachment; filename=users.xlsx")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ryframe_common::AppError::Internal(format!("构建响应失败: {}", e)))?;
    
    Ok(response)
}

/// 从 Excel 导入用户数据
async fn import_users(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> AppResult<Json<serde_json::Value>> {
    use ryframe_common::utils::ExcelImporter;
    use validator::Validate;
    
    while let Some(field) = multipart.next_field().await.map_err(|e| {
        ryframe_common::AppError::Internal(format!("读取 multipart 失败: {}", e))
    })? {
        if field.name() == Some("file") {
            let bytes = field.bytes().await.map_err(|e| {
                ryframe_common::AppError::Internal(format!("读取文件失败: {}", e))
            })?;
            
            // 解析 Excel
            let import_data: Vec<UserImportData> = ExcelImporter::read_from_bytes(&bytes, None)?;
            
            // 验证并导入
            let mut success_count = 0;
            let mut fail_count = 0;
            let mut errors = Vec::new();
            
            for (index, data) in import_data.iter().enumerate() {
                // 验证数据
                if let Err(e) = data.validate() {
                    fail_count += 1;
                    errors.push(format!("第 {} 行数据验证失败: {}", index + 2, e));
                    continue;
                }
                
                // 创建用户
                match state.user_service.create(
                    &state.db,
                    &data.username,
                    "123456", // 默认密码
                    &data.nickname,
                    &data.email,
                    data.phone.as_deref().unwrap_or(""),
                    data.dept_id,
                    None,
                ).await {
                    Ok(_) => success_count += 1,
                    Err(e) => {
                        fail_count += 1;
                        errors.push(format!("第 {} 行导入失败: {}", index + 2, e));
                    }
                }
            }
            
            return Ok(Json(serde_json::json!({
                "code": 200,
                "message": "导入完成",
                "data": {
                    "success_count": success_count,
                    "fail_count": fail_count,
                    "errors": errors
                }
            })));
        }
    }
    
    Err(ryframe_common::AppError::Validation("未找到上传的文件".into()))
}

/// 下载导入模板
async fn download_import_template() -> AppResult<axum::response::Response> {
    use ryframe_common::utils::ExcelExporter;
    
    let bytes = ExcelExporter::export_template(
        "用户数据",
        UserImportData::excel_headers(),
    )?;
    
    let response = axum::response::Response::builder()
        .status(200)
        .header("Content-Type", "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet")
        .header("Content-Disposition", "attachment; filename=user_template.xlsx")
        .body(axum::body::Body::from(bytes))
        .map_err(|e| ryframe_common::AppError::Internal(format!("构建响应失败: {}", e)))?;
    
    Ok(response)
}
