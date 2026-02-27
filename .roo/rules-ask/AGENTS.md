# Ask Mode Rules

- The `a2lfile` crate (external dependency) provides parsing, writing, and the A2L object model. a2ltool adds ELF/DWARF integration on top.
- "Module-level merge" (`--merge`) combines contents of two MODULEs into one. "Project-level merge" (`--merge-project`) appends MODULEs side by side.
- A2L version downgrade (`--a2lversion`) is lossy: incompatible features are deleted, not converted.
- The `src/dwarf/` module reads DWARF debug info from ELF files. Types are resolved lazily per-variable, not eagerly for the whole file.
- IF_DATA blocks contain tool-vendor-specific data (primarily XCP protocol settings). The A2ML specification in `src/ifdata.rs` defines the expected structure.
