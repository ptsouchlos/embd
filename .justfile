set windows-shell := ["pwsh.exe", "-NoLogo", "-Command"]
set shell := ["bash", "-c"]

default:
    @just -l

[doc('Build the project (default is debug)')]
[group('dev')]
build config="debug":
    cargo build --workspace --all-targets {{ if config == "release" { "--release" } else { "" } }}

[doc('Build and run tests (default is debug)')]
[group('dev')]
test config="debug":
    cargo test --workspace --all-targets {{ if config == "release" { "--release" } else { "" } }} -- --include-ignored

[doc('Run clippy')]
[group('dev')]
lint:
    cargo clippy --all --tests -- -D warnings

[doc('Format all Rust code')]
[group('dev')]
format:
    cargo fmt --all
