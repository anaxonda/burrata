#![cfg_attr(not(windows), allow(dead_code))]

fn main() {
    if let Err(error) = burrata_lib::browser_bridge::native_host_main() {
        // Native messaging stdout is reserved for framed protocol messages.
        eprintln!("Burrata browser host: {error}");
        std::process::exit(1);
    }
}
