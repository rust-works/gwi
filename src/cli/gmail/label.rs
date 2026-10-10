//! CLI commands for Gmail label list / add / remove.

pub(crate) mod add;
pub(crate) mod list;
pub(crate) mod remove;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::gmail::client::GmailClient;

/// Manages Gmail labels.
#[derive(Parser)]
pub struct LabelCommand {
    /// The label subcommand to execute.
    #[command(subcommand)]
    pub command: LabelSubcommands,
}

/// Label subcommands.
#[derive(Subcommand)]
pub enum LabelSubcommands {
    /// Lists Gmail labels (mirrors the `gmail_label_list` MCP tool).
    #[command(about = "Lists Gmail labels (mirrors the gmail_label_list MCP tool)", long_about = None)]
    List(list::ListCommand),
    /// Adds a label to one or more messages (`gmail.modify` scope required).
    #[command(about = "Adds a label to one or more messages (gmail.modify scope required)", long_about = None)]
    Add(add::AddCommand),
    /// Removes a label from one or more messages (`gmail.modify` scope required).
    #[command(about = "Removes a label from one or more messages (gmail.modify scope required)", long_about = None)]
    Remove(remove::RemoveCommand),
}

impl LabelCommand {
    /// Executes the label command.
    pub async fn execute(self, client: &GmailClient) -> Result<()> {
        match self.command {
            LabelSubcommands::List(cmd) => cmd.execute(client).await,
            LabelSubcommands::Add(cmd) => cmd.execute(client).await,
            LabelSubcommands::Remove(cmd) => cmd.execute(client).await,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn label_subcommands_list_variant() {
        let cmd = LabelCommand {
            command: LabelSubcommands::List(list::ListCommand {
                output: crate::cli::gmail::format::OutputFormat::Table,
            }),
        };
        assert!(matches!(cmd.command, LabelSubcommands::List(_)));
    }
}
