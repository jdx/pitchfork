# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
