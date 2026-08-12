/// `import` command — import CSV/TSV data into an Excel sheet.
use clap::Args;
use handler_common::HandlerError;

#[derive(Args)]
pub struct ImportCommand {
    /// Target Excel file (.xlsx)
    pub file: String,

    /// Sheet path (e.g. /Sheet1)
    pub parent_path: String,

    /// Source CSV/TSV file to import (C#-compatible positional form).
    pub source_file: Option<String>,

    /// Source CSV/TSV file to import
    #[arg(long, short)]
    pub file_source: Option<String>,

    /// Read CSV/TSV data from stdin
    #[arg(long)]
    pub stdin: bool,

    /// Data format: csv or tsv (default: inferred from file extension, or csv)
    #[arg(long)]
    pub format: Option<String>,

    /// First row is header: set AutoFilter and freeze pane
    #[arg(long)]
    pub header: bool,

    /// Starting cell (default: A1)
    #[arg(long, default_value = "A1")]
    pub start_cell: String,
}

pub fn handle_import(
    cmd: ImportCommand,
    _format: handler_common::OutputFormat,
) -> Result<String, HandlerError> {
    // Only xlsx supported
    if !cmd.file.to_lowercase().ends_with(".xlsx") {
        return Err(HandlerError::InvalidArgument(
            "Import is only supported for .xlsx files".to_string(),
        ));
    }

    // Read CSV content
    if cmd.source_file.is_some() && cmd.file_source.is_some() {
        return Err(HandlerError::InvalidArgument(
            "Use either positional source-file or --file, not both.".to_string(),
        ));
    }
    let source_path = cmd.file_source.as_deref().or(cmd.source_file.as_deref());
    let csv_content = if cmd.stdin {
        use std::io::Read;
        let mut content = String::new();
        std::io::stdin()
            .read_to_string(&mut content)
            .map_err(|e| HandlerError::OperationFailed(format!("Failed to read stdin: {}", e)))?;
        content
            .strip_prefix('\u{feff}')
            .unwrap_or(&content)
            .to_string()
    } else if let Some(source_path) = source_path {
        std::fs::read_to_string(source_path).map_err(|e| {
            HandlerError::OperationFailed(format!(
                "Failed to read source file '{}': {}",
                source_path, e
            ))
        })?
    } else {
        return Err(HandlerError::InvalidArgument(
            "Either --file or --stdin must be specified".to_string(),
        ));
    };

    // Determine delimiter
    let delimiter = if let Some(fmt) = &cmd.format {
        match fmt.to_lowercase().as_str() {
            "tsv" => '\t',
            "csv" => ',',
            other => {
                return Err(HandlerError::InvalidArgument(format!(
                    "Unknown format: {}. Use 'csv' or 'tsv'",
                    other
                )))
            }
        }
    } else if let Some(source_path) = source_path {
        let ext = source_path.rsplit('.').next().unwrap_or("").to_lowercase();
        if ext == "tsv" || ext == "tab" {
            '\t'
        } else {
            ','
        }
    } else {
        ','
    };

    let handler = crate::open_handler(&cmd.file, true)?;
    let result = handler.import_csv(
        &cmd.parent_path,
        &csv_content,
        delimiter,
        cmd.header,
        &cmd.start_cell,
    )?;
    handler.save()?;

    Ok(result)
}
