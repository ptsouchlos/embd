# Import shared just recipes
import "infra/rust/just/shells.just"
import "infra/rust/just/clippy.just"
import "infra/rust/just/format.just"

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

[doc('Install embd locally')]
[group('dev')]
install:
    cargo install --path .
