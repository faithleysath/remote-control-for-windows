use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub(crate) struct Cli {
    #[arg(long, global = true)]
    pub(crate) server: Option<String>,
    #[arg(long, global = true)]
    pub(crate) token: Option<String>,
    #[arg(long, global = true)]
    pub(crate) session: Option<PathBuf>,
    #[arg(long, global = true)]
    pub(crate) json: bool,
    #[arg(long, global = true)]
    pub(crate) timeout: Option<String>,
    #[arg(long, global = true)]
    pub(crate) audit_label: Option<String>,
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    #[command(name = "connect")]
    Open {
        #[arg(long = "id")]
        id: String,
        #[arg(long)]
        totp: String,
        #[arg(long)]
        totp_period_seconds: Option<u64>,
        #[arg(long)]
        force: bool,
    },
    Status,
    Exec {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        command: Vec<String>,
    },
    Upload {
        local: PathBuf,
        remote: String,
        #[arg(long)]
        overwrite: bool,
        #[arg(long)]
        sha256: Option<String>,
    },
    Download {
        remote: String,
        local: PathBuf,
    },
    Screenshot {
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        display: Option<u32>,
        #[arg(long, default_value = "png")]
        format: String,
    },
    Windows,
    #[command(name = "mouse-move")]
    Move {
        #[arg(long, allow_negative_numbers = true)]
        x: i32,
        #[arg(long, allow_negative_numbers = true)]
        y: i32,
    },
    #[command(name = "mouse-click")]
    Click {
        #[arg(long, allow_negative_numbers = true)]
        x: i32,
        #[arg(long, allow_negative_numbers = true)]
        y: i32,
        #[arg(long, default_value = "left")]
        button: String,
    },
    #[command(name = "mouse-scroll")]
    Scroll {
        #[arg(long, allow_negative_numbers = true)]
        delta: i32,
    },
    #[command(name = "keyboard-type")]
    Type {
        text: String,
    },
    #[command(name = "keyboard-key")]
    Key {
        key: String,
    },
    #[command(name = "disconnect")]
    Close,
    Mcp,
}

pub(crate) fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Open { .. } => "connect",
        Commands::Status => "status",
        Commands::Exec { .. } => "exec",
        Commands::Upload { .. } => "upload",
        Commands::Download { .. } => "download",
        Commands::Screenshot { .. } => "screenshot",
        Commands::Windows => "windows",
        Commands::Move { .. } => "mouse.move",
        Commands::Click { .. } => "mouse.click",
        Commands::Scroll { .. } => "mouse.scroll",
        Commands::Type { .. } => "keyboard.type",
        Commands::Key { .. } => "keyboard.key",
        Commands::Close => "disconnect",
        Commands::Mcp => "mcp",
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, Commands};

    #[test]
    fn parses_negative_mouse_scroll_delta() {
        let cli = Cli::try_parse_from(["rcwctl", "mouse-scroll", "--delta", "-1"]).unwrap();
        assert!(matches!(cli.command, Commands::Scroll { delta: -1 }));
    }

    #[test]
    fn parses_negative_mouse_coordinates() {
        let cli =
            Cli::try_parse_from(["rcwctl", "mouse-click", "--x", "-20", "--y", "-10"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::Click { x: -20, y: -10, .. }
        ));
    }
}
