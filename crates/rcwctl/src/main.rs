use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, Context, Result};
use clap::{Parser, Subcommand};
use directories::ProjectDirs;
use futures_util::{SinkExt, StreamExt};
use rcw_common::{
    audit::{append_jsonl, AuditEvent},
    config,
    ids::new_request_id,
    protocol::{
        CommandCompletePayload, CommandOutputPayload, CommandRequestPayload, ControlOpenPayload,
        ControlOpenResultPayload, DownloadArgs, ErrorPayload, ExecArgs, KeyboardKeyArgs,
        KeyboardTypeArgs, MouseClickArgs, MouseMoveArgs, MouseScrollArgs, ScreenshotArgs,
        SessionClosePayload, SessionCloseResultPayload, SessionStatusPayload,
        SessionStatusResultPayload, UploadArgs, WindowInfo, WireMessage, COMMAND_DOWNLOAD_BEGIN,
        COMMAND_EXEC, COMMAND_KEYBOARD_KEY, COMMAND_KEYBOARD_TYPE, COMMAND_MOUSE_CLICK,
        COMMAND_MOUSE_MOVE, COMMAND_MOUSE_SCROLL, COMMAND_SCREENSHOT, COMMAND_UPLOAD_BEGIN,
        COMMAND_WINDOWS, PROTOCOL_VERSION, TYPE_COMMAND_COMPLETE, TYPE_COMMAND_OUTPUT,
        TYPE_COMMAND_REQUEST, TYPE_CONTROL_OPEN, TYPE_DOWNLOAD_COMPLETE, TYPE_ERROR,
        TYPE_SESSION_CLOSE, TYPE_SESSION_STATUS, TYPE_UPLOAD_COMPLETE,
    },
    transfer::{chunk_binary, sha256_bytes, sha256_file, BinaryFrame, BinaryKind},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};

type WsStream = futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>;
type WsSink = futures_util::stream::SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>;

#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    #[arg(long, global = true)]
    server: Option<String>,
    #[arg(long, global = true)]
    token: Option<String>,
    #[arg(long, global = true)]
    session: Option<PathBuf>,
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true)]
    timeout: Option<String>,
    #[arg(long, global = true)]
    audit_label: Option<String>,
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Open {
        #[arg(long = "id")]
        id: String,
        #[arg(long)]
        totp: String,
        #[arg(long)]
        totp_period_seconds: Option<u64>,
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
    Move {
        #[arg(long)]
        x: i32,
        #[arg(long)]
        y: i32,
    },
    Click {
        #[arg(long)]
        x: i32,
        #[arg(long)]
        y: i32,
        #[arg(long, default_value = "left")]
        button: String,
    },
    Scroll {
        #[arg(long)]
        delta: i32,
    },
    Type {
        text: String,
    },
    Key {
        key: String,
    },
    Close,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionFile {
    server: String,
    machine_id: String,
    session_id: String,
    session_token: String,
    created_at: String,
    last_used_at: String,
}

#[derive(Debug, Default)]
struct CommandResponse {
    stdout: String,
    stderr: String,
    file: Vec<u8>,
    json_stream: String,
    complete: Option<CommandCompletePayload>,
}

enum IncomingFrame {
    Text(WireMessage),
    Binary(Vec<u8>),
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().compact().init();
    let code = match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("rcwctl: {err:#}");
            1
        }
    };
    std::process::exit(code);
}

async fn run() -> Result<i32> {
    let cli = Cli::parse();
    let started = Instant::now();
    let request_id = new_request_id();

    let result = match &cli.command {
        Commands::Open {
            id,
            totp,
            totp_period_seconds,
        } => open_session(&cli, &request_id, id, totp, *totp_period_seconds).await,
        Commands::Status => status_session(&cli, &request_id).await,
        Commands::Exec { command } => exec_command(&cli, &request_id, command).await,
        Commands::Upload {
            local,
            remote,
            overwrite,
            sha256,
        } => {
            upload_file(
                &cli,
                &request_id,
                local,
                remote,
                *overwrite,
                sha256.as_deref(),
            )
            .await
        }
        Commands::Download { remote, local } => {
            download_file(&cli, &request_id, remote, local).await
        }
        Commands::Screenshot {
            output,
            display,
            format,
        } => screenshot(&cli, &request_id, output, *display, format).await,
        Commands::Windows => windows(&cli, &request_id).await,
        Commands::Move { x, y } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_MOUSE_MOVE,
                json!(MouseMoveArgs { x: *x, y: *y }),
            )
            .await
        }
        Commands::Click { x, y, button } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_MOUSE_CLICK,
                json!(MouseClickArgs {
                    x: *x,
                    y: *y,
                    button: button.clone()
                }),
            )
            .await
        }
        Commands::Scroll { delta } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_MOUSE_SCROLL,
                json!(MouseScrollArgs { delta: *delta }),
            )
            .await
        }
        Commands::Type { text } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_KEYBOARD_TYPE,
                json!(KeyboardTypeArgs { text: text.clone() }),
            )
            .await
        }
        Commands::Key { key } => {
            simple_command(
                &cli,
                &request_id,
                COMMAND_KEYBOARD_KEY,
                json!(KeyboardKeyArgs { key: key.clone() }),
            )
            .await
        }
        Commands::Close => close_session(&cli, &request_id).await,
    };

    let audit_result = if result.is_ok() { "ok" } else { "failed" };
    append_controller_audit(
        &cli,
        &request_id,
        command_name(&cli.command),
        audit_result,
        started.elapsed().as_millis() as u64,
        None,
    );
    result
}

async fn open_session(
    cli: &Cli,
    request_id: &str,
    machine_id: &str,
    totp: &str,
    explicit_period: Option<u64>,
) -> Result<i32> {
    let server = config::resolve_server_url(cli.server.as_deref())?;
    let token = config::control_token(cli.token.as_deref())?;
    let period = config::resolve_totp_period_seconds(explicit_period)?;
    let message = WireMessage::new(
        TYPE_CONTROL_OPEN,
        Some(request_id.to_owned()),
        None,
        ControlOpenPayload {
            protocol_version: PROTOCOL_VERSION,
            control_token: token,
            machine_id: machine_id.to_owned(),
            totp: totp.to_owned(),
            totp_period_seconds: period,
        },
    )?;
    let messages = send_and_collect(
        &server,
        message,
        &[rcw_common::protocol::TYPE_CONTROL_OPEN_RESULT],
        wait_timeout(cli)?,
    )
    .await?;
    let result: ControlOpenResultPayload = last_payload(&messages)?;
    let now = rcw_common::audit::now_rfc3339();
    let session = SessionFile {
        server: server.clone(),
        machine_id: result.machine_id.clone(),
        session_id: result.session_id.clone(),
        session_token: result.session_token,
        created_at: now.clone(),
        last_used_at: now,
    };
    write_session(cli, &session)?;

    if cli.json {
        print_json(json!({
            "ok": true,
            "session_id": result.session_id,
            "machine_id": result.machine_id,
            "server": server,
            "request_id": request_id,
        }))?;
    } else {
        println!(
            "opened session {} for {} ({server})",
            result.session_id, result.machine_id
        );
        println!("request_id: {request_id}");
    }
    Ok(0)
}

async fn status_session(cli: &Cli, request_id: &str) -> Result<i32> {
    let session = read_session(cli)?;
    let message = WireMessage::new(
        TYPE_SESSION_STATUS,
        Some(request_id.to_owned()),
        Some(session.session_id.clone()),
        SessionStatusPayload {
            session_token: session.session_token.clone(),
        },
    )?;
    let messages = send_and_collect(
        &session.server,
        message,
        &[rcw_common::protocol::TYPE_SESSION_STATUS_RESULT],
        wait_timeout(cli)?,
    )
    .await?;
    let result: SessionStatusResultPayload = last_payload(&messages)?;
    touch_session(cli, session)?;

    if cli.json {
        print_json(json!({
            "ok": result.ok,
            "machine_id": result.machine_id,
            "host_online": result.host_online,
            "session_active": result.session_active,
            "request_id": request_id,
        }))?;
    } else {
        println!("machine_id: {}", result.machine_id);
        println!("host_online: {}", result.host_online);
        println!("session_active: {}", result.session_active);
        println!("request_id: {request_id}");
    }
    Ok(
        if result.ok && result.host_online && result.session_active {
            0
        } else {
            1
        },
    )
}

async fn close_session(cli: &Cli, request_id: &str) -> Result<i32> {
    let session = read_session(cli)?;
    let message = WireMessage::new(
        TYPE_SESSION_CLOSE,
        Some(request_id.to_owned()),
        Some(session.session_id.clone()),
        SessionClosePayload {
            session_token: session.session_token.clone(),
        },
    )?;
    let messages = send_and_collect(
        &session.server,
        message,
        &[rcw_common::protocol::TYPE_SESSION_CLOSE_RESULT],
        wait_timeout(cli)?,
    )
    .await?;
    let result: SessionCloseResultPayload = last_payload(&messages)?;
    remove_session(cli)?;

    if cli.json {
        print_json(json!({
            "ok": result.ok,
            "session_id": result.session_id,
            "request_id": request_id,
        }))?;
    } else {
        println!("closed session {}", result.session_id);
        println!("request_id: {request_id}");
    }
    Ok(0)
}

async fn exec_command(cli: &Cli, request_id: &str, command: &[String]) -> Result<i32> {
    if command.is_empty() {
        bail!("exec requires a program");
    }
    let wait = wait_timeout(cli)?;
    let remote_timeout_ms = wait.as_millis().min(u64::MAX as u128) as u64;
    let response = send_command(
        cli,
        request_id,
        COMMAND_EXEC,
        json!(ExecArgs {
            program: command[0].clone(),
            argv: command[1..].to_vec(),
            cwd: None,
            timeout_ms: remote_timeout_ms,
        }),
        wait + Duration::from_secs(10),
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;

    if cli.json {
        print_json(json!({
            "ok": complete.ok,
            "exit_code": complete.exit_code,
            "stdout": response.stdout,
            "stderr": response.stderr,
            "duration_ms": complete.duration_ms,
            "request_id": request_id,
        }))?;
    } else {
        print!("{}", response.stdout);
        eprint!("{}", response.stderr);
        eprintln!("request_id: {request_id}");
    }
    Ok(complete
        .exit_code
        .unwrap_or(if complete.ok { 0 } else { 1 }))
}

async fn upload_file(
    cli: &Cli,
    request_id: &str,
    local: &Path,
    remote: &str,
    overwrite: bool,
    expected_sha256: Option<&str>,
) -> Result<i32> {
    let bytes = fs::read(local).with_context(|| format!("failed to read {}", local.display()))?;
    let actual = sha256_bytes(&bytes);
    if let Some(expected) = expected_sha256 {
        if expected != actual {
            bail!("local sha256 mismatch: expected {expected}, calculated {actual}");
        }
    }
    let response = send_command_with_frames_and_terminal(
        cli,
        request_id,
        COMMAND_UPLOAD_BEGIN,
        json!(UploadArgs {
            remote_path: remote.to_owned(),
            overwrite,
            sha256: actual.clone(),
            size: bytes.len() as u64,
        }),
        chunk_binary(request_id, BinaryKind::UploadChunk, &bytes)?,
        &[TYPE_UPLOAD_COMPLETE],
        wait_timeout(cli)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;

    if cli.json {
        print_json(json!({
            "ok": complete.ok,
            "remote": remote,
            "size": complete.size,
            "sha256": complete.sha256,
            "request_id": request_id,
        }))?;
    } else {
        println!("uploaded {} -> {remote}", local.display());
        println!("sha256: {}", complete.sha256.unwrap_or(actual));
        println!("request_id: {request_id}");
    }
    Ok(if complete.ok { 0 } else { 1 })
}

async fn download_file(cli: &Cli, request_id: &str, remote: &str, local: &Path) -> Result<i32> {
    let response = send_command_with_terminal(
        cli,
        request_id,
        COMMAND_DOWNLOAD_BEGIN,
        json!(DownloadArgs {
            remote_path: remote.to_owned()
        }),
        &[TYPE_DOWNLOAD_COMPLETE],
        wait_timeout(cli)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    write_output_file(local, &response.file)?;
    if let Some(expected) = &complete.sha256 {
        let actual = sha256_file(local)?;
        if &actual != expected {
            bail!("download checksum mismatch: expected {expected}, calculated {actual}");
        }
    }

    if cli.json {
        print_json(json!({
            "ok": complete.ok,
            "remote": remote,
            "output": local,
            "size": complete.size,
            "sha256": complete.sha256,
            "request_id": request_id,
        }))?;
    } else {
        println!("downloaded {remote} -> {}", local.display());
        if let Some(sha256) = complete.sha256 {
            println!("sha256: {sha256}");
        }
        println!("request_id: {request_id}");
    }
    Ok(if complete.ok { 0 } else { 1 })
}

async fn screenshot(
    cli: &Cli,
    request_id: &str,
    output: &Path,
    display: Option<u32>,
    format: &str,
) -> Result<i32> {
    let response = send_command(
        cli,
        request_id,
        COMMAND_SCREENSHOT,
        json!(ScreenshotArgs {
            display,
            format: format.to_owned(),
        }),
        wait_timeout(cli)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    write_output_file(output, &response.file)?;
    if let Some(expected) = &complete.sha256 {
        let actual = sha256_file(output)?;
        if &actual != expected {
            bail!("screenshot checksum mismatch: expected {expected}, calculated {actual}");
        }
    }

    if cli.json {
        print_json(json!({
            "ok": complete.ok,
            "output": output,
            "size": complete.size,
            "sha256": complete.sha256,
            "request_id": request_id,
        }))?;
    } else {
        println!("wrote screenshot {}", output.display());
        println!("request_id: {request_id}");
    }
    Ok(if complete.ok { 0 } else { 1 })
}

async fn windows(cli: &Cli, request_id: &str) -> Result<i32> {
    let response = send_command(
        cli,
        request_id,
        COMMAND_WINDOWS,
        json!({}),
        wait_timeout(cli)?,
    )
    .await?;
    let complete = response.complete.context("missing command.complete")?;
    if cli.json {
        let windows: Value = serde_json::from_str(&response.json_stream)?;
        print_json(json!({
            "ok": complete.ok,
            "windows": windows,
            "request_id": request_id,
        }))?;
    } else {
        let windows: Vec<WindowInfo> = serde_json::from_str(&response.json_stream)?;
        for window in windows {
            println!(
                "{} pid={} visible={} focused={} title={}",
                window.handle, window.process_id, window.visible, window.focused, window.title
            );
        }
        println!("request_id: {request_id}");
    }
    Ok(if complete.ok { 0 } else { 1 })
}

async fn simple_command(cli: &Cli, request_id: &str, command: &str, args: Value) -> Result<i32> {
    let response = send_command(cli, request_id, command, args, wait_timeout(cli)?).await?;
    let complete = response.complete.context("missing command.complete")?;
    if cli.json {
        print_json(json!({
            "ok": complete.ok,
            "summary": complete.summary,
            "request_id": request_id,
        }))?;
    } else {
        println!("{}", complete.summary.unwrap_or_else(|| "ok".to_owned()));
        println!("request_id: {request_id}");
    }
    Ok(if complete.ok { 0 } else { 1 })
}

async fn send_command(
    cli: &Cli,
    request_id: &str,
    command: &str,
    args: Value,
    wait: Duration,
) -> Result<CommandResponse> {
    send_command_with_terminal(
        cli,
        request_id,
        command,
        args,
        &[TYPE_COMMAND_COMPLETE],
        wait,
    )
    .await
}

async fn send_command_with_terminal(
    cli: &Cli,
    request_id: &str,
    command: &str,
    args: Value,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<CommandResponse> {
    send_command_with_frames_and_terminal(
        cli,
        request_id,
        command,
        args,
        Vec::new(),
        terminal_kinds,
        wait,
    )
    .await
}

async fn send_command_with_frames_and_terminal(
    cli: &Cli,
    request_id: &str,
    command: &str,
    args: Value,
    binary_frames: Vec<Vec<u8>>,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<CommandResponse> {
    let mut session = read_session(cli)?;
    let payload = CommandRequestPayload {
        session_token: session.session_token.clone(),
        command: command.to_owned(),
        audit_label: cli.audit_label.clone(),
        args,
    };
    let message = WireMessage::new(
        TYPE_COMMAND_REQUEST,
        Some(request_id.to_owned()),
        Some(session.session_id.clone()),
        payload,
    )?;
    let messages = send_and_collect_with_binary(
        &session.server,
        message,
        binary_frames,
        terminal_kinds,
        wait,
    )
    .await?;
    session.last_used_at = rcw_common::audit::now_rfc3339();
    write_session(cli, &session)?;
    command_response(messages)
}

async fn send_and_collect(
    server: &str,
    message: WireMessage,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<Vec<IncomingFrame>> {
    send_and_collect_with_binary(server, message, Vec::new(), terminal_kinds, wait).await
}

async fn send_and_collect_with_binary(
    server: &str,
    message: WireMessage,
    binary_frames: Vec<Vec<u8>>,
    terminal_kinds: &[&str],
    wait: Duration,
) -> Result<Vec<IncomingFrame>> {
    let (mut sink, mut stream) = connect_control(server).await?;
    send_json(&mut sink, message).await?;
    for frame in binary_frames {
        sink.send(Message::Binary(frame)).await?;
    }

    let mut messages = Vec::new();
    loop {
        let frame = next_message(&mut stream, wait).await?;
        let terminal = match &frame {
            IncomingFrame::Text(message) => {
                if message.kind == TYPE_ERROR {
                    let error: ErrorPayload = message.payload_as()?;
                    bail!("{:?}: {}", error.code, error.message);
                }
                terminal_kinds.iter().any(|kind| *kind == message.kind)
            }
            IncomingFrame::Binary(_) => false,
        };
        messages.push(frame);
        if terminal {
            return Ok(messages);
        }
    }
}

async fn connect_control(server: &str) -> Result<(WsSink, WsStream)> {
    let url = config::ws_endpoint_url(server, "/ws/control")?;
    let (ws, _) = connect_async(url)
        .await
        .context("failed to connect to rcw-server control websocket")?;
    Ok(ws.split())
}

async fn send_json(sink: &mut WsSink, message: WireMessage) -> Result<()> {
    sink.send(Message::Text(serde_json::to_string(&message)?))
        .await?;
    Ok(())
}

async fn next_message(stream: &mut WsStream, wait: Duration) -> Result<IncomingFrame> {
    loop {
        let frame = timeout(wait, stream.next())
            .await
            .context("timed out waiting for server response")?
            .ok_or_else(|| anyhow!("server closed control websocket"))??;
        match frame {
            Message::Text(text) => return Ok(IncomingFrame::Text(serde_json::from_str(&text)?)),
            Message::Binary(bytes) => return Ok(IncomingFrame::Binary(bytes)),
            Message::Close(_) => bail!("server closed control websocket"),
            Message::Ping(_) | Message::Pong(_) => {}
            Message::Frame(_) => {}
        }
    }
}

fn command_response(messages: Vec<IncomingFrame>) -> Result<CommandResponse> {
    let mut response = CommandResponse::default();
    for frame in messages {
        match frame {
            IncomingFrame::Text(message) => match message.kind.as_str() {
                TYPE_COMMAND_OUTPUT => {
                    let output: CommandOutputPayload = message.payload_as()?;
                    match output.stream.as_str() {
                        "stdout" => response.stdout.push_str(&output.data),
                        "stderr" => response.stderr.push_str(&output.data),
                        "json" => response.json_stream.push_str(&output.data),
                        _ => {}
                    }
                }
                TYPE_COMMAND_COMPLETE | TYPE_UPLOAD_COMPLETE | TYPE_DOWNLOAD_COMPLETE => {
                    response.complete = Some(message.payload_as()?);
                }
                _ => {}
            },
            IncomingFrame::Binary(bytes) => {
                let frame = BinaryFrame::decode(&bytes)?;
                match frame.kind {
                    BinaryKind::DownloadChunk | BinaryKind::ScreenshotChunk => {
                        response.file.extend_from_slice(&frame.payload);
                    }
                    BinaryKind::UploadChunk => {}
                }
            }
        }
    }
    Ok(response)
}

fn last_payload<T>(messages: &[IncomingFrame]) -> Result<T>
where
    T: for<'de> Deserialize<'de>,
{
    let message = messages
        .iter()
        .rev()
        .find_map(|frame| match frame {
            IncomingFrame::Text(message) => Some(message),
            IncomingFrame::Binary(_) => None,
        })
        .ok_or_else(|| anyhow!("missing response message"))?;
    Ok(message.payload_as()?)
}

fn session_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.session {
        return Ok(path.clone());
    }
    Ok(project_dirs()?.data_dir().join("session.json"))
}

fn audit_path() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().join("audit.jsonl"))
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", "rcwctl").ok_or_else(|| anyhow!("failed to resolve app data dir"))
}

fn read_session(cli: &Cli) -> Result<SessionFile> {
    let path = session_path(cli)?;
    let data = fs::read_to_string(&path)
        .with_context(|| format!("failed to read session file {}", path.display()))?;
    Ok(serde_json::from_str(&data)?)
}

fn write_session(cli: &Cli, session: &SessionFile) -> Result<()> {
    let path = session_path(cli)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_vec_pretty(session)?;
    fs::write(&path, data)?;
    restrict_user_only(&path)?;
    Ok(())
}

fn touch_session(cli: &Cli, mut session: SessionFile) -> Result<()> {
    session.last_used_at = rcw_common::audit::now_rfc3339();
    write_session(cli, &session)
}

fn remove_session(cli: &Cli) -> Result<()> {
    let path = session_path(cli)?;
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err.into()),
    }
}

#[cfg(unix)]
fn restrict_user_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_user_only(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_output_file(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, bytes)?;
    Ok(())
}

fn wait_timeout(cli: &Cli) -> Result<Duration> {
    match &cli.timeout {
        Some(value) => parse_duration(value),
        None => Ok(Duration::from_secs(30)),
    }
}

fn parse_duration(value: &str) -> Result<Duration> {
    let value = value.trim();
    if let Some(ms) = value.strip_suffix("ms") {
        return Ok(Duration::from_millis(ms.parse()?));
    }
    if let Some(seconds) = value.strip_suffix('s') {
        return Ok(Duration::from_secs(seconds.parse()?));
    }
    if let Some(minutes) = value.strip_suffix('m') {
        return Ok(Duration::from_secs(minutes.parse::<u64>()? * 60));
    }
    Ok(Duration::from_secs(value.parse()?))
}

fn print_json(value: Value) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn append_controller_audit(
    cli: &Cli,
    request_id: &str,
    command: &str,
    result: &str,
    duration_ms: u64,
    summary: Option<String>,
) {
    let mut event = AuditEvent::new("controller", "command.invoked");
    event.request_id = Some(request_id.to_owned());
    event.command = Some(command.to_owned());
    event.audit_label = cli.audit_label.clone();
    event.result = Some(result.to_owned());
    event.duration_ms = Some(duration_ms);
    event.summary = summary;
    if let Ok(path) = audit_path() {
        let _ = append_jsonl(path, &event);
    }
}

fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::Open { .. } => "open",
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
        Commands::Close => "close",
    }
}
