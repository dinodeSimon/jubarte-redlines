## 2026-11-08 - Prevent String Reallocations from Iterator Collect

**Learning:** Iterators chaining `.split_inclusive`, `.filter` or `.map`, and `.collect::<String>()` hide their exact size hints, causing `String` to dynamically reallocate intermediate objects in memory, which wastes cycles on hot paths.
**Action:** Replace the chain with `String::with_capacity(input.len())` and use a `for` loop with direct `push_str` when possible to ensure the string uses a single contiguous block of memory without reallocation.
