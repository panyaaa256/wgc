//! End-to-end tests of the `wgc` binary on small generated worlds.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output};

fn wgc(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wgc"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run wgc")
}

fn wgc_env(args: &[&str], env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_wgc"));
    cmd.args(args).env_remove("NO_COLOR");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd.output().expect("failed to run wgc")
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

/// NBT: root compound with Int and Long entries, or a nested `Data` compound.
fn nbt_entry(out: &mut Vec<u8>, tag: u8, name: &str) {
    out.push(tag);
    out.extend_from_slice(&(name.len() as u16).to_be_bytes());
    out.extend_from_slice(name.as_bytes());
}

fn chunk_nbt(inhabited: i64) -> Vec<u8> {
    let mut out = Vec::new();
    nbt_entry(&mut out, 10, "");
    nbt_entry(&mut out, 3, "DataVersion");
    out.extend_from_slice(&4325i32.to_be_bytes());
    nbt_entry(&mut out, 4, "InhabitedTime");
    out.extend_from_slice(&inhabited.to_be_bytes());
    out.push(0);
    out
}

fn level_dat() -> Vec<u8> {
    let mut out = Vec::new();
    nbt_entry(&mut out, 10, "");
    nbt_entry(&mut out, 10, "Data");
    nbt_entry(&mut out, 3, "DataVersion");
    out.extend_from_slice(&4325i32.to_be_bytes());
    out.push(0);
    out.push(0);
    gzip(&out)
}

/// A region file with chunk i at index i holding `times[i]`.
fn region(times: &[i64]) -> Vec<u8> {
    let mut out = vec![0u8; 8192];
    for (i, t) in times.iter().enumerate() {
        let data = zlib(&chunk_nbt(*t));
        let offset = out.len() / 4096;
        out.extend_from_slice(&(data.len() as u32 + 1).to_be_bytes());
        out.push(2);
        out.extend_from_slice(&data);
        out.resize(out.len().div_ceil(4096) * 4096, 0);
        let count = out.len() / 4096 - offset;
        out[i * 4..i * 4 + 4]
            .copy_from_slice(&(((offset as u32) << 8) | count as u32).to_be_bytes());
    }
    out
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn world() -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    write(&t.path().join("level.dat"), &level_dat());
    write(&t.path().join("session.lock"), "\u{2603}".as_bytes());
    write(&t.path().join("region/r.0.0.mca"), &region(&[0, 100, 0]));
    t
}

fn snapshot(dir: &Path) -> Vec<(String, u64, std::time::SystemTime)> {
    let mut out = Vec::new();
    for entry in walk(dir) {
        let m = std::fs::metadata(&entry).unwrap();
        out.push((entry.display().to_string(), m.len(), m.modified().unwrap()));
    }
    out.sort();
    out
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files.extend(walk(&p));
        } else {
            files.push(p);
        }
    }
    files
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

#[test]
fn scan_and_strict() {
    let w = world();
    let o = wgc(&["scan", s(w.path())]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stdout(&o).contains("minecraft:overworld"));
    // T-3 is unknown (no players): a warning, so --strict exits with 1.
    assert_eq!(
        wgc(&["scan", s(w.path()), "--strict"]).status.code(),
        Some(1)
    );
}

#[test]
fn json_goes_to_stdout_alone() {
    let w = world();
    let o = wgc(&["scan", s(w.path()), "--json"]);
    assert_eq!(o.status.code(), Some(0));
    let v: serde_json::Value = serde_json::from_slice(&o.stdout).expect("stdout is only JSON");
    assert_eq!(v["schema"], "wgc.scan/1");
    assert!(
        stderr(&o).contains("minecraft:overworld"),
        "summary goes to stderr"
    );
}

#[test]
fn estimate_writes_nothing() {
    let w = world();
    let before = snapshot(w.path());
    let o = wgc(&["gc", s(w.path())]);
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
    assert!(stdout(&o).contains("Nothing was written"));
    assert_eq!(snapshot(w.path()), before);
}

#[test]
fn gc_output_and_verify() {
    let w = world();
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");

    // Stopped by T-3 without --force: exit 3, nothing created.
    let r = wgc(&["gc", s(w.path()), "-o", s(&out)]);
    assert_eq!(r.status.code(), Some(3), "{}", stderr(&r));
    assert!(stderr(&r).contains("--force=T-3"));
    assert!(!out.exists());

    // Allowing only T-5 still stops on T-3.
    assert_eq!(
        wgc(&["gc", s(w.path()), "-o", s(&out), "--force=T-5"])
            .status
            .code(),
        Some(3)
    );

    let r = wgc(&["gc", s(w.path()), "-o", s(&out), "--force=T-3"]);
    assert_eq!(r.status.code(), Some(0), "{}", stderr(&r));
    assert!(stdout(&r).contains("Done:"));
    assert!(stdout(&r).contains("wgc verify"));
    assert!(out.join(".wgc/complete.json").is_file());

    let v = wgc(&["verify", s(&out)]);
    assert_eq!(v.status.code(), Some(0), "{}", stderr(&v));
    assert!(stdout(&v).contains("Ready to swap."));
    assert!(stdout(&v).contains("--force"), "forced output is flagged");

    let j = wgc(&["verify", s(&out), "--json"]);
    let json: serde_json::Value = serde_json::from_slice(&j.stdout).unwrap();
    assert_eq!(json["result"], "ready");

    std::fs::write(w.path().join("level.dat"), b"changed").unwrap();
    let v = wgc(&["verify", s(&out)]);
    assert_eq!(v.status.code(), Some(5));
    assert!(stderr(&v).contains("The world has changed"));
}

#[test]
fn layout_errors_exit_with_4() {
    let empty = tempfile::tempdir().unwrap();
    let o = wgc(&["scan", s(empty.path())]);
    assert_eq!(o.status.code(), Some(4));
    assert!(stderr(&o).contains("No Minecraft world"));

    // W-1: a vanilla server root.
    let root = tempfile::tempdir().unwrap();
    write(
        &root.path().join("server.properties"),
        b"level-name=world\n",
    );
    write(&root.path().join("world/level.dat"), &level_dat());
    write(&root.path().join("world/region/r.0.0.mca"), &region(&[1]));
    write(
        &root.path().join("world/DIM-1/region/r.0.0.mca"),
        &region(&[1]),
    );
    let o = wgc(&["gc", s(root.path())]);
    assert_eq!(o.status.code(), Some(4));
    assert!(stderr(&o).contains("Pass the world folder instead: wgc gc"));
}

#[test]
fn dimension_arguments() {
    let w = world();
    assert_eq!(
        wgc(&["gc", s(w.path()), "--dimension", "nether"])
            .status
            .code(),
        Some(2)
    );
    assert_eq!(
        wgc(&["gc", s(w.path()), "--dim", "the_end"]).status.code(),
        Some(2)
    );
    assert_eq!(
        wgc(&["gc", s(w.path()), "--dim", "overworld"])
            .status
            .code(),
        Some(0)
    );
}

#[test]
fn lists_are_counted_by_default_and_listed_with_verbose() {
    let w = world();
    write(&w.path().join("region/r.1.0.mca"), &[]);

    let o = wgc(&["scan", s(w.path())]);
    let text = stdout(&o);
    assert!(text.contains("Also found: 1 empty file"), "{text}");
    assert!(text.contains("--verbose"), "{text}");
    assert!(!text.contains("r.1.0.mca"), "{text}");

    let o = wgc(&["-v", "scan", s(w.path())]);
    let text = stdout(&o);
    assert!(text.contains("empty file region/r.1.0.mca"), "{text}");
    assert!(!text.contains("Also found"), "{text}");
    assert!(!text.contains("--verbose"), "{text}");
}

#[test]
fn no_color_disables_escapes() {
    let w = world();
    let o = wgc_env(&["scan", s(w.path())], &[("NO_COLOR", "1")]);
    assert!(!stdout(&o).contains('\x1b'));
    // Not a terminal: no colors even without NO_COLOR.
    let o = wgc_env(&["scan", s(w.path())], &[]);
    assert!(!stdout(&o).contains('\x1b'));
}

#[test]
fn partial_force_keeps_the_other_stops() {
    let w = world();
    // Unreadable forced chunks: T-5 is unknown, in addition to T-3.
    write(&w.path().join("data/chunks.dat"), b"broken");
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");
    let r = wgc(&["gc", s(w.path()), "-o", s(&out), "--force=T-3"]);
    assert_eq!(r.status.code(), Some(3), "{}", stderr(&r));
    assert!(stderr(&r).contains("T-5"));
    assert!(stderr(&r).contains("NOT protected"));
    assert!(!out.exists());
}

#[test]
fn forced_missing_lock_is_reported_as_missing() {
    let w = world();
    std::fs::remove_file(w.path().join("session.lock")).unwrap();
    let o = tempfile::tempdir().unwrap();
    let out = o.path().join("out");
    let r = wgc(&[
        "gc",
        s(w.path()),
        "-o",
        s(&out),
        "--force=T-3,L-1",
        "--json",
    ]);
    assert_eq!(r.status.code(), Some(0), "{}", stderr(&r));
    let v: serde_json::Value = serde_json::from_slice(&r.stdout).unwrap();
    assert_eq!(v["lock"], "missing");
    assert_eq!(v["forced"], serde_json::json!(["T-3", "L-1"]));
}

#[test]
fn scan_lock_warning_names_the_cause() {
    let w = world();
    std::fs::remove_file(w.path().join("session.lock")).unwrap();
    let r = wgc(&["scan", s(w.path())]);
    assert!(stderr(&r).contains("is missing, so a running game cannot be detected"));
    assert!(!stderr(&r).contains("is in use"));
}
