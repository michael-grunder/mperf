use std::{ffi::OsString, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Terminal CPU profiling on macOS, with a perf-like workflow",
    long_about = "Record Instruments traces through xctrace, explore CPU samples in a terminal, and compare recordings. Full Xcode is required for recording and exporting .trace bundles; exported XML can be analyzed anywhere.",
    after_help = "Examples:\n  mperf record -g ./my-program --flag\n  mperf record -g -p 1234 --time-limit 10s\n  mperf report\n  mperf diff\n\nUse 'mperf COMMAND help' for detailed guidance. This is a subset of Linux perf, not a perf.data reader."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Record a launched command, existing process, or the whole system
    Record(Record),
    /// Explore CPU samples and call chains (TUI by default)
    Report(Report),
    /// Compare CPU sample shares between two recordings
    Diff(Diff),
    /// List xctrace templates, instruments, or devices
    List {
        #[arg(value_enum, default_value = "templates")]
        kind: ListKind,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ListKind {
    Templates,
    Instruments,
    Devices,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  mperf record -g ./program arg1 --program-option\n  mperf record -g -p 1234 --time-limit 10s\n  mperf record -a --time-limit 5s\n  mperf record -g --instrument os_signpost -- ./program\n  mperf record --template 'Time Profiler' --device 'My iPhone' -p 1234\n\nPut mperf options BEFORE the command. Use '--' to launch a command named 'help'.\n-g documents your intent: Time Profiler already collects call stacks. xctrace controls unwinding and sampling frequency; perf -F, -c, -e, --call-graph, CPU affinity and multiple PIDs are not supported. Use a custom template for instrument configuration. Additional instruments are retained in the trace, but report/diff currently analyze CPU backtraces only.\n\nCtrl-C stops xctrace and lets it finish saving. Existing OUTPUT is rotated to OUTPUT.old only after a successful recording; the previous .old is replaced. On failure the original is kept and the partial trace path is printed. Default: mperf.trace / mperf.trace.old. The last successful output is remembered in .mperf-latest in the current directory.\n\nRequires full Xcode selected with xcode-select (Command Line Tools alone are insufficient). macOS may require Developer Tools authorization and permission to attach. Compile with debug symbols for useful names. See 'mperf list' and 'xcrun xctrace help record' for local capabilities."
)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["pid", "all_cpus", "command"])))]
#[allow(clippy::struct_excessive_bools)] // Independent CLI switches, not state-machine states.
pub struct Record {
    /// Collect call stacks (already enabled by the default Time Profiler template)
    #[arg(short = 'g')]
    pub call_graph: bool,
    /// Attach to one running PID
    #[arg(short, long, value_parser = clap::value_parser!(u32).range(1..))]
    pub pid: Option<u32>,
    /// Record all processes (xctrace --all-processes; not perf CPU selection)
    #[arg(short = 'a', long = "all-cpus")]
    pub all_cpus: bool,
    /// Output trace bundle; must end in .trace
    #[arg(short, long, default_value = "mperf.trace")]
    pub output: PathBuf,
    /// Instruments template name or .tracetemplate path
    #[arg(long, default_value = "Time Profiler")]
    pub template: String,
    /// Add an instrument; repeat to combine CPU profiling with other data
    #[arg(long)]
    pub instrument: Vec<String>,
    /// Device name or UDID (default: this Mac)
    #[arg(long)]
    pub device: Option<String>,
    /// Stop after a duration, e.g. 500ms, 10s, 2m, 1h
    #[arg(long, value_parser = duration)]
    pub time_limit: Option<String>,
    /// Keep only the trailing duration in a ring buffer, e.g. 5s
    #[arg(long, value_parser = window)]
    pub window: Option<String>,
    /// Name the run inside the trace
    #[arg(long)]
    pub run_name: Option<String>,
    /// Skip xctrace privacy prompts (does not grant permissions)
    #[arg(long)]
    pub no_prompt: bool,
    /// Print the mapping without running xctrace or changing files
    #[arg(long)]
    pub dry_run: bool,
    /// Command and its arguments; all remaining arguments belong to the command
    #[arg(trailing_var_arg = true, num_args = 1..)]
    pub command: Vec<OsString>,
}

fn duration(value: &str) -> Result<String, String> {
    let number = ["ms", "s", "m", "h"]
        .iter()
        .find_map(|suffix| value.strip_suffix(suffix));
    if number
        .and_then(|n| n.parse::<f64>().ok())
        .is_some_and(|n| n.is_finite() && n > 0.0)
    {
        Ok(value.to_owned())
    } else {
        Err("use a positive duration with units, e.g. 500ms, 10s, 2m, 1h".into())
    }
}

fn window(value: &str) -> Result<String, String> {
    if value.ends_with('h') {
        return Err("window supports ms, s, or m; use minutes instead of hours".into());
    }
    duration(value)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Sort {
    #[default]
    Overhead,
    SelfWeight,
    Symbol,
    Dso,
    Comm,
    Delta,
}

#[derive(Debug, Args)]
pub struct ViewOptions {
    /// Print a plain text report (also automatic when redirected or TERM=dumb)
    #[arg(long)]
    pub stdio: bool,
    /// Force the interactive TUI (requires a terminal)
    #[arg(long, conflicts_with = "stdio")]
    pub tui: bool,
    /// Include callees in overhead; self overhead is always displayed separately
    #[arg(long)]
    pub children: bool,
    /// Sort order (default: overhead for report, absolute delta for diff)
    #[arg(long, value_enum)]
    pub sort: Option<Sort>,
    /// Filter symbol names by case-insensitive substring
    #[arg(long)]
    pub symbol: Option<String>,
    /// Filter process names by case-insensitive substring
    #[arg(long)]
    pub comm: Option<String>,
    /// Filter image/library names by case-insensitive substring
    #[arg(long)]
    pub dso: Option<String>,
    /// Minimum overhead percentage (either side in diff)
    #[arg(long, default_value = "0", value_parser = percent)]
    pub percent_limit: f64,
    /// Maximum top-level entries in plain text output
    #[arg(long, default_value = "50", value_parser = clap::value_parser!(u32).range(1..))]
    pub limit: u32,
}

fn percent(value: &str) -> Result<f64, String> {
    value
        .parse::<f64>()
        .ok()
        .filter(|n| (0.0..=100.0).contains(n))
        .ok_or_else(|| "percentage must be between 0 and 100".into())
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  mperf report\n  mperf report -i external.trace\n  mperf report --children --comm my-program\n  mperf report -i exported.xml --stdio --limit 100\n  mperf report -i multi-run.trace --run 2 --schema time-profile\n\nDefault input: last successful recording in this directory (.mperf-latest), otherwise mperf.trace. Accepts traces created directly by Instruments/xctrace and raw xctrace CPU-table XML exports.\n\nAuto-selects time-profile, then cpu-profile in the selected run. --run defaults to the latest run. Other instruments can be recorded, but their tables are not yet visualized. --schema chooses a supported CPU table explicitly.\n\nOverhead is the percentage of exported sample weight (or sample count if no weight column exists), NOT wall time or perf hardware cycles. Self = leaf frame; --children counts each symbol once per stack. Filters keep the original denominator. Unresolved stacks are counted and reported separately. Large XML exports are loaded into memory.\n\nTUI: h/?/F1 help, arrows/j move, Space/PgDn page, +/e expand call chains, E/C expand/collapse all, d zoom DSO, / filter symbols, H headers, i metadata, q/Esc back/quit. Enter opens symbol details (no assembly annotation yet). Additional mperf keys: o sort, w self/inclusive. Full key guide is available inside the TUI."
)]
pub struct Report {
    /// Trace bundle or exported CPU-table XML
    #[arg(short, long)]
    pub input: Option<PathBuf>,
    #[command(flatten)]
    pub selection: Selection,
    #[command(flatten)]
    pub view: ViewOptions,
}

#[derive(Debug, Args)]
pub struct Selection {
    /// Run number in the trace (default: latest); XML exports are already selected
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..))]
    pub run: Option<u32>,
    /// CPU sample schema (auto-detected by default)
    #[arg(long, value_parser = ["time-profile", "cpu-profile"])]
    pub schema: Option<String>,
}

#[derive(Debug, Args)]
#[command(
    after_help = "Examples:\n  mperf diff\n  mperf diff before.trace after.trace\n  mperf diff before.xml after.xml --stdio --children\n\nWithout paths, compares the last recording's .old sibling to that recording (fallback mperf.trace.old and mperf.trace). Provide both paths or neither. --run and --schema apply to both inputs.\n\nMatches by process NAME + image + symbol (PIDs are intentionally ignored). Delta = new overhead minus baseline overhead in percentage points; positive means a larger share, not necessarily slower execution. Missing symbols have zero share on that side. Default ordering is absolute delta. Compare similar workloads and recording settings. Incompatible weight units are rejected. Unresolved addresses may not match across ASLR runs.\n\nUses the report TUI with h/?/F1 help. Expanded call chains show the comparison recording's stacks. Both totals and recording metadata are available with i. See 'mperf report help' for filtering and percentage semantics."
)]
pub struct Diff {
    /// Baseline trace or XML (default: last recording's .old sibling)
    #[arg(requires = "new")]
    pub baseline: Option<PathBuf>,
    /// Comparison trace or XML (default: last successful recording)
    #[arg(requires = "baseline")]
    pub new: Option<PathBuf>,
    #[command(flatten)]
    pub selection: Selection,
    #[command(flatten)]
    pub view: ViewOptions,
}

/// Support `mperf report help` without treating a launched program's arguments as help.
pub fn arguments() -> Vec<OsString> {
    let mut args: Vec<_> = std::env::args_os().collect();
    if args.len() == 3
        && matches!(
            args[1].to_str(),
            Some("record" | "report" | "diff" | "list")
        )
        && args[2] == "help"
    {
        args[2] = "--help".into();
    }
    args
}
