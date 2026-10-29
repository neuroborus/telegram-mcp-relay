set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# List contributor commands.
default:
    @just --list

# Validate documentation links, skills, canonical symlinks and Runner setup.
check-repo:
    python3 tools/check_repository.py

# Run all Rust gates after the application crate has been introduced.
check-rust:
    cargo fmt --all -- --check
    cargo clippy --locked --workspace --all-targets -- -D warnings
    RUSTDOCFLAGS="-D warnings" cargo doc --locked --workspace --no-deps --document-private-items
    cargo test --locked --workspace

# Run the current repository gates; absent Rust is reported as non-applicable.
check: check-repo
    if test -f Cargo.toml; then just check-rust; else printf '%s\n' 'Rust gates not applicable: application crate is not implemented yet.'; fi
