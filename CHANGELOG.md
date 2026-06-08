# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
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

[0.3.0]: https://github.com/denfren/ito/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/denfren/ito/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/denfren/ito/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/denfren/ito/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/denfren/ito/releases/tag/v0.1.0

