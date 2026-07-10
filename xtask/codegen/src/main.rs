use anyhow::Context;
use clap::{Parser, Subcommand};
use paperdown::config::schema::{generate_schema, schema_path};

#[derive(Debug, Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    JsonSchema,
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::JsonSchema => {
            let path = schema_path();
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create schema directory {}", parent.display()))?;
            }
            let schema = generate_schema().context("generate JSON Schema")?;
            std::fs::write(&path, schema)
                .with_context(|| format!("write schema artifact {}", path.display()))?;
        }
    }
    Ok(())
}
