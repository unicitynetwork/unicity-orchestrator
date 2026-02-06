# Contributing

## Getting Started

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests
5. Submit a pull request

## Development Setup

```bash
git clone https://github.com/joshuajbouw/unicity-orchestrator.git
cd unicity-orchestrator
cargo build
```

## Build Commands

```bash
# Build
cargo build
cargo build --release

# Run tests
cargo test
cargo test <test_name>              # Run a specific test
cargo test -- --nocapture           # Show test output

# Linting
cargo clippy
cargo fmt
```

## Project Structure

See the [Architecture Overview](architecture/overview.md) for a detailed module map.

## Adding a New Registry Provider

Implement the `RegistryProvider` trait:

```rust
#[async_trait]
impl RegistryProvider for MyRegistryProvider {
    async fn list_manifests(&self) -> Result<Vec<RegistryManifest>>;
    async fn get_manifest(&self, name: &str, version: &str) -> Result<Option<RegistryManifest>>;
    async fn download_manifest(&self, manifest: &RegistryManifest) -> Result<serde_json::Value>;
    async fn verify_manifest(&self, manifest: &RegistryManifest, content: &[u8]) -> Result<bool>;
}
```

## Adding Symbolic Rules

Rules are stored in the `symbolic_rule` database table. See [Symbolic Reasoning](architecture/symbolic-reasoning.md) for the rule format and expression language.

## Code Style

- Follow standard Rust conventions (`cargo fmt`, `cargo clippy`)
- Use `anyhow` for error handling in application code
- Use `tracing` for logging (not `println!`)
- Prefer `Arc` for shared ownership in async contexts

## Support

For issues and questions, please use the [GitHub issue tracker](https://github.com/joshuajbouw/unicity-orchestrator/issues).

## License

MIT License — see the LICENSE file for details.
