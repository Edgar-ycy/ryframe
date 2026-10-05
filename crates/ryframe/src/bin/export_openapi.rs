use std::{env, error::Error, fs, io, io::Write, path::PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    let output_path = parse_output_path()?;
    let modules = ryframe::business::registered_business_modules()?;
    let document = ryframe_sdk::compose_openapi(ryframe_api::openapi::document(), &modules)?;
    let rendered = ryframe_api::openapi::render_openapi_json(&document)?;

    if let Some(path) = output_path {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, rendered)?;
    } else {
        io::stdout().write_all(rendered.as_bytes())?;
    }
    Ok(())
}

fn parse_output_path() -> Result<Option<PathBuf>, Box<dyn Error>> {
    let mut args = env::args_os().skip(1);
    let output_path = args.next().map(PathBuf::from);
    if args.next().is_some() {
        return Err("usage: export_openapi [output-path]".into());
    }
    Ok(output_path)
}
