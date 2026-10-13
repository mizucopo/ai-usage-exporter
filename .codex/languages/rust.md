# Rust guidance

Run the Rust quality checks from the repository root:

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```
