//! Standalone std-only fixture: compile with rustc, never run a real Codex CLI.

use std::{
    io::{BufRead, Write},
    path::PathBuf,
    time::Duration,
};

fn main() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("signal") {
        #[cfg(windows)]
        {
            signal(arguments[1].parse().unwrap());
            return;
        }
        #[cfg(not(windows))]
        panic!("console signals are only used on Windows");
    }
    #[cfg(windows)]
    ignore_console_signals();

    let (mode, pid_file) = if arguments.first().map(String::as_str) == Some("app-server") {
        assert_eq!(
            arguments,
            [
                "app-server",
                "--stdio",
                "-c",
                "cli_auth_credentials_store=\"file\""
            ]
        );
        assert_eq!(
            std::env::var_os("CODEX_HOME"),
            std::env::var_os("AI_USAGE_EXPORTER_MOCK_CODEX_HOME")
        );
        (
            std::env::var("AI_USAGE_EXPORTER_MOCK_MODE").unwrap(),
            PathBuf::from(std::env::var_os("AI_USAGE_EXPORTER_MOCK_PID_FILE").unwrap()),
        )
    } else {
        (arguments[0].clone(), PathBuf::from(&arguments[1]))
    };
    if let Some(path) = std::env::var_os("AI_USAGE_EXPORTER_MOCK_CALLS_FILE") {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap()
            .write_all(b"x")
            .unwrap();
    }
    std::fs::write(pid_file, std::process::id().to_string()).unwrap();
    if mode == "eof" {
        std::process::exit(7);
    }
    let input = std::io::stdin();
    let mut input = input.lock().lines();
    let initialize = input.next().unwrap().unwrap();
    assert!(initialize.contains("\"method\":\"initialize\""));
    assert!(initialize.contains("\"name\":\"ai_usage_exporter\""));
    let mut output = std::io::stdout().lock();
    if mode == "invalid" {
        writeln!(output, "secret-invalid-response").unwrap();
    } else if mode == "oversize" {
        output.write_all(&vec![b'x'; 1024 * 1024 + 65536]).unwrap();
    } else {
        if mode == "slow" {
            std::thread::sleep(Duration::from_millis(150));
        }
        writeln!(output, "{{\"id\":1,\"result\":{{}}}}").unwrap();
        output.flush().unwrap();
        assert_eq!(
            input.next().unwrap().unwrap(),
            "{\"method\":\"initialized\"}"
        );
        assert!(
            input
                .next()
                .unwrap()
                .unwrap()
                .contains("\"method\":\"account/rateLimits/read\"")
        );
        if let Some(path) = std::env::var_os("AI_USAGE_EXPORTER_MOCK_READY_FILE") {
            std::fs::write(path, b"ready").unwrap();
        }
        match mode.as_str() {
            "success" | "slow" => {
                if mode == "slow" {
                    std::thread::sleep(Duration::from_millis(150));
                }
                writeln!(output, "{{\"method\":\"notice\"}}").unwrap();
                writeln!(output, "{{\"id\":999,\"result\":{{}}}}").unwrap();
                writeln!(output, "{{\"id\":2,\"result\":{{\"rateLimits\":{{\"primary\":{{\"usedPercent\":25,\"windowDurationMins\":300,\"resetsAt\":4102444800}}}}}}}}").unwrap();
            }
            "error" => writeln!(
                output,
                "{{\"id\":2,\"error\":{{\"message\":\"secret-token\"}}}}"
            )
            .unwrap(),
            "timeout" => {}
            _ => panic!("unknown mock mode"),
        }
    }
    output.flush().unwrap();
    // Stay alive so tests prove that completion, errors and cancellation kill
    // the process, rather than relying on the mock to terminate itself.
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
    fn FreeConsole() -> i32;
    fn AttachConsole(process_id: u32) -> i32;
    fn GenerateConsoleCtrlEvent(event: u32, process_group: u32) -> i32;
}

#[cfg(windows)]
unsafe extern "system" fn console_handler(_: u32) -> i32 {
    1
}

#[cfg(windows)]
fn ignore_console_signals() {
    // SAFETY: the static handler accepts every control event and owns no data.
    assert_ne!(
        unsafe { SetConsoleCtrlHandler(Some(console_handler), 1) },
        0
    );
}

#[cfg(windows)]
fn signal(process_id: u32) {
    // SAFETY: these calls affect only this helper's console attachment and the
    // explicitly supplied exporter process group created by the test.
    unsafe {
        FreeConsole();
        assert_ne!(AttachConsole(process_id), 0);
        ignore_console_signals();
        assert_ne!(GenerateConsoleCtrlEvent(1, process_id), 0);
    }
}
