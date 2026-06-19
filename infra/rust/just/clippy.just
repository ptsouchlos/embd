[doc('Run clippy')]
[group('dev')]
lint:
    cargo clippy --all --tests -- -D warnings

echo:
    echo "Run 'just lint' to run clippy"
