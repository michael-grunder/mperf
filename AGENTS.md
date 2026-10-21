# mperf

## Purpose

`mperf` brings Linux `perf`-like profiling to macOS without requiring a GUI. It
serves both as a wrapper around Apple's `xctrace` and as a terminal user interface
(TUI) for exploring profiling data, akin to `perf report`.

The primary audience is Linux programmers who want to profile macOS code using
familiar terminal workflows, including over SSH on headless machines. Keep
recording and analysis usable entirely from the command line.

## CLI and profiling workflow

- Use `clap` for argument parsing, with a subcommand structure such as
  `mperf record ...` and `mperf report ...`.
- Model commands and options on Linux `perf` where there is a meaningful mapping
  to `xctrace`. Translate the user's profiling intent into the appropriate
  `xctrace` invocation.
- Aim to support the familiar workflow:

  ```sh
  mperf record -g <command> [args...]
  mperf report
  ```

  `record` should invoke `xctrace`; `report` should open the most recently
  generated profiling data in the TUI by default.
- Display the mapped `xctrace` command when recording so users can learn how
  Linux `perf` options translate to macOS tooling. Render it with accurate shell
  quoting, and execute subprocesses using structured arguments.
- Support opening profiling data created directly with `xctrace`, independently
  of `mperf record`.
- Be explicit about unsupported options and differences in profiling semantics.
  Do not silently ignore flags or imply equivalence where none exists.

## Documentation and integrated help

- Keep `README.md` up to date as features evolve. Keep it concise, with a short
  overview and just a few examples of basic usage; put detailed guidance in the
  binary's integrated help.
- Strive for an extremely self-documenting binary. Provide rich, discoverable
  help for both the overall application and individual commands and features,
  including valid invocation syntax, options, relevant context, and examples.
- Support `--help` and an explicit `help` argument at the application and
  subcommand levels. For example, `mperf help` explains the application, and
  `mperf report help` provides detailed guidance and examples for `report`.
- When arguments are malformed, show an actionable error and usage specific to
  the affected command, with enough context to correct the invocation and a
  pointer to its detailed help. For example, invalid `mperf report` arguments
  should produce `report` usage rather than only top-level usage.
- Keep README examples and integrated help aligned with implemented behavior
  as features change.

## Report TUI and terminal output

- Make `mperf report` feel familiar to users of Linux `perf report`, especially
  its navigation, key mappings, and profiling-data exploration workflows.
- Preserve `perf` key mappings wherever practical. Add mappings for useful
  `xctrace` capabilities that have no `perf` equivalent, and make them discoverable
  through in-app help.
- Use rich terminal output by default, including color and bold text where they
  improve readability. Respect terminal capabilities and `NO_COLOR`, and keep
  redirected output readable without terminal control sequences.
- Keep the core workflow usable without opening Instruments or another GUI.

## Implementation standards

- Prefer appropriate, maintained Rust crates over hand-rolled implementations.
  Write custom code when it offers a clear benefit in simplicity, correctness,
  performance, or project fit; explain non-obvious tradeoffs.
- Keep argument translation, `xctrace` execution, profiling-data handling, and
  presentation sufficiently separated to evolve and validate independently.
- Document intended behavior as a goal until it is implemented; do not describe
  planned commands or compatibility as already available.

## Changelog

- Maintain `CHANGELOG.md` in the **Keep a Changelog** format.
- Record notable changes under `## [Unreleased]`, using the applicable sections:
  `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed`, and `Security`.
- On release, move those entries into a versioned section dated `YYYY-MM-DD`.
  Write entries for users, rather than copying commit logs.

## Validation

- Run Clippy for every change, enabling the `all`, `nursery`, and `pedantic` lint
  groups across all targets and features:

  ```sh
  cargo clippy --all-targets --all-features -- -D warnings -D clippy::all -D clippy::nursery -D clippy::pedantic
  ```

- Fix lint findings before considering a change complete. Keep any justified
  lint exceptions narrowly scoped and explain why they are necessary; do not
  broadly disable these lint groups.
- For Rust changes, check formatting with `cargo fmt --check` and run tests
  appropriate to the changed behavior.
- If validation cannot run, report the blocker and which checks remain unverified.
