## 2026-11-08 - [Quadratic parsing in pulldown-cmark RUSTSEC-2026-0433]
**Vulnerability:** pulldown-cmark < 0.13.6 is vulnerable to quadratic parsing of nested image wikilinks with `ENABLE_WIKILINKS`.
**Learning:** Security vulnerabilities in dependencies reported by cargo-deny can cause CI failures. The fix is to update the dependency to the patched version. Since this project has multiple Cargo.toml manifests, the update needs to be applied to all of them.
**Prevention:** Regularly audit and update dependencies, keeping in mind that workspaces or multiple manifests require updating the dependency in each manifest individually.
