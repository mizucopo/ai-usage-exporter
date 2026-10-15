Run the Rust quality gate with the committed lockfile:

```bash
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets --all-features
```

Use local mock Codex processes for tests. Do not read or change real account
credentials, contact the account service, or alter production environments.

Preserve `/metrics`, environment variables, quota parsing, cache invalidation,
and Prometheus names, labels and meanings. Performance measurement and
benchmarks are out of scope.
