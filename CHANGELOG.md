# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.8.0](https://github.com/endevco/pitchfork/compare/v2.7.0...v2.8.0) - 2026-04-28

### Added

- *(boot-start)* support system level register ([#397](https://github.com/endevco/pitchfork/pull/397))

### Fixed

- *(tui)* remove blocking loading ([#394](https://github.com/endevco/pitchfork/pull/394))

## [2.7.0](https://github.com/jdx/pitchfork/compare/v2.6.0...v2.7.0) - 2026-04-26

### Added

- *(supervisor)* run daemons as a configured user ([#384](https://github.com/jdx/pitchfork/pull/384))
- *(watch)* impl poll mode ([#353](https://github.com/jdx/pitchfork/pull/353))
- *(cli)* stop / restart --all-global / --all-local ([#385](https://github.com/jdx/pitchfork/pull/385))
- version check in IPC ([#354](https://github.com/jdx/pitchfork/pull/354))

### Fixed

- pass error when failed to parse toml ([#386](https://github.com/jdx/pitchfork/pull/386))

### Other

- *(deps)* update rust crate xx to v2.5.4 ([#378](https://github.com/jdx/pitchfork/pull/378))
- *(deps)* lock file maintenance ([#371](https://github.com/jdx/pitchfork/pull/371))
- *(deps)* update rust crate xx to v2.5.4 ([#363](https://github.com/jdx/pitchfork/pull/363))
- *(deps)* update rust crate hyper-rustls to v0.27.9 ([#359](https://github.com/jdx/pitchfork/pull/359))
- *(deps)* update rust crate rmcp to v1.5.0 ([#364](https://github.com/jdx/pitchfork/pull/364))
- *(deps)* update rust crate libc to v0.2.185 ([#360](https://github.com/jdx/pitchfork/pull/360))
- *(deps)* update rust crate tokio to v1.52.1 ([#365](https://github.com/jdx/pitchfork/pull/365))
- *(deps)* update rust crate uuid to v1.23.1 ([#362](https://github.com/jdx/pitchfork/pull/362))
- *(deps)* update rust crate rustls to v0.23.38 ([#361](https://github.com/jdx/pitchfork/pull/361))
- *(deps)* update rust crate clap to v4.6.1 ([#358](https://github.com/jdx/pitchfork/pull/358))
- *(deps)* update rust crate axum to v0.8.9 ([#357](https://github.com/jdx/pitchfork/pull/357))

## [2.6.0](https://github.com/jdx/pitchfork/compare/v2.5.0...v2.6.0) - 2026-04-12

### Added

- *(proxy)* auto start when visiting the proxied URL ([#347](https://github.com/jdx/pitchfork/pull/347))

### Fixed

- some issues related to sudo supervisor ([#323](https://github.com/jdx/pitchfork/pull/323))
- *(port)* should fail when ready_port is in use ([#350](https://github.com/jdx/pitchfork/pull/350))
- *(deps)* update rcgen to 0.14 ([#349](https://github.com/jdx/pitchfork/pull/349))
- *(deps)* update reqwest to 0.13 ([#348](https://github.com/jdx/pitchfork/pull/348))
- detect port conflicts on loopback addresses, not just 0.0.0.0 ([#345](https://github.com/jdx/pitchfork/pull/345))
- narrow REAPED_STATUSES cfg to non-Linux unix only ([#346](https://github.com/jdx/pitchfork/pull/346))
- *(deps)* update rust crate ratatui to 0.30 ([#331](https://github.com/jdx/pitchfork/pull/331))
- *(deps)* update rust crate toml to v1 ([#344](https://github.com/jdx/pitchfork/pull/344))
- *(deps)* update rust crate strum to 0.28 ([#334](https://github.com/jdx/pitchfork/pull/334))
- *(deps)* update rust crate notify-debouncer-full to 0.7 ([#330](https://github.com/jdx/pitchfork/pull/330))
- *(deps)* update rust crate nix to 0.31 ([#329](https://github.com/jdx/pitchfork/pull/329))
- *(deps)* update rust crate listeners to 0.5 ([#328](https://github.com/jdx/pitchfork/pull/328))
- *(deps)* update rust crate sysinfo to 0.38 ([#335](https://github.com/jdx/pitchfork/pull/335))
- *(deps)* update rust crate cron to 0.16 ([#324](https://github.com/jdx/pitchfork/pull/324))
- *(deps)* update rust crate crossterm to 0.29 ([#325](https://github.com/jdx/pitchfork/pull/325))

### Other

- *(deps)* update rust crate rmcp to v1.4.0 ([#327](https://github.com/jdx/pitchfork/pull/327))

## [2.5.0](https://github.com/jdx/pitchfork/compare/v2.4.0...v2.5.0) - 2026-04-10

### Added

- *(port-management)* impl proxy for local dev ([#293](https://github.com/jdx/pitchfork/pull/293))

### Other

- *(deps)* update rust crate indexmap to v2.14.0 ([#321](https://github.com/jdx/pitchfork/pull/321))
- *(deps)* update rust crate xx to v2.5.3 ([#322](https://github.com/jdx/pitchfork/pull/322))
- *(deps)* update rust crate tokio to v1.51.1 ([#320](https://github.com/jdx/pitchfork/pull/320))
- update inconsistencies in docs ([#312](https://github.com/jdx/pitchfork/pull/312))

## [2.4.0](https://github.com/jdx/pitchfork/compare/v2.3.0...v2.4.0) - 2026-04-09

### Added

- add mcp tools ([#311](https://github.com/jdx/pitchfork/pull/311))
- impl container mode ([#305](https://github.com/jdx/pitchfork/pull/305))

### Fixed

- use correct base dir for `.config/pitchfork.toml` case ([#307](https://github.com/jdx/pitchfork/pull/307))
- use FSEvent on macos to avoid `Too many files` ([#301](https://github.com/jdx/pitchfork/pull/301))

### Other

- *(deps)* lock file maintenance ([#310](https://github.com/jdx/pitchfork/pull/310))

## [2.3.0](https://github.com/jdx/pitchfork/compare/v2.2.0...v2.3.0) - 2026-03-31

### Added

- impl memory/cpu limit ([#294](https://github.com/jdx/pitchfork/pull/294))

## [2.2.0](https://github.com/jdx/pitchfork/compare/v2.1.0...v2.2.0) - 2026-03-24

### Added

- *(hooks)* add `on_stop` and `on_exit` ([#291](https://github.com/jdx/pitchfork/pull/291))
- impl `start --all-local` and `--all-global` ([#282](https://github.com/jdx/pitchfork/pull/282))

### Other

- *(deps)* lock file maintenance ([#292](https://github.com/jdx/pitchfork/pull/292))

## [2.1.0](https://github.com/jdx/pitchfork/compare/v2.0.0...v2.1.0) - 2026-03-08

### Added

- add `settings.toml` ([#275](https://github.com/jdx/pitchfork/pull/275))

### Fixed

- correct json schema for DaemonId ([#277](https://github.com/jdx/pitchfork/pull/277))
- *(supervisor)* prevent file descriptor leaks in SSE streaming and IPC ([#267](https://github.com/jdx/pitchfork/pull/267))
- fixed scroll disabled when log <20 lines ([#268](https://github.com/jdx/pitchfork/pull/268))

### Other

- Support .config/pitchfork.toml ([#265](https://github.com/jdx/pitchfork/pull/265))
- *(README)* update broken link ([#270](https://github.com/jdx/pitchfork/pull/270))

## [2.0.0](https://github.com/jdx/pitchfork/compare/v1.6.0...v2.0.0) - 2026-03-04

### Added

- *(port-management)* Add port conflict detection and auto-bump support ([#259](https://github.com/jdx/pitchfork/pull/259))

### Fixed

- fixed tui log rendering and scroll behavior ([#264](https://github.com/jdx/pitchfork/pull/264))
- *(config)* correct pitchfork config add to generate valid TOML ([#258](https://github.com/jdx/pitchfork/pull/258))
- *(watch)* implement file watching for daemon auto-restart ([#255](https://github.com/jdx/pitchfork/pull/255))
- *(logs)* bypass pager when --tail is active ([#253](https://github.com/jdx/pitchfork/pull/253))

### Other

- [**breaking**] add namespace to daemon id ([#213](https://github.com/jdx/pitchfork/pull/213))
- *(deps)* lock file maintenance ([#263](https://github.com/jdx/pitchfork/pull/263))
- Update README.md ([#254](https://github.com/jdx/pitchfork/pull/254))

## [1.6.0](https://github.com/jdx/pitchfork/compare/v1.5.0...v1.6.0) - 2026-02-21

### Added

- *(web)* add PITCHFORK_WEB_PATH support for reverse proxy path prefixes ([#244](https://github.com/jdx/pitchfork/pull/244))
- add daemon lifecycle hooks and retry count env vars ([#245](https://github.com/jdx/pitchfork/pull/245))

### Fixed

- pass cwd to ready_cmd spawning ([#243](https://github.com/jdx/pitchfork/pull/243))

## [1.5.0](https://github.com/jdx/pitchfork/compare/v1.4.3...v1.5.0) - 2026-02-16

### Added

- *(supervisor)* use process groups for atomic daemon shutdown ([#239](https://github.com/jdx/pitchfork/pull/239))
- SIGKILL after SIGTERM ([#238](https://github.com/jdx/pitchfork/pull/238))

## [1.4.2](https://github.com/jdx/pitchfork/compare/v1.4.1...v1.4.2) - 2026-02-11

### Fixed

- *(state)* fix Errored(None) producing invalid TOML state file ([#231](https://github.com/jdx/pitchfork/pull/231))

## [1.4.0](https://github.com/jdx/pitchfork/compare/v1.3.0...v1.4.0) - 2026-02-11

### Added

- *(config)* add `dir` and `env` fields for daemons ([#227](https://github.com/jdx/pitchfork/pull/227))

### Fixed

- *(status)* return error when daemon not found and improve test diagnostics ([#224](https://github.com/jdx/pitchfork/pull/224))

## [1.3.0](https://github.com/jdx/pitchfork/compare/v1.2.0...v1.3.0) - 2026-02-01

### Added

- *(list)* show available daemons and align logics with TUI ([#206](https://github.com/jdx/pitchfork/pull/206))
- *(logs)* support `--since <humantime>`, use pager by default ([#204](https://github.com/jdx/pitchfork/pull/204))
- support `pitchfork.local.toml` ([#198](https://github.com/jdx/pitchfork/pull/198))
- impl `stop --all` ([#195](https://github.com/jdx/pitchfork/pull/195))
- beautify web ui ([#191](https://github.com/jdx/pitchfork/pull/191))
- add ready_cmd option ([#187](https://github.com/jdx/pitchfork/pull/187))

### Fixed

- refactor the logic of stopping a daemon and add tests ([#192](https://github.com/jdx/pitchfork/pull/192))

### Other

- re-order code to suit for multi-frontend structure ([#197](https://github.com/jdx/pitchfork/pull/197))
- *(deps)* update rust crate xx to v2.3.1 ([#203](https://github.com/jdx/pitchfork/pull/203))
- *(deps)* update rust crate clap to v4.5.56 ([#202](https://github.com/jdx/pitchfork/pull/202))
- *(ci)* run linting on all files in CI ([#196](https://github.com/jdx/pitchfork/pull/196))
- Update README.md logo ([#184](https://github.com/jdx/pitchfork/pull/184))

## [1.2.0](https://github.com/jdx/pitchfork/compare/v1.1.0...v1.2.0) - 2026-01-19

### Added

- enhance miette error diagnostics with source highlighting and URLs ([#183](https://github.com/jdx/pitchfork/pull/183))
- add structured IPC error types with miette diagnostics ([#181](https://github.com/jdx/pitchfork/pull/181))
- add structured config error types with file path context ([#182](https://github.com/jdx/pitchfork/pull/182))
- add config editor to TUI for creating and editing daemons ([#171](https://github.com/jdx/pitchfork/pull/171))
- add custom diagnostic error types with miette ([#180](https://github.com/jdx/pitchfork/pull/180))

### Other

- improve miette error handling adoption ([#177](https://github.com/jdx/pitchfork/pull/177))
- modularize supervisor.rs into focused submodules ([#175](https://github.com/jdx/pitchfork/pull/175))

## [1.1.0](https://github.com/jdx/pitchfork/compare/v1.0.2...v1.1.0) - 2026-01-19

### Added

- add file watching to auto-restart daemons ([#165](https://github.com/jdx/pitchfork/pull/165))
- support boolean values for retry configuration ([#170](https://github.com/jdx/pitchfork/pull/170))
- disable web UI by default ([#172](https://github.com/jdx/pitchfork/pull/172))
- auto-generate JSON schema from Rust types ([#167](https://github.com/jdx/pitchfork/pull/167))

### Fixed

- improve cron watcher granularity for sub-minute schedules ([#163](https://github.com/jdx/pitchfork/pull/163))
- improve log file position tracking accuracy ([#164](https://github.com/jdx/pitchfork/pull/164))

## [1.0.2](https://github.com/jdx/pitchfork/compare/v1.0.1...v1.0.2) - 2026-01-19

### Fixed

- handle channel send errors in watch_files.rs ([#157](https://github.com/jdx/pitchfork/pull/157))
- implement clean IPC server shutdown ([#154](https://github.com/jdx/pitchfork/pull/154))
- replace unwraps with proper error handling in deps.rs ([#153](https://github.com/jdx/pitchfork/pull/153))

### Other

- *(deps)* lock file maintenance ([#158](https://github.com/jdx/pitchfork/pull/158))

## [1.0.1](https://github.com/jdx/pitchfork/releases/tag/v1.0.1) - 2026-01-19

### Fixed

- correct tag ref format for release asset uploads ([#149](https://github.com/jdx/pitchfork/pull/149))

## [1.0.0](https://github.com/jdx/pitchfork/releases/tag/v1.0.0) - 2026-01-19

### Added

- implement daemon dependency resolution ([#135](https://github.com/jdx/pitchfork/pull/135))
- add restart command to CLI ([#134](https://github.com/jdx/pitchfork/pull/134))

### Fixed

- restart command preserves daemon dependency configuration ([#142](https://github.com/jdx/pitchfork/pull/142))
- add missing depends field to restart command ([#136](https://github.com/jdx/pitchfork/pull/136))
- set IPC socket permissions to 0600 for security ([#133](https://github.com/jdx/pitchfork/pull/133))
- handle shell command parsing errors instead of silently failing ([#132](https://github.com/jdx/pitchfork/pull/132))

### Other

- bump version to 1.0.0 ([#147](https://github.com/jdx/pitchfork/pull/147))
- release v0.3.1 ([#121](https://github.com/jdx/pitchfork/pull/121))
- reduce unnecessary daemon cloning in loops ([#144](https://github.com/jdx/pitchfork/pull/144))
- use periodic log flushing instead of per-line ([#139](https://github.com/jdx/pitchfork/pull/139))
- refresh only tracked PIDs instead of all processes ([#141](https://github.com/jdx/pitchfork/pull/141))
- cache compiled regex patterns ([#143](https://github.com/jdx/pitchfork/pull/143))

### Security

- add rate limiting to IPC server ([#137](https://github.com/jdx/pitchfork/pull/137))
- canonicalize config paths to prevent symlink exploitation ([#138](https://github.com/jdx/pitchfork/pull/138))
- add centralized daemon ID validation ([#140](https://github.com/jdx/pitchfork/pull/140))

## [0.3.1](https://github.com/jdx/pitchfork/compare/v0.3.0...v0.3.1) - 2026-01-19

### Added

- implement daemon dependency resolution ([#135](https://github.com/jdx/pitchfork/pull/135))
- add restart command to CLI ([#134](https://github.com/jdx/pitchfork/pull/134))

### Fixed

- restart command preserves daemon dependency configuration ([#142](https://github.com/jdx/pitchfork/pull/142))
- add missing depends field to restart command ([#136](https://github.com/jdx/pitchfork/pull/136))
- set IPC socket permissions to 0600 for security ([#133](https://github.com/jdx/pitchfork/pull/133))
- handle shell command parsing errors instead of silently failing ([#132](https://github.com/jdx/pitchfork/pull/132))

### Other

- reduce unnecessary daemon cloning in loops ([#144](https://github.com/jdx/pitchfork/pull/144))
- use periodic log flushing instead of per-line ([#139](https://github.com/jdx/pitchfork/pull/139))
- refresh only tracked PIDs instead of all processes ([#141](https://github.com/jdx/pitchfork/pull/141))
- cache compiled regex patterns ([#143](https://github.com/jdx/pitchfork/pull/143))

### Security

- add rate limiting to IPC server ([#137](https://github.com/jdx/pitchfork/pull/137))
- canonicalize config paths to prevent symlink exploitation ([#138](https://github.com/jdx/pitchfork/pull/138))
- add centralized daemon ID validation ([#140](https://github.com/jdx/pitchfork/pull/140))

## [0.3.0](https://github.com/jdx/pitchfork/compare/v0.2.1...v0.3.0) - 2026-01-18

### Added

- *(web)* add devilish pitchfork theming to web UI ([#115](https://github.com/jdx/pitchfork/pull/115))
- *(web)* add web UI for daemon management ([#112](https://github.com/jdx/pitchfork/pull/112))
- show startup logs on successful daemon start ([#111](https://github.com/jdx/pitchfork/pull/111))
- add HTTP ready check for daemon startup ([#110](https://github.com/jdx/pitchfork/pull/110))
- delay autostopping daemons when leaving directory ([#108](https://github.com/jdx/pitchfork/pull/108))
- *(logs)* clear all logs when no daemon specified ([#109](https://github.com/jdx/pitchfork/pull/109))
- *(list)* show error messages in daemon list output ([#107](https://github.com/jdx/pitchfork/pull/107))
- refactor the code structure of `start` and `run`, allowing for parallel starting daemons ([#56](https://github.com/jdx/pitchfork/pull/56))
- [**breaking**] support auto start on boot ([#53](https://github.com/jdx/pitchfork/pull/53))
- print logs when failed on `pf start|run` ([#52](https://github.com/jdx/pitchfork/pull/52))
- [**breaking**] support global system/user config ([#46](https://github.com/jdx/pitchfork/pull/46))
- *(test)* refactor tests and add tests for `interval_watch` and `cron_watch` ([#45](https://github.com/jdx/pitchfork/pull/45))

### Fixed

- add timeouts to IPC operations to prevent shell hook hangs ([#106](https://github.com/jdx/pitchfork/pull/106))
- *(deps)* update rust crate toml to 0.9 ([#50](https://github.com/jdx/pitchfork/pull/50))
- replace panics with proper error handling ([#90](https://github.com/jdx/pitchfork/pull/90))
- *(deps)* update rust crate notify to v8 ([#78](https://github.com/jdx/pitchfork/pull/78))
- *(deps)* update rust crate duct to v1 ([#72](https://github.com/jdx/pitchfork/pull/72))
- *(deps)* update rust crate dirs to v6 ([#64](https://github.com/jdx/pitchfork/pull/64))
- *(deps)* update rust crate cron to 0.15 ([#48](https://github.com/jdx/pitchfork/pull/48))
- *(deps)* update rust crate sysinfo to 0.37 ([#49](https://github.com/jdx/pitchfork/pull/49))
- *(deps)* update rust crate itertools to 0.14 ([#33](https://github.com/jdx/pitchfork/pull/33))
- *(deps)* update rust crate strum to 0.27 ([#35](https://github.com/jdx/pitchfork/pull/35))
- *(deps)* update rust crate console to 0.16 ([#32](https://github.com/jdx/pitchfork/pull/32))
- give a user-friendly error when the work fails ([#44](https://github.com/jdx/pitchfork/pull/44))

### Other

- *(cli)* add long_about with examples to CLI commands ([#91](https://github.com/jdx/pitchfork/pull/91))
- fix documentation issues and inconsistencies ([#89](https://github.com/jdx/pitchfork/pull/89))
- *(deps)* lock file maintenance ([#88](https://github.com/jdx/pitchfork/pull/88))
- *(deps)* update rust crate serde_json to v1.0.149 ([#87](https://github.com/jdx/pitchfork/pull/87))
- *(deps)* lock file maintenance ([#85](https://github.com/jdx/pitchfork/pull/85))
- *(deps)* update rust crate serde_json to v1.0.148 ([#84](https://github.com/jdx/pitchfork/pull/84))
- *(deps)* update rust crate tempfile to v3.24.0 ([#82](https://github.com/jdx/pitchfork/pull/82))
- *(deps)* update rust crate rmp-serde to v1.3.1 ([#80](https://github.com/jdx/pitchfork/pull/80))
- *(deps)* update rust crate serde_json to v1.0.147 ([#81](https://github.com/jdx/pitchfork/pull/81))
- *(deps)* lock file maintenance ([#79](https://github.com/jdx/pitchfork/pull/79))
- *(deps)* update rust crate shell-words to v1.1.1 ([#77](https://github.com/jdx/pitchfork/pull/77))
- *(deps)* lock file maintenance ([#76](https://github.com/jdx/pitchfork/pull/76))
- *(deps)* update rust crate log to v0.4.29 ([#75](https://github.com/jdx/pitchfork/pull/75))
- *(deps)* lock file maintenance ([#73](https://github.com/jdx/pitchfork/pull/73))
- *(deps)* lock file maintenance ([#68](https://github.com/jdx/pitchfork/pull/68))
- *(deps)* lock file maintenance ([#65](https://github.com/jdx/pitchfork/pull/65))
- *(deps)* lock file maintenance ([#62](https://github.com/jdx/pitchfork/pull/62))
- *(deps)* update rust crate clap to v4.5.51 ([#60](https://github.com/jdx/pitchfork/pull/60))
- *(deps)* lock file maintenance ([#59](https://github.com/jdx/pitchfork/pull/59))
- *(deps)* update rust crate clap to v4.5.50 ([#57](https://github.com/jdx/pitchfork/pull/57))
- Update README ([#55](https://github.com/jdx/pitchfork/pull/55))
- *(deps)* lock file maintenance ([#54](https://github.com/jdx/pitchfork/pull/54))
- *(deps)* lock file maintenance ([#47](https://github.com/jdx/pitchfork/pull/47))
