# AGENTS.md

This file provides guidance to agents when working with code in this repository.

## Build / Test / Lint

- **Build**: `cargo build`
- **Test**: `cargo test`
- **Single test**: `cargo test test_name` (e.g. `cargo test test_adjust_limits`)
- **Clippy**: `cargo clippy` (treat warnings seriously; `src/ifdata.rs` is generated code and uses `#![allow(clippy::all)]`)
- **Rust edition**: 2021 (see Cargo.toml)

## Architecture

a2ltool is a CLI tool for reading, writing, and modifying ASAP2 (A2L) files used in automotive ECU calibration. Entry point is `src/main.rs::core()` which orchestrates operations in fixed order: load -> check -> merge -> update addresses -> cleanup -> sort -> output.

Key modules:
- `src/dwarf/` - DWARF debug info reader (ELF files). `TypeInfo` and `DwarfDataType` are central types.
- `src/update/` - Address update logic. Each A2L object type (characteristic, measurement, axis_pts, blob, instance) has its own submodule.
- `src/insert.rs` - Creates new A2L objects from ELF symbols.
- `src/ifdata.rs` - **Generated code** from `a2ml_specification!` macro. Do not manually edit.
- `src/symbol.rs` - Symbol lookup with support for struct member access (`var.field[idx]`) and Vector-style annotations (`{Function:name}{CompileUnit:unit}`).

## Code Style

- Visibility: use `pub(crate)` for cross-module items, not `pub` (except `A2lVersion` enum and `main`).
- Error handling: functions return `Result<T, String>` (not custom error types). Use `.map_err(|e| e.to_string())?` for conversion.
- The `cond_print!` macro gates output on verbosity level; `ext_println!` always prints but adds timestamps at verbose >= 2.
- `IndexMap` (not `HashMap`) is used for `variables` to preserve insertion order in debug data.
- Types from the `a2lfile` crate are re-exported and used directly (e.g. `a2lfile::Measurement`, `a2lfile::A2lFile`).

## Testing

- Tests live in `#[cfg(test)] mod test` blocks inside source files, not in separate files.
- ELF test fixtures are in `tests/elffiles/` (binary `.elf` files + source `.c` files).
- The `update::test` and `symbol::test` modules contain the bulk of unit tests.
