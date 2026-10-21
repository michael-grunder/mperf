use crate::cli::Record;
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub fn old_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".old");
    name.into()
}

pub fn latest() -> Result<PathBuf> {
    match fs::read_to_string(".mperf-latest") {
        Ok(path) => Ok(PathBuf::from(path)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok("mperf.trace".into()),
        Err(e) => Err(e).context("reading .mperf-latest; use -i to select a trace explicitly"),
    }
}

pub fn arguments(args: &Record, output: &Path) -> Vec<OsString> {
    let mut result: Vec<OsString> = vec![
        "xctrace".into(),
        "record".into(),
        "--template".into(),
        args.template.clone().into(),
        "--output".into(),
        output.as_os_str().to_owned(),
    ];
    for (flag, value) in [
        ("--device", &args.device),
        ("--time-limit", &args.time_limit),
        ("--window", &args.window),
        ("--run-name", &args.run_name),
    ] {
        if let Some(value) = value {
            result.extend([flag.into(), value.into()]);
        }
    }
    for instrument in &args.instrument {
        result.extend(["--instrument".into(), instrument.into()]);
    }
    if args.no_prompt {
        result.push("--no-prompt".into());
    }
    if let Some(pid) = args.pid {
        result.extend(["--attach".into(), pid.to_string().into()]);
    } else if args.all_cpus {
        result.push("--all-processes".into());
    } else {
        result.extend([
            "--target-stdout".into(),
            "-".into(),
            "--launch".into(),
            "--".into(),
        ]);
        result.extend(args.command.iter().cloned());
    }
    result
}

pub fn shell_command(args: &[OsString]) -> Result<String> {
    let words = std::iter::once(Ok("xcrun"))
        .chain(args.iter().map(|a| {
            a.to_str()
                .context("command display requires UTF-8 arguments")
        }))
        .collect::<Result<Vec<_>>>()?;
    shlex::try_join(words).context("command arguments cannot contain NUL bytes")
}

struct Lock(PathBuf);
impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn record(args: &Record) -> Result<()> {
    ensure!(
        args.output.extension().is_some_and(|ext| ext == "trace"),
        "output must end in .trace, e.g. -o session.trace; see 'mperf record help'"
    );
    let _ = shell_command(&arguments(args, &args.output))?;
    if args.call_graph && args.template != "Time Profiler" {
        eprintln!(
            "mperf: -g cannot configure a custom template; ensure '{}' records CPU call stacks",
            args.template
        );
    }
    if args.dry_run {
        println!("{}", shell_command(&arguments(args, &args.output))?);
        return Ok(());
    }
    crate::trace::check_available()?;
    let output = std::path::absolute(&args.output)?;
    ensure!(
        !output.is_symlink(),
        "refusing to replace a symlink output: {}",
        output.display()
    );
    let parent = output.parent().context("output has no parent directory")?;
    let mut lock_path = output.as_os_str().to_owned();
    lock_path.push(".lock");
    let lock_path = PathBuf::from(lock_path);
    let mut lock_file = fs::OpenOptions::new().write(true).create_new(true).open(&lock_path)
        .with_context(|| format!("cannot lock {}; another recording may be active. Remove a stale lock only after checking its PID", lock_path.display()))?;
    let _lock = Lock(lock_path);
    writeln!(lock_file, "{}", std::process::id())?;
    let staging = tempfile::Builder::new()
        .prefix(".mperf-record-")
        .tempdir_in(parent)?;
    let staged_output = staging.path().join("recording.trace");
    let command_args = arguments(args, &staged_output);
    eprintln!("{}", shell_command(&command_args)?);
    eprintln!(
        "mperf: recording for {}; Ctrl-C stops and saves",
        output.display()
    );
    let status = execute_recording(&command_args);
    // Keep partial data even if xctrace or publication fails.
    let staging_path = staging.keep();
    let status = status.with_context(|| {
        format!(
            "waiting for xctrace; partial data: {}",
            staging_path.display()
        )
    })?;
    if !status.success() {
        bail!(
            "xctrace exited with {status}; previous recording kept. Partial data: {}. Check Xcode selection, Developer Tools permissions, and target attach permissions",
            staged_output.display()
        );
    }
    ensure!(
        staged_output.is_dir(),
        "xctrace succeeded but produced no trace at {}; previous recording kept",
        staged_output.display()
    );
    publish(&staged_output, &output).with_context(|| {
        format!(
            "publishing recording; new trace is at {}",
            staged_output.display()
        )
    })?;
    let _ = fs::remove_dir(staging_path);
    if let Err(error) = remember(&output) {
        eprintln!(
            "mperf: saved trace, but could not update .mperf-latest: {error:#}; open with mperf report -i {}",
            output.display()
        );
    }
    eprintln!("mperf: saved {}", output.display());
    Ok(())
}

fn execute_recording(command_args: &[OsString]) -> Result<std::process::ExitStatus> {
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ])
    .context("installing recording stop handlers")?;
    let mut command = Command::new("xcrun");
    command.args(command_args);
    // Give the recorder the foreground terminal, like a shell launching a job.
    // Ctrl-C goes directly to xctrace; signals sent to mperf are forwarded once.
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command.spawn().context("launching xcrun xctrace")?;
    let foreground = match Foreground::claim(child.id()) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("giving xctrace the foreground terminal");
        }
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Err(error) => break Err(error),
            Ok(None) => (),
        }
        for _ in signals.pending() {
            interrupt_child(child.id());
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    drop(foreground);
    status.context("waiting for xctrace")
}

fn interrupt_child(pid: u32) {
    if let Ok(pid) = i32::try_from(pid) {
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(pid),
            nix::sys::signal::Signal::SIGINT,
        );
    }
}

/// Restore shell job control even when recording fails. No controlling terminal
/// is normal over SSH without a PTY or in redirected/automated sessions.
struct Foreground {
    terminal: fs::File,
    previous: nix::unistd::Pid,
}
impl Foreground {
    fn claim(pid: u32) -> Result<Option<Self>> {
        let Ok(terminal) = fs::File::open("/dev/tty") else {
            return Ok(None);
        };
        let previous = nix::unistd::tcgetpgrp(&terminal)?;
        if previous != nix::unistd::getpgrp() {
            return Ok(None);
        }
        let guard = Self { terminal, previous };
        let child_group = nix::unistd::Pid::from_raw(i32::try_from(pid)?);
        guard.set(child_group)?;
        // The child can reach a prompt before tcsetpgrp; resume it if SIGTTIN
        // stopped it during that short startup race.
        let _ = nix::sys::signal::kill(child_group, nix::sys::signal::Signal::SIGCONT);
        Ok(Some(guard))
    }
    fn set(&self, group: nix::unistd::Pid) -> Result<()> {
        use nix::sys::signal::{SigSet, SigmaskHow, Signal, pthread_sigmask};
        let mut block = SigSet::empty();
        block.add(Signal::SIGTTOU);
        let mut previous_mask = SigSet::empty();
        pthread_sigmask(
            SigmaskHow::SIG_BLOCK,
            Some(&block),
            Some(&mut previous_mask),
        )?;
        let result = nix::unistd::tcsetpgrp(&self.terminal, group);
        pthread_sigmask(SigmaskHow::SIG_SETMASK, Some(&previous_mask), None)?;
        result.context("setting terminal foreground process group")
    }
}
impl Drop for Foreground {
    fn drop(&mut self) {
        if let Err(error) = self.set(self.previous) {
            eprintln!("mperf: could not restore terminal foreground: {error:#}");
        }
    }
}

fn remove_previous(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn publish(staged: &Path, output: &Path) -> Result<()> {
    let old = old_path(output);
    let existed = output.try_exists()?;
    if existed {
        remove_previous(&old)?;
        fs::rename(output, &old).context("rotating previous recording")?;
    }
    if let Err(error) = fs::rename(staged, output) {
        if existed {
            fs::rename(&old, output).context(
                "publication failed and rollback failed; previous recording remains at .old",
            )?;
        }
        return Err(error).context("moving new trace into place");
    }
    Ok(())
}

fn remember(output: &Path) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(".")?;
    file.write_all(
        output
            .to_str()
            .context("latest path requires UTF-8")?
            .as_bytes(),
    )?;
    file.persist(".mperf-latest")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command as CliCommand};
    use clap::Parser;

    fn parse(args: &[&str]) -> Record {
        let CliCommand::Record(record) = Cli::parse_from(args).command else {
            panic!("expected record")
        };
        record
    }

    #[test]
    fn mapping_preserves_target_arguments_and_quotes() {
        let args = parse(&[
            "mperf",
            "record",
            "-g",
            "echo",
            "a b",
            "$(touch nope)",
            "it's",
            "--flag",
        ]);
        let mapped = arguments(&args, Path::new("test.trace"));
        assert_eq!(&mapped[mapped.len() - 5..], &args.command);
        let display = shell_command(&mapped).unwrap();
        let split = shlex::split(&display).unwrap();
        assert_eq!(
            split[1..],
            mapped
                .iter()
                .map(|a| a.to_str().unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn attach_does_not_redirect_target_output() {
        let args = parse(&["mperf", "record", "-g", "-p", "123"]);
        let mapped = arguments(&args, Path::new("a.trace"));
        assert!(mapped.contains(&"--attach".into()));
        assert!(!mapped.contains(&"--target-stdout".into()));
    }

    #[test]
    fn publication_rotates_and_rolls_back() {
        let temp = tempfile::tempdir().unwrap();
        let output = temp.path().join("a.trace");
        let staged = temp.path().join("new.trace");
        fs::create_dir(&output).unwrap();
        fs::write(output.join("data"), "old").unwrap();
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("data"), "new").unwrap();
        publish(&staged, &output).unwrap();
        assert_eq!(
            fs::read_to_string(old_path(&output).join("data")).unwrap(),
            "old"
        );
        assert_eq!(fs::read_to_string(output.join("data")).unwrap(), "new");
        assert!(publish(&staged, &output).is_err());
        assert_eq!(fs::read_to_string(output.join("data")).unwrap(), "new");
    }
}
