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
