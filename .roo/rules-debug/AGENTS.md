# Debug Mode Rules

- Use `--debug-print` flag to dump parsed A2L and ELF data structures. This is expensive and bypasses `cond_print!` intentionally.
- Use `-vv` (double verbose) to get timestamped output for performance profiling.
- DWARF type resolution errors are non-fatal: `get_type()` in `src/dwarf/typereader.rs` replaces failures with `DwarfDataType::Other(0)` and prints diagnostics, allowing partial results.
- `wip_items` in the type reader tracks the recursion stack to detect self-referential types (linked lists, trees) and prevent infinite loops.
- Symbol lookup tries three sources in order: SYMBOL_LINK, IF_DATA (CANAPE_EXT), then object name. Check all three when debugging lookup failures.
