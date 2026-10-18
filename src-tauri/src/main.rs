#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
fn main() {
    if std::env::args().any(|arg| arg == "--mcp") {
        if let Err(error) = burrata_lib::install_integration::mcp_launcher_main() {
            eprintln!("Burrata MCP: {error}");
            std::process::exit(1);
        }
        return;
    }
    if std::env::args().any(|arg| arg == "--native-messaging" || arg == "--browser-native-host") {
        if let Err(error) = burrata_lib::browser_bridge::native_host_main() {
            eprintln!("Burrata native messaging: {error}");
            std::process::exit(1);
        }
        return;
    }
    burrata_lib::run();
}
