# CLAUDE.md - Project Configuration

## Project
**mtb** - Polymarket Trading Bot (Rust)

## Tech Stack
- Language: Rust (edition 2021)
- Build: Cargo

## Commands
- Build: `cargo build`
- Run: `cargo run`
- Test: `cargo test`
- Check: `cargo check`
- Lint: `cargo clippy`
- Format: `cargo fmt`

## Code Style
- Follow standard Rust conventions (rustfmt defaults)
- Use `clippy` lints — treat warnings as errors
- Prefer strong typing over stringly-typed patterns
- Use `thiserror` for library errors, `anyhow` for application errors
- Async runtime: tokio
- Keep functions small and focused
- Write tests for all non-trivial logic

## Architecture Guidelines
- Separate concerns: API clients, strategy logic, execution, and configuration
- Never hardcode secrets — use environment variables or config files
- All API calls must have proper error handling and retries
- Log all trades and decisions for auditability
- Fail safe: if in doubt, do not place a trade

## Git
- Main branch: `main`
- Commit messages: imperative mood, concise
- Do not commit secrets, API keys, or `.env` files

## Important Notes
- This is a financial trading bot — correctness and safety are paramount
- Always confirm before making destructive changes
- Refer to AGENT.md for agent behavior and PROGRESS.md for current status
