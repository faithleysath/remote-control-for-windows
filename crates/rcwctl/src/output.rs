use std::{fs, path::Path};

use anyhow::{bail, Result};
use serde_json::Value;

pub(crate) fn write_output_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

pub(crate) fn write_output_file_checked(path: &Path, bytes: &[u8], overwrite: bool) -> Result<()> {
    if !overwrite && path.exists() {
        bail!(
            "refusing to overwrite existing local file {}; set overwrite=true to replace it",
            path.display()
        );
    }
    write_output_file(path, bytes)
}

pub(crate) fn print_json(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
