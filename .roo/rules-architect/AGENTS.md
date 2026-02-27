# Architect Mode Rules

- Operations in `core()` execute in a fixed order that cannot be rearranged: load -> check -> version convert -> merge -> update -> insert -> cleanup -> sort -> output.
- The `DebugData` struct owns all type info and variable info. It's immutable after construction. Update functions borrow it via `&DebugData`.
- `TypeInfo` uses recursive `Box<TypeInfo>` for arrays, bitfields, and pointers. `DwarfDataType::TypeRef` is a lazy reference resolved during iteration.
- `iter::VariablesIterator` flattens nested structs/arrays into individual addressable items for bulk insertion. It resolves `TypeRef` chains at iteration time.
- Module 0 assumption: many operations assume `a2l_file.project.module[0]` is the target. Only `--merge-project` creates multiple modules.
- The `a2lfile` crate handles all A2L parsing/serialization. a2ltool never directly reads/writes A2L text.
