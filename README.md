# mperf

A terminal profiling workflow for macOS, inspired by Linux `perf`. Record with
Apple's `xctrace`, explore CPU hotspots and call stacks in a TUI, and compare
recordings without opening Instruments. Requires full Xcode for recording and
trace export; exported CPU-table XML can be analyzed without Xcode.

```sh
cargo install --path .
mperf record -g ./my-program --its-options
mperf report

mperf record -g -p 1234 --time-limit 10s
mperf diff
```

Recordings default to `mperf.trace`, with the previous successful recording in
`mperf.trace.old`. Custom `-o session.trace` outputs are remembered in the current
directory, so `report` and `diff` find them automatically. Failed captures preserve
the previous recordings. Ctrl-C stops recording and lets xctrace save.

```sh
mperf report -i external.trace
mperf diff before.trace after.trace --stdio
mperf record -g --instrument os_signpost --window 5s ./my-program
mperf list templates
```

Press **h**, **?**, or **F1** in the TUI for its key guide. Navigation, paging,
call-chain expansion, symbol filtering, and DSO zoom follow perf where practical.
Use `mperf record help`, `mperf report help`, or `mperf diff help` for detailed
options, examples, semantics, and limitations. Redirected reports are plain text;
`NO_COLOR` disables TUI colors.

This initial version analyzes `time-profile` and `cpu-profile` backtraces.
Percentages describe sampled weight, and diffs show changes in sample share—not
elapsed-time speedups. Extra instruments and custom templates can be recorded,
but non-CPU tables, assembly annotation, Linux PMU flags, and `perf.data` are not
supported. XML exports are loaded into memory. Build targets with debug symbols
for useful names. Actual recording also depends on macOS profiling permissions.
