use std::path::PathBuf;

use clap::{Parser, Subcommand};
use rcw_common::protocol::{DEFAULT_MOUSE_BUTTON, DEFAULT_SCREENSHOT_FORMAT};

#[derive(Debug, Parser)]
#[command(author, version, about)]
pub(crate) struct Cli {
    #[arg(
        long,
        global = true,
        help = "Server HTTP/WS endpoint. Defaults to RCW_SERVER or the built-in local endpoint."
    )]
    pub(crate) server: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Control token used to open and operate a remote-control session."
    )]
    pub(crate) token: Option<String>,
    #[arg(
        long,
        global = true,
        help = "Path to the CLI session file. MCP keeps session state in memory instead."
    )]
    pub(crate) session: Option<PathBuf>,
    #[arg(long, global = true, help = "Print machine-readable JSON output.")]
    pub(crate) json: bool,
    #[arg(
        long,
        global = true,
        help = "Short purpose label written to controller/server/host audit logs."
    )]
    pub(crate) audit_label: Option<String>,
    #[arg(short, long, global = true, help = "Enable verbose diagnostic output.")]
    pub(crate) verbose: bool,
    #[command(subcommand)]
    pub(crate) command: Commands,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    #[command(name = "connect")]
    Open {
        #[arg(long = "id", help = "Target machine ID displayed by rcw-host.")]
        id: String,
        #[arg(
            long,
            help = "Optional runtime Host ID displayed by rcw-host; use to disambiguate short ID collisions."
        )]
        host_id: Option<String>,
        #[arg(long, help = "Current TOTP code displayed by rcw-host.")]
        totp: String,
        #[arg(
            long,
            help = "Expected TOTP period in seconds. Defaults to the configured project period."
        )]
        totp_period_seconds: Option<u64>,
        #[arg(
            long,
            help = "Replace an existing active session for this host after TOTP verification."
        )]
        force: bool,
    },
    Status,
    Exec {
        #[arg(
            long,
            help = "Maximum remote process runtime. Defaults to 24h. Examples: 500ms, 30s, 10m."
        )]
        timeout: Option<String>,
        #[arg(
            long,
            help = "How long this CLI call waits for completion. Defaults to 90s; 0 returns a task_id immediately."
        )]
        wait: Option<String>,
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            required = true,
            help = "Remote program and arguments to execute after `--`."
        )]
        command: Vec<String>,
    },
    #[command(name = "exec-status")]
    ExecStatus {
        #[arg(help = "Task ID returned by exec when the remote command is still running.")]
        task_id: String,
    },
    #[command(name = "exec-cancel")]
    ExecCancel {
        #[arg(help = "Task ID returned by exec.")]
        task_id: String,
    },
    Forward {
        #[arg(
            short = 'L',
            value_name = "LISTEN=TARGET",
            help = "Local forward: controller listens, host connects target. May be repeated."
        )]
        local: Vec<String>,
        #[arg(
            short = 'R',
            value_name = "LISTEN=TARGET",
            help = "Remote forward: host listens, controller connects target. May be repeated."
        )]
        remote: Vec<String>,
    },
    Upload {
        #[arg(help = "Local file path to read and upload.")]
        local: PathBuf,
        #[arg(help = "Destination path on the remote Windows host.")]
        remote: String,
        #[arg(long, help = "Allow replacing an existing remote file.")]
        overwrite: bool,
        #[arg(long, help = "Expected SHA-256 of the local file before upload.")]
        sha256: Option<String>,
    },
    Download {
        #[arg(help = "Remote file path to download.")]
        remote: String,
        #[arg(help = "Local output file path to write.")]
        local: PathBuf,
    },
    Screenshot {
        #[arg(long, help = "Local PNG output path.")]
        output: PathBuf,
        #[arg(long, help = "Optional display index on the remote host.")]
        display: Option<u32>,
        #[arg(
            long,
            default_value = DEFAULT_SCREENSHOT_FORMAT,
            help = "Screenshot format. Currently only png is supported."
        )]
        format: String,
    },
    Windows,
    #[command(name = "mouse-move")]
    Move {
        #[arg(
            long,
            allow_negative_numbers = true,
            help = "Absolute screen X coordinate."
        )]
        x: i32,
        #[arg(
            long,
            allow_negative_numbers = true,
            help = "Absolute screen Y coordinate."
        )]
        y: i32,
    },
    #[command(name = "mouse-click")]
    Click {
        #[arg(
            long,
            allow_negative_numbers = true,
            help = "Absolute screen X coordinate."
        )]
        x: i32,
        #[arg(
            long,
            allow_negative_numbers = true,
            help = "Absolute screen Y coordinate."
        )]
        y: i32,
        #[arg(
            long,
            default_value = DEFAULT_MOUSE_BUTTON,
            help = "Mouse button to click: left, right, or middle."
        )]
        button: String,
    },
    #[command(name = "mouse-scroll")]
    Scroll {
        #[arg(
            long,
            allow_negative_numbers = true,
            help = "Mouse wheel delta. Negative values scroll down."
        )]
        delta: i32,
    },
    #[command(name = "keyboard-type")]
    Type {
        #[arg(help = "Text to type on the remote host.")]
        text: String,
    },
    #[command(name = "keyboard-key")]
    Key {
        #[arg(help = "Key or key chord to press, for example Enter or Control+C.")]
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
        Commands::ExecStatus { .. } => "exec.status",
        Commands::ExecCancel { .. } => "exec.cancel",
        Commands::Forward { .. } => "forward",
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

    #[test]
    fn rejects_global_timeout_flag() {
        let err = Cli::try_parse_from(["rcwctl", "--timeout", "5m", "status"]).unwrap_err();
        assert!(err.to_string().contains("unexpected argument"));
    }

    #[test]
    fn parses_exec_timeout_as_subcommand_option() {
        let cli =
            Cli::try_parse_from(["rcwctl", "exec", "--timeout", "5m", "--", "cmd.exe"]).unwrap();
        match cli.command {
            Commands::Exec {
                timeout,
                wait,
                command,
            } => {
                assert_eq!(timeout.as_deref(), Some("5m"));
                assert!(wait.is_none());
                assert_eq!(command, vec!["cmd.exe"]);
            }
            _ => panic!("expected exec command"),
        }
    }

    #[test]
    fn parses_exec_wait_flag() {
        let cli = Cli::try_parse_from(["rcwctl", "exec", "--wait", "0", "--", "cmd.exe"]).unwrap();
        match cli.command {
            Commands::Exec { wait, command, .. } => {
                assert_eq!(wait.as_deref(), Some("0"));
                assert_eq!(command, vec!["cmd.exe"]);
            }
            _ => panic!("expected exec command"),
        }
    }

    #[test]
    fn parses_repeated_forward_specs() {
        let cli = Cli::try_parse_from([
            "rcwctl",
            "forward",
            "-L",
            "127.0.0.1:15432=127.0.0.1:5432",
            "-R",
            "127.0.0.1:18080=127.0.0.1:8080",
        ])
        .unwrap();
        match cli.command {
            Commands::Forward { local, remote } => {
                assert_eq!(local.len(), 1);
                assert_eq!(remote.len(), 1);
            }
            _ => panic!("expected forward command"),
        }
    }

    #[test]
    fn parses_connect_host_id() {
        let cli = Cli::try_parse_from([
            "rcwctl",
            "connect",
            "--id",
            "8A4F-2B7C-91D0",
            "--host-id",
            "host_abc",
            "--totp",
            "123456",
        ])
        .unwrap();
        match cli.command {
            Commands::Open { id, host_id, .. } => {
                assert_eq!(id, "8A4F-2B7C-91D0");
                assert_eq!(host_id.as_deref(), Some("host_abc"));
            }
            _ => panic!("expected connect command"),
        }
    }
}
