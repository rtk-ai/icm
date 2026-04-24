#![cfg(target_os = "windows")]

use std::process::Command;

#[test]
fn debug_binary_handles_help_without_stack_overflow() {
    let binary = env!("CARGO_BIN_EXE_icm");
    let output = Command::new(binary)
        .arg("--help")
        .output()
        .expect("failed to launch icm --help");

    assert!(
        output.status.success(),
        "expected icm --help to succeed, status={:?}, stderr={} stdout={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
}