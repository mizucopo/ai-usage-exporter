use std::{ffi::OsStr, process::Command};

fn exporter() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ai-usage-exporter"));
    // Invalid settings prevent a regression from starting a long-running server.
    // CLI handling must finish before these settings are read.
    command.env("AI_USAGE_EXPORTER_PORT", "invalid");
    command
}

fn assert_invalid_arguments(arguments: &[impl AsRef<OsStr>]) {
    let output = exporter().args(arguments).output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Invalid command-line arguments."));
    assert!(stderr.contains("Usage: ai-usage-exporter [--help | --version]"));
    assert!(stderr.contains("environment variables; see --help."));
    assert!(!stderr.contains("Cannot start exporter"));
}

#[test]
fn help_and_version_aliases_succeed_without_reading_settings() {
    for argument in ["--help", "-h", "--version", "-V"] {
        let output = exporter().arg(argument).output().unwrap();
        assert!(output.status.success(), "{argument}");
        assert!(output.stderr.is_empty());
        let stdout = String::from_utf8(output.stdout).unwrap();
        let version = format!("ai-usage-exporter {}", env!("CARGO_PKG_VERSION"));
        assert!(stdout.starts_with(&version));
        if matches!(argument, "--help" | "-h") {
            assert!(stdout.contains("Usage: ai-usage-exporter"));
            assert!(stdout.contains("AI_USAGE_EXPORTER_PORT"));
        } else {
            assert_eq!(stdout.trim_end(), version);
        }
    }
}

#[test]
fn unknown_options_and_positional_arguments_are_rejected() {
    for arguments in [
        vec!["--unknown"],
        vec!["-v"],
        vec!["--port", "9999"],
        vec!["--port=9999"],
        vec!["unexpected"],
        vec![""],
        vec!["--"],
        vec!["--", "--help"],
        vec!["--unknown", "--help"],
    ] {
        assert_invalid_arguments(&arguments);
    }
}

#[test]
fn help_and_version_do_not_hide_trailing_arguments() {
    for argument in ["--help", "-h", "--version", "-V"] {
        for extra in ["--unknown", "unexpected", "--help", "--version", ""] {
            assert_invalid_arguments(&[argument, extra]);
        }
    }
}

#[test]
fn no_arguments_continue_to_environment_configuration() {
    let output = exporter().output().unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("Cannot start exporter: AI_USAGE_EXPORTER_PORT"));
    assert!(!stderr.contains("Invalid command-line arguments"));
}

#[cfg(unix)]
#[test]
fn non_unicode_arguments_are_rejected_without_panicking() {
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};

    let invalid = OsString::from_vec(vec![0xff]);
    assert_invalid_arguments(std::slice::from_ref(&invalid));
    assert_invalid_arguments(&[OsString::from("--help"), invalid]);
}

#[cfg(windows)]
#[test]
fn non_unicode_arguments_are_rejected_without_panicking() {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};

    let invalid = OsString::from_wide(&[0xd800]);
    assert_invalid_arguments(std::slice::from_ref(&invalid));
    assert_invalid_arguments(&[OsString::from("--help"), invalid]);
}
