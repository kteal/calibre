# Repository instructions

- Preserve user work and inspect `git status` before editing.
- Use stable Rust with edition 2024 and MSRV 1.85.0.
- Forbid unsafe Rust.
- Keep `rusqlite` types out of the public API.
- Treat database paths as hostile and keep filesystem work inside the canonical
  library root.
- Parameterize values in SQL. Allow dynamic identifiers only from a closed,
  validated set.
- Never change Calibre's schema version or run migrations.
- Write tests only to fresh temporary libraries.
- Do not copy or translate Calibre or Citadel source, SQL, triggers, or
  algorithms. Record behavioral research in `docs/provenance.md`.
- Run formatting, checks, tests, strict Clippy, rustdoc, packaging, and Nix
  checks before a milestone commit.
- Do not run `cargo publish` or push without owner approval.
