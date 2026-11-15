//! Keep codec panics inside the decoder's ordinary failure/recovery boundary.
pub(super) fn run(task: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(task))
        .unwrap_or_else(|_| Err("Audio decoder stopped unexpectedly".into()))
}

#[cfg(test)]
mod tests {
    use super::run;

    #[test]
    fn codec_panics_become_failures_without_publishing_the_panic_payload() {
        let result = run(|| panic!("private-source-url-and-token"));
        assert_eq!(result.unwrap_err(), "Audio decoder stopped unexpectedly");
    }

    #[test]
    fn successful_and_failed_decodes_retain_their_result() {
        assert_eq!(run(|| Ok(())), Ok(()));
        assert_eq!(
            run(|| Err("decoder error".into())),
            Err("decoder error".into())
        );
    }
}
