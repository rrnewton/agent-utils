//! Regenerate the checked-in run-handle JSON Schema from its type.
//! Usage: generate_run_schemas <output-dir>
use std::path::PathBuf;

fn main() {
    let dir = std::env::args().nth(1).expect("output directory argument");
    let dir = PathBuf::from(dir);
    std::fs::create_dir_all(&dir).expect("create schema directory");
    for (stem, schema) in local_ci_hub::run_handle::generated_schemas() {
        let mut text = serde_json::to_string_pretty(&schema).expect("schema serializes");
        text.push('\n');
        std::fs::write(dir.join(format!("{stem}.schema.json")), text).expect("write schema");
    }
}
