_default:
    just --list

# Run cargo clippy and cargo fmt
lint:
    cargo clippy \
      --all-targets \
      --all-features \
      --locked \
      -- \
      -D warnings \
      -D clippy::dbg_macro

    cargo fmt

# Apply fixes reported by `just lint`
lint-fix:
    cargo clippy \
      --all-targets \
      --all-features \
      --locked \
      --fix --allow-dirty

    cargo fmt

# Generates the `paperdown.schema.json`
gen-schema:
    cargo run -p xtask_codegen -- json-schema

# Install the paperdown binary (release mode) to `~/.cargo/bin/paperdown`.
# Note that a `~/.local/bin/paperdown` installed another way may shadow this.
install-binary:
    cargo install --path . --force --profile=release
