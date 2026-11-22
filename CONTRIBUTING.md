# Contributing

Open an issue before changing schema compatibility or write behavior. Include
the Calibre version, `PRAGMA user_version`, operating system, and a small
reproduction.

Keep production code independent from Calibre and Citadel source. You may use
Calibre as a black-box oracle in a temporary library. Record new research in
`docs/provenance.md`.

Use Cargo 1.100 or newer when adding or updating dependencies, including when
working in a Nix shell. The repository sets a seven-day minimum publish age
for registry dependencies. Cargo keeps versions already in `Cargo.lock`.
Continue testing the resulting lockfile with Rust 1.89.0.

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
