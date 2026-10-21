use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_mperf"))
}
fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}.xml", env!("CARGO_MANIFEST_DIR"))
}
fn success(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn help_and_command_specific_errors() {
    for command in ["record", "report", "diff", "list"] {
        for help in ["help", "--help"] {
            let text = success(&binary().args([command, help]).output().unwrap());
            assert!(text.contains(&format!("mperf {command}")));
        }
        let output = binary().args([command, "--bogus"]).output().unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("mperf {command}")));
    }
    assert!(success(&binary().arg("help").output().unwrap()).contains("diff"));
    for args in [
        vec!["record"],
        vec!["record", "-p", "0"],
        vec!["record", "-p", "12", "echo"],
        vec!["record", "-a", "-p", "12"],
        vec!["record", "-F", "99", "echo"],
        vec!["record", "--time-limit", "NaNs", "echo"],
        vec!["diff", "only-one.trace"],
        vec!["report", "--percent-limit", "NaN"],
    ] {
        let output = binary().args(&args).output().unwrap();
        assert!(!output.status.success(), "accepted {args:?}");
    }
}

#[test]
fn dry_run_is_non_mutating_and_preserves_command_options() {
    let dir = tempfile::tempdir().unwrap();
    let output = binary()
        .current_dir(dir.path())
        .args([
            "record",
            "--dry-run",
            "-g",
            "echo",
            "--help",
            "a b",
            "$(danger)",
        ])
        .output()
        .unwrap();
    let text = success(&output);
    let words = shlex::split(text.trim()).unwrap();
    assert_eq!(
        &words[words.len() - 4..],
        ["echo", "--help", "a b", "$(danger)"]
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    let text = success(
        &binary()
            .args(["record", "--dry-run", "-g", "-p", "123"])
            .output()
            .unwrap(),
    );
    assert!(text.contains("--attach 123"));
    assert!(!text.contains("--launch"));
}

#[test]
fn reports_and_diffs_exported_xml_without_terminal_sequences() {
    let output = binary()
        .args(["report", "-i", &fixture("before"), "--stdio"])
        .output()
        .unwrap();
    let text = success(&output);
    assert!(text.contains("50.00") && text.contains("25.00") && text.contains("hot & busy"));
    assert!(!text.contains('\x1b'));
    let output = binary()
        .args(["diff", &fixture("before"), &fixture("after"), "--stdio"])
        .output()
        .unwrap();
    let text = success(&output);
    assert!(text.contains("+50.00") && text.contains("-25.00"));
    assert!(text.contains("new work") && text.contains("cold"));
    let text = success(
        &binary()
            .args([
                "report",
                "-i",
                &fixture("before"),
                "--symbol",
                "does-not-exist",
            ])
            .output()
            .unwrap(),
    );
    assert!(text.contains("No symbols match"));
    assert!(
        !binary()
            .args(["report", "-i", &fixture("before"), "--tui"])
            .output()
            .unwrap()
            .status
            .success()
    );
}

#[cfg(unix)]
fn mock_xcrun(directory: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let script = directory.join("xcrun");
    fs::write(&script, include_str!("fixtures/xcrun.sh")).unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(unix)]
#[test]
fn record_report_diff_round_trip_with_mock_xctrace_and_failed_capture() {
    let dir = tempfile::tempdir().unwrap();
    mock_xcrun(dir.path());
    let command = || {
        let mut command = binary();
        let mut paths = vec![dir.path().to_path_buf()];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        command
            .current_dir(dir.path())
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("MPERF_QUERY_LOG", dir.path().join("query"));
        command
    };
    success(
        &command()
            .env("MPERF_FIXTURE", fixture("before"))
            .args(["record", "-g", "-o", "custom.trace", "echo", "hello"])
            .output()
            .unwrap(),
    );
    assert!(dir.path().join("custom.trace/samples.xml").exists());
    let latest = fs::read_to_string(dir.path().join(".mperf-latest")).unwrap();
    assert!(latest.ends_with("custom.trace"));
    let text = success(&command().args(["report", "--stdio"]).output().unwrap());
    assert!(text.contains("hot & busy"));
    assert!(
        fs::read_to_string(dir.path().join("query"))
            .unwrap()
            .contains("@schema='time-profile'")
    );
    success(
        &command()
            .env("MPERF_FIXTURE", fixture("after"))
            .args(["record", "-g", "-p", "123", "-o", "custom.trace"])
            .output()
            .unwrap(),
    );
    assert!(dir.path().join("custom.trace.old/samples.xml").exists());
    let text = success(&command().args(["diff", "--stdio"]).output().unwrap());
    assert!(text.contains("+50.00"));
    let old = fs::read(dir.path().join("custom.trace.old/samples.xml")).unwrap();
    let new = fs::read(dir.path().join("custom.trace/samples.xml")).unwrap();
    let output = command()
        .env("MPERF_FIXTURE", fixture("before"))
        .env("MPERF_FAIL", "1")
        .args(["record", "-p", "123", "-o", "custom.trace"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Partial data"));
    assert_eq!(
        old,
        fs::read(dir.path().join("custom.trace.old/samples.xml")).unwrap()
    );
    assert_eq!(
        new,
        fs::read(dir.path().join("custom.trace/samples.xml")).unwrap()
    );
    assert!(!dir.path().join("custom.trace.lock").exists());
}

#[cfg(unix)]
#[test]
fn interrupting_the_wrapper_waits_for_xctrace_to_save() {
    use std::{
        process::Stdio,
        time::{Duration, Instant},
    };
    let dir = tempfile::tempdir().unwrap();
    mock_xcrun(dir.path());
    let paths = std::iter::once(dir.path().to_path_buf())
        .chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ))
        .collect::<Vec<_>>();
    let ready = dir.path().join("ready");
    let mut child = binary()
        .current_dir(dir.path())
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("MPERF_FIXTURE", fixture("before"))
        .env("MPERF_WAIT", "1")
        .env("MPERF_READY", &ready)
        .args(["record", "-g", "-p", "123"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() {
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("recorder never became ready");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        Command::new("/bin/kill")
            .args(["-INT", &child.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("recording did not stop after SIGINT");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(dir.path().join("mperf.trace/samples.xml").exists());
    assert!(dir.path().join(".mperf-latest").exists());
}
