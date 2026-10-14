//! Console-only MCP launcher. It runs Burrata's bundled Node runtime and bridge.
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

fn resources_from(executable: &std::path::Path) -> Result<PathBuf, String> {
    for ancestor in executable.ancestors() {
        if ancestor.join("mcp/server.cjs").is_file() {
            return Ok(ancestor.to_path_buf());
        }
    }
    Err("Burrata MCP resources are missing".into())
}

fn main() {
    let result = (|| -> Result<(), String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let resources = resources_from(&executable)?;
        let node_name = if cfg!(windows) { "node.exe" } else { "node" };
        let node = resources
            .join("tools")
            .join(burrata_lib::tools::target_triple())
            .join(node_name);
        let script = resources.join("mcp/server.cjs");
        if !node.is_file() || !script.is_file() {
            return Err("Burrata's bundled MCP runtime is incomplete".into());
        }
        let status = Command::new(node)
            .arg(script)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| e.to_string())?;
        std::process::exit(status.code().unwrap_or(1));
    })();
    if let Err(error) = result {
        eprintln!("Burrata MCP: {error}");
        std::process::exit(1);
    }
}
