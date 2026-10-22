//! The command line: arguments, exit codes and the commands that need no lab.

use std::{
    io::Write,
    process::{Command, Output, Stdio},
};

fn homie(args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_homie"))
        .args(args)
        .env_remove("HOMIE_CONFIG")
        .env("HOME", env!("CARGO_TARGET_TMPDIR"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn version_and_help() {
    for args in [&["version"][..], &["--version"], &["-V"]] {
        let o = homie(args, "");
        assert!(o.status.success());
        assert_eq!(out(&o), format!("homie {}\n", env!("CARGO_PKG_VERSION")));
    }
    let o = homie(&["--help"], "");
    assert!(o.status.success());
    assert!(out(&o).starts_with("usage: homie"));
}

#[test]
fn bad_arguments_exit_2() {
    let o = homie(&["frobnicate"], "");
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).starts_with("unknown command \"frobnicate\""));
    let o = homie(&["check", "extra"], "");
    assert_eq!(o.status.code(), Some(2));
    assert!(err(&o).starts_with("unexpected argument \"extra\""));
}

#[test]
fn hash_password_reads_stdin() {
    let o = homie(&["hash-password"], "short\n");
    assert_eq!(o.status.code(), Some(1));
    assert_eq!(err(&o), "error: use at least 12 characters\n");
    let o = homie(&["hash-password"], "a long enough password\n");
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).starts_with("$argon2id$"));
}

#[test]
fn check_and_serve_with_a_config() {
    let missing = "/nonexistent/homie.toml";
    for cmd in ["check", "serve"] {
        let o = homie(&[cmd, "--config", missing], "");
        assert_eq!(o.status.code(), Some(1));
        assert!(err(&o).contains("does not exist"), "{}", err(&o));
    }
    let empty = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("empty.toml");
    std::fs::write(&empty, "").unwrap();
    let o = homie(&["check", "-c", empty.to_str().unwrap()], "");
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("nothing configured"));
}
