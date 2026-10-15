use ryframe_kernel::AppResult;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

use super::{AppliedMigration, database_error};

pub(in crate::migration) struct LedgerSnapshot {
    pub exists: bool,
    pub rows: Vec<LedgerSnapshotRow>,
}

#[derive(serde::Serialize)]
pub(in crate::migration) struct LedgerSnapshotRow {
    pub record: AppliedMigration,
    pub applied_epoch: String,
}

pub(in crate::migration) async fn load_snapshot<C: ConnectionTrait>(
    database: &C,
) -> AppResult<LedgerSnapshot> {
    if !super::exists(database).await? {
        return Ok(LedgerSnapshot {
            exists: false,
            rows: Vec::new(),
        });
    }
    let rows = database.query_all_raw(Statement::from_string(DbBackend::MySql,
        format!("SELECT scope,module_name,migration_id,checksum,before_fingerprint,after_fingerprint,origin,tables_json,adoption_hash,CAST(UNIX_TIMESTAMP(applied_at) AS CHAR) AS applied_epoch FROM {} ORDER BY scope,module_name,migration_id", super::NAME)))
        .await.map_err(super::ledger_read_error)?;
    let rows = rows
        .into_iter()
        .map(|row| {
            let applied_epoch = row.try_get("", "applied_epoch").map_err(database_error)?;
            Ok(LedgerSnapshotRow {
                record: AppliedMigration::from_row(row)?,
                applied_epoch,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;
    Ok(LedgerSnapshot { exists: true, rows })
}
