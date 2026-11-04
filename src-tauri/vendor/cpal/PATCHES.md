# Local patches

This directory vendors CPAL 0.17.3 from the Cargo registry source package. The
upstream package license is preserved in `LICENSE`; `Cargo.toml` is the
registry-normalized manifest, and `Cargo.toml.orig` is included for reference.

## macOS default-output disconnect callback

In `src/host/coreaudio/macos/device.rs`, the CoreAudio output stream now forwards
`DeviceNotAvailable` from the existing disconnect manager for default-output
streams. CPAL already pauses the stream on the device-is-alive notification; the
default-output path previously replaced its error callback with a no-op. The
patch uses the callback's existing shared `Arc<Mutex<...>>` path and leaves
`IOType::DefaultOutput`, stream configuration, and automatic route behavior
unchanged. It does not backport CPAL 0.18's dedicated default-route monitor.

`src/host/coreaudio/macos/mod.rs` extracts the existing pause-then-notify sequence
and adds the no-hardware unit test
`host::coreaudio::macos::disconnect_tests::disconnect_pauses_before_default_output_error_callback`.
The focused macOS CI step runs that test. It verifies injected event ordering and
callback delivery; it does not qualify a physical unplug or claim successful
automatic rerouting.
