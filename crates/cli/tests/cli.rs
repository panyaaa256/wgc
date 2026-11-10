use std::process::{Command, Output};

fn wgc(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wgc"))
        .args(args)
        .output()
        .expect("failed to run wgc")
}

#[test]
fn help_and_version_succeed() {
    for args in [
        &["--help"][..],
        &["--version"],
        &["scan", "--help"],
        &["verify", "--help"],
    ] {
        let out = wgc(args);
        assert!(out.status.success(), "{args:?} failed");
    }
}

#[test]
fn gc_help_says_nothing_is_written_without_out() {
    let out = wgc(&["gc", "--help"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("Without --out, only an estimate is printed and nothing is written"));
    assert!(text.contains("wgc gc ./world -o ./world-gc"));
}

#[test]
fn usage_errors_exit_with_2() {
    assert_eq!(wgc(&[]).status.code(), Some(2));
    assert_eq!(wgc(&["gc", "w", "--force=T-9"]).status.code(), Some(2));
    assert_eq!(wgc(&["-q", "-v", "scan", "w"]).status.code(), Some(2));
    assert_eq!(
        wgc(&["scan", "w", "--quiet", "--verbose"]).status.code(),
        Some(2)
    );
}
