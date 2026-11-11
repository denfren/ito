# ito — a Luau script runner for config files

# List available recipes
default:
    @just --list

# ---- Build ----

# Build both binaries (debug)
build:
    cargo build

# Build with release optimizations
build-release:
    cargo build --release

# Type-check without producing binaries
check:
    cargo check

# ---- Test ----

# Run the full test suite
test:
    cargo test

# Run tests and update all insta snapshots in place
test-accept:
    INSTA_UPDATE=always cargo test

# ---- Lint / format ----

# Format code with rustfmt
fmt:
    cargo fmt

# Verify formatting (CI)
fmt-check:
    cargo fmt --all --check

# Clippy across the workspace, warnings denied (CI)
clippy:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

# Everything CI should run
ci: fmt-check clippy test

# ---- Docs ----

# Build the mdbook site into docs/book/ (requires mdbook)
docs:
    mdbook build docs

# Serve the mdbook site on http://localhost:3000 (requires mdbook)
docs-serve:
    mdbook serve docs

# View the embedded help (append a topic, e.g. `just doc fs`)
doc topic="":
    cargo run --quiet --bin ito -- --doc {{topic}}

# ---- Run ----

# Run a script with the debug `ito` binary: `just run path/to/script.luau args...`
run script *args:
    cargo run --quiet --bin ito -- {{script}} {{args}}

# Run a script with the debug `ied` binary
run-ied script *args:
    cargo run --quiet --bin ied -- {{script}} {{args}}

# ---- Install / release ----

# Install both binaries from source
install:
    cargo install --path ito

# Build distributable artifacts with cargo-dist
dist:
    dist build

# Show the cargo-dist release plan
dist-plan:
    dist plan
