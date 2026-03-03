# Contributing to envpod

We welcome contributions! Here's how to get involved.

## Bug Reports

Open a [GitHub issue](https://github.com/markamo/envpod-ce/issues/new?template=bug_report.md) with:

- envpod version (`envpod --version`)
- Linux distro, kernel version, architecture
- Steps to reproduce
- Expected vs actual behavior
- Relevant pod.yaml (redact secrets)

## Pull Requests

1. Fork the repo and create a feature branch from `main`
2. Make your changes with atomic commits
3. Ensure all checks pass:
   ```bash
   cargo fmt --check
   cargo clippy -- -D warnings
   cargo test
   ```
4. Open a PR against `main` with a clear description

### Code Standards

- **Format:** `cargo fmt` (rustfmt defaults)
- **Lint:** `cargo clippy` with no warnings
- **Test:** `cargo test` must pass. Add tests for new functionality.
- **Commits:** Atomic, descriptive commit messages. One logical change per commit.

### What Makes a Good PR

- Focused on a single change
- Includes tests for new behavior
- Doesn't break existing tests
- Follows existing code patterns

## Contributor License Agreement (CLA)

By submitting a pull request, you agree to the [Xtellix CLA](https://envpod.dev/cla). This allows Xtellix Inc. to distribute your contribution under both the BSL-1.1 open source license and a commercial license. You retain copyright of your contribution.

## Code of Conduct

- Be respectful and constructive
- Focus on technical merit
- No harassment, discrimination, or personal attacks
- Assume good intent

Violations may result in removal from the project. Report concerns to mark@envpod.dev.

## Questions?

- [GitHub Discussions](https://github.com/markamo/envpod-ce/discussions)
- Email: mark@envpod.dev
