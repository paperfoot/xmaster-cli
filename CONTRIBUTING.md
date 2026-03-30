# Contributing to XMaster

Thanks for your interest in contributing.

## Getting Started

1. Fork the repo and clone your fork
2. Install Rust (stable): https://rustup.rs
3. Build and run:
   ```bash
   cargo build
   cargo run -- post "test" --json
   ```
4. Run tests:
   ```bash
   cargo test
   ```

## What to Work On

- Check [open issues](https://github.com/199-biotechnologies/xmaster/issues) for things labeled `good first issue` or `help wanted`
- Bug fixes are always welcome
- If you want to add a new command or feature, open an issue first so we can discuss scope

## Pull Requests

- Keep PRs focused. One feature or fix per PR.
- Add tests for new functionality when possible.
- Make sure `cargo check` and `cargo test` pass before submitting.
- Write a clear PR description explaining what changed and why.

## Code Style

- Follow standard Rust conventions (`cargo fmt`, `cargo clippy`).
- Keep functions short. If a function does too much, split it.
- Error messages should be actionable -- tell the user what to do, not just what went wrong.

## License

By contributing, you agree that your contributions will be licensed under the [MIT License](LICENSE).
