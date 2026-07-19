# Contributing

Open an issue before changing schema compatibility or write behavior. Include
the Calibre version, `PRAGMA user_version`, operating system, and a small
reproduction.

Keep production code independent from Calibre and Citadel source. You may use
Calibre as a black-box oracle in a temporary library. Record new research in
`docs/provenance.md`.

Run:

```console
cargo fmt --check
cargo check --all-targets --all-features
cargo test --all-features
cargo clippy --all-targets --all-features -- \
  -D warnings \
  -W clippy::pedantic \
  -W clippy::nursery
cargo doc --all-features --no-deps
```

Add failure tests around each SQLite/filesystem boundary. Tests may write only
inside a fresh temporary directory.

Use conventional commit subjects. Do not include unrelated changes in a
commit. The maintainer will decide when to publish.
