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
        for relative in [
            "Resources",
            "resources",
            "Resources/resources",
            "resources/resources",
            "../lib/Burrata/resources",
            "../share/Burrata/resources",
            "../lib/burrata/resources",
            "../share/burrata/resources",
        ] {
            let resources = ancestor.join(relative);
            if resources.join("mcp/server.cjs").is_file() {
                return Ok(resources);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn fixture(root: &Path, relative: &str) -> PathBuf {
        let resources = root.join(relative);
        std::fs::create_dir_all(resources.join("mcp")).unwrap();
        std::fs::write(resources.join("mcp/server.cjs"), "fixture").unwrap();
        resources
    }

    #[test]
    fn console_mcp_resolves_extracted_linux_capitalized_resources() {
        let directory = tempfile::tempdir().unwrap();
        let resources = fixture(directory.path(), "usr/lib/Burrata/resources");
        let executable = directory.path().join("usr/bin/burrata-mcp");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        assert_eq!(
            resources_from(&executable).unwrap().canonicalize().unwrap(),
            resources.canonicalize().unwrap()
        );
    }

    #[test]
    fn console_mcp_keeps_resource_helper_macos_and_windows_layouts() {
        for (relative, executable) in [
            ("resources", "resources/tools/target/burrata-mcp"),
            (
                "Burrata.app/Contents/Resources",
                "Burrata.app/Contents/MacOS/burrata-mcp",
            ),
            (
                "Burrata.app/Contents/Resources/resources",
                "Burrata.app/Contents/MacOS/burrata-mcp",
            ),
            (
                "Program Files/Burrata/resources",
                "Program Files/Burrata/burrata-mcp.exe",
            ),
            (
                "Program Files/Burrata/resources/resources",
                "Program Files/Burrata/burrata-mcp.exe",
            ),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let resources = fixture(directory.path(), relative);
            let executable = directory.path().join(executable);
            std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
            assert_eq!(
                resources_from(&executable).unwrap().canonicalize().unwrap(),
                resources.canonicalize().unwrap()
            );
        }
    }

    #[test]
    fn console_mcp_rejects_missing_packaged_bridge() {
        let directory = tempfile::tempdir().unwrap();
        assert!(resources_from(&directory.path().join("usr/bin/burrata-mcp")).is_err());
    }
}
