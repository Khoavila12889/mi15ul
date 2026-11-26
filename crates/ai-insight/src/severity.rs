/// Returns true for fatal, ANR, panic, or native-crash shaped logcat lines.
///
/// Matches Fatal level; tags `AndroidRuntime`, `DEBUG`, or `libc`; or message substrings
/// `FATAL EXCEPTION`, `ANR in`, `Fatal signal`, `tombstone`, `CheckJNI`, or `panic`
/// (ASCII case-insensitive).
pub fn is_high_severity(level: char, tag: &str, message: &str) -> bool {
    if level == 'F' {
        return true;
    }
    if matches!(tag, "AndroidRuntime" | "DEBUG" | "libc") {
        return true;
    }
    let message = message.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "fatal exception",
        "anr in",
        "fatal signal",
        "tombstone",
        "checkjni",
        "panic",
    ];
    NEEDLES.iter().any(|needle| message.contains(needle))
}

/// A narrowly identified vendor success message logged at Error priority.
/// Keep unknown messages and all fatal/crash evidence visible.
pub fn is_known_diagnostic_noise(level: char, tag: &str, message: &str) -> bool {
    level == 'E'
        && tag.trim() == "MI-SF"
        && message.trim() == "surfaceflinger setAffinity success"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_filter_preserves_failures_and_fatal_messages() {
        assert!(is_known_diagnostic_noise('E', "MI-SF   ", "surfaceflinger setAffinity success"));
        assert!(!is_known_diagnostic_noise('F', "MI-SF", "surfaceflinger setAffinity success"));
        assert!(!is_known_diagnostic_noise('E', "MI-SF", "surfaceflinger setAffinity failed"));
        assert!(!is_known_diagnostic_noise('E', "MI-SF", "surfaceflinger setAffinity success; FATAL EXCEPTION"));
        assert!(!is_known_diagnostic_noise('E', "HwcComposer", "presentOrValidateDisplay 0 presentFence:-1"));
    }

    #[test]
    fn matches_fatal_level() {
        assert!(is_high_severity('F', "Tag", "something died"));
    }

    #[test]
    fn matches_android_runtime_fatal_exception() {
        assert!(is_high_severity(
            'E',
            "AndroidRuntime",
            "FATAL EXCEPTION: main"
        ));
    }

    #[test]
    fn matches_anr_in_activity_manager() {
        assert!(is_high_severity(
            'E',
            "ActivityManager",
            "ANR in com.example.myapp (com.example.myapp/.MainActivity)"
        ));
    }

    #[test]
    fn rejects_ordinary_error() {
        assert!(!is_high_severity('E', "OkHttp", "unexpected end of stream"));
    }
}
