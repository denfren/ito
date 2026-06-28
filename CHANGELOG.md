# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **ito:** Script flags — pass `key=value` and `+flag` on the command line;
  read them with `ito::opt`, `ito::try_opt`, and `ito::flag`
- First-class invocation of well-known script names — `ito <name>`
  - looks for one of
    - `<name>.ito.rhai`
    - `.<name>.ito.rhai`
    - `ito/<name>.rhai`
    - `.ito/<name>.rhai`

### Deprecated

- `-S`/`-N`/`-B` (`--string`/`--numeric`/`--bool`) injected globals — use `key=value` and `+flag` with `ito::opt`/`ito::flag` instead

### Changed

- **Breaking:** `run` now requires an explicit path argument
  - to get the old default run `ito main`

### Fixed

- Writing an empty string to a new file now correctly reports as a change
- **yamlito:** Block scalar decoding fixes (literal and folded styles)
  - test cases taken from [yaml-test-suite](https://github.com/yaml/yaml-test-suite)
  - explicit indent indicator (`|2`, `>1`) now relative to parent block indent per spec
  - folded scalar line-folding and blank-line handling corrected
  - double-quoted scalar trailing whitespace trimmed correctly before line folds

## [0.4.0] - 2026-06-11

### Added

- **yaml:** Structural mapping edits; rename seq edits to append/insert_at/remove_at

## [0.3.4] - 2026-06-10

### Fixed

- **yamlito:** Reindent block-seq item continuations at green level
- **yamlito:** Fixes and refactorings

## [0.3.2] - 2026-06-10

### Added

- **yaml:** Add yaml::edit for format-preserving in-place edits
- **yamlito:** Add sequence cursor insert/push/remove edit API
- **yamlito:** Continue multiline flow sequence layout on insert/remove
- **yamlito:** Return live node from sequence item edits
- **yaml:** Add sequence push/insert/remove to yaml::edit cursor

### Documentation

- **yaml:** Document yaml::edit .doc entry point and scalar quote-style API

## [0.3.0] - 2026-06-08

### Added

- **scripting:** Prefix builder methods with with_*
- **fs:** Exclude hidden files by default in glob and finder
- **scripting:** Make_cleared returns nothing
- **scripting:** Make_* string methods return nothing instead of the result
- **scripting:** Set_* string methods accept char or string

### Fixed

- **docs:** Make SUMMARY.md mdbook-parseable

## [0.2.0] - 2026-06-02

### Added

- **string:** Add trim_start, trim_end, and array join methods
- **string:** Replace built-in overrides with to_*/make_* variants

### Documentation

- Split strings-arrays-maps into string/array/map topics, add ito-vs-rhai

### Fixed

- **string:** Join takes array by value, not mut ref

## [0.1.2] - 2026-06-02

### Added

- Add cargo-deb packaging for linux x86_64 and aarch64

### Fixed

- **deb:** Fix tarball glob and deb arch copy path
- **deb:** Install aarch64 cross libs and skip strip for cross-arch deb
- **deb:** Copy deb from target/<triple>/debian/ not target/debian/
- **deb:** Use --output to write deb directly to target path
- **deb:** Write debs to target/deb/ to avoid self-copy by dist

## [0.1.1] - 2026-06-01

### Added

- **re:** Add regular expression module

## [0.1.0] - 2026-06-01

### Added

- Initial release of ito

[0.4.0]: https://github.com/denfren/ito/compare/v0.3.4...v0.4.0
[0.3.4]: https://github.com/denfren/ito/compare/v0.3.2...v0.3.4
[0.3.2]: https://github.com/denfren/ito/compare/v0.3.0...v0.3.2
[0.3.0]: https://github.com/denfren/ito/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/denfren/ito/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/denfren/ito/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/denfren/ito/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/denfren/ito/releases/tag/v0.1.0

