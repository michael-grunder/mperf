# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `mperf record` for commands, individual PIDs, and all-process capture through
  xctrace, with shell-quoted command previews and a non-mutating dry-run mode.
- Current/previous trace rotation, preservation of failed captures, and automatic
  selection of the last successful output in the working directory.
- CPU report TUI with self/inclusive overhead, call chains, symbol filters, DSO
  zoom, sorting, metadata, and an integrated perf-style keyboard help screen.
- `mperf diff` TUI and text output comparing baseline and new sample shares,
  including added and removed symbols.
- Support for external trace bundles and exported CPU-table XML, including
  xctrace frame/backtrace references, tagged backtraces, run selection, and
  time-profile/cpu-profile schema selection.
- Recording templates, extra instruments, device selection, time limits, ring
  buffer windows, and discovery of local xctrace capabilities through `mperf list`.
- Detailed command help, plain text output for pipes and headless workflows, and
  respect for `NO_COLOR` and basic terminal capabilities.
