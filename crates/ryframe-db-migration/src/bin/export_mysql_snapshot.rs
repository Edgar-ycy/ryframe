use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: export_mysql_snapshot <output.sql>")?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, ryframe_db_migration::mysql_snapshot_sql())?;
    println!("wrote {}", output.display());
    Ok(())
}
