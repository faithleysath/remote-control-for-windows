from __future__ import annotations

import asyncio
import json
import os
import re
import shutil
import subprocess
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any
from urllib.parse import urlparse

import websockets


@dataclass
class TestConfig:
    repo_root: Path
    output_root: Path
    vm_host: str
    share_dir: Path
    server_url: str
    guest_server_url: str
    control_token: str
    totp_period_seconds: int
    start_server: bool
    build_host: bool
    build_gui: bool
    gui_cdp_port: int
    local_cdp_port: int


TestConfig.__test__ = False


@dataclass
class HostRuntime:
    machine_id: str
    host_id: str
    totp: str


@dataclass
class GuiAutomationConnectionInfo:
    server_url: str
    machine_id: str
    host_id: str
    totp: str
    totp_period_seconds: int
    totp_remaining_seconds: int
    listener_status: str
    audit_path: str


@dataclass
class GuiCdpProbe:
    version: dict[str, Any]
    targets: list[dict[str, Any]]
    page_target: dict[str, Any]
    evaluate: dict[str, Any]
    connection_info_response: dict[str, Any]
    connection_info: GuiAutomationConnectionInfo


@dataclass
class ServerHandle:
    process: subprocess.Popen[str]
    pid_file: Path
    stdout_log: Path
    stderr_log: Path
    audit_log: Path


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def env_bool(name: str, default: str = "0") -> bool:
    return os.environ.get(name, default) == "1"


def utc_now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def log(*parts: object) -> None:
    print("[rcw-test]", utc_now(), *parts, flush=True)


def run(
    args: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    capture_output: bool = True,
    check: bool = True,
    text: bool = True,
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        args,
        cwd=cwd,
        env=env,
        capture_output=capture_output,
        check=check,
        text=text,
    )


def render_text(path: Path, replacements: list[tuple[str, str]]) -> str:
    text = path.read_text(encoding="utf-8")
    for old, new in replacements:
        text = text.replace(old, new)
    return text


def server_port(server_url: str) -> int:
    parsed = urlparse(server_url)
    if parsed.port is not None:
        return parsed.port
    return 443 if parsed.scheme == "wss" else 80


def wait_for_file(path: Path, attempts: int = 30, sleep_seconds: float = 1.0) -> None:
    for _ in range(attempts):
        if path.is_file():
            return
        time.sleep(sleep_seconds)
    raise AssertionError(f"timed out waiting for file: {path}")


def wait_http_ready(url: str, attempts: int = 40, sleep_seconds: float = 1.0) -> None:
    import urllib.request

    for _ in range(attempts):
        try:
            with urllib.request.urlopen(url, timeout=2) as resp:
                if resp.status == 200:
                    return
        except Exception:
            time.sleep(sleep_seconds)
    raise AssertionError(f"timed out waiting for HTTP readiness: {url}")


def fetch_json_with_retry(url: str, attempts: int = 20, delay_seconds: float = 1.0) -> Any:
    import urllib.request

    last_error = None
    for _ in range(attempts):
        try:
            with urllib.request.urlopen(url, timeout=5) as resp:
                if resp.status != 200:
                    raise RuntimeError(f"unexpected status={resp.status}")
                return json.loads(resp.read().decode("utf-8"))
        except Exception as error:
            last_error = error
            time.sleep(delay_seconds)
    raise AssertionError(f"failed to fetch {url}: {last_error}")


def ssh(vm_host: str, remote_command: str) -> subprocess.CompletedProcess[str]:
    return run(["ssh", "-o", "BatchMode=yes", vm_host, remote_command])


def copy_if_exists(src: Path, dst: Path) -> None:
    if src.exists():
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, dst)


def copy_console_payload(test_config: TestConfig) -> None:
    shutil.copy2(
        test_config.repo_root / "target/x86_64-pc-windows-msvc/release/rcw-host.exe",
        test_config.share_dir / "rcw-host-debug.exe",
    )


def copy_gui_payload(test_config: TestConfig) -> None:
    shutil.copy2(
        test_config.repo_root / "target/x86_64-pc-windows-msvc/release/rcw-host-gui.exe",
        test_config.share_dir / "rcw-host-gui-debug.exe",
    )


def clear_console_share_artifacts(test_config: TestConfig) -> None:
    for name in (
        "rcw-host-session1-report.txt",
        "rcw-host-session1.stdout.log",
        "rcw-host-session1.stderr.log",
        "rcw-host-session1-audit.jsonl",
    ):
        (test_config.share_dir / name).unlink(missing_ok=True)


def clear_gui_share_artifacts(test_config: TestConfig) -> None:
    for name in (
        "rcw-host-gui-session1-report.txt",
        "rcw-host-gui-session1.stdout.log",
        "rcw-host-gui-session1.stderr.log",
        "rcw-host-gui-session1-audit.jsonl",
    ):
        (test_config.share_dir / name).unlink(missing_ok=True)


def render_console_launcher(test_config: TestConfig, output_dir: Path) -> str:
    source = test_config.repo_root / "scripts/debug/windows/launch-rcw-host-interactive.ps1"
    text = render_text(
        source,
        [
            ("ws://192.168.122.1:17800", test_config.guest_server_url),
            (
                'Stop-Process -Name "rcw-host-debug" -Force -ErrorAction SilentlyContinue',
                'Stop-Process -Name "rcw-host-debug" -Force -ErrorAction SilentlyContinue\n'
                'Stop-Process -Name "rcw-host-gui-debug" -Force -ErrorAction SilentlyContinue',
            ),
        ],
    )
    local_path = output_dir / "launch-rcw-host-interactive.generated.ps1"
    local_path.write_text(text, encoding="utf-8")
    share_name = local_path.name
    shutil.copy2(local_path, test_config.share_dir / share_name)
    return share_name


def render_gui_launcher(test_config: TestConfig, output_dir: Path) -> str:
    source = test_config.repo_root / "scripts/debug/windows/launch-rcw-host-gui-interactive.ps1"
    text = render_text(
        source,
        [
            ("ws://192.168.122.1:17800", test_config.guest_server_url),
            ("$cdpPort = 9222", f"$cdpPort = {test_config.gui_cdp_port}"),
        ],
    )
    local_path = output_dir / "launch-rcw-host-gui-interactive.generated.ps1"
    local_path.write_text(text, encoding="utf-8")
    share_name = local_path.name
    shutil.copy2(local_path, test_config.share_dir / share_name)
    return share_name


def copy_console_evidence(test_config: TestConfig, output_dir: Path) -> None:
    guest_dir = output_dir / "guest"
    guest_dir.mkdir(parents=True, exist_ok=True)
    copy_if_exists(test_config.share_dir / "rcw-host-session1-report.txt", guest_dir / "report.txt")
    copy_if_exists(test_config.share_dir / "rcw-host-session1.stdout.log", guest_dir / "stdout.log")
    copy_if_exists(test_config.share_dir / "rcw-host-session1.stderr.log", guest_dir / "stderr.log")
    copy_if_exists(test_config.share_dir / "rcw-host-session1-audit.jsonl", guest_dir / "audit.jsonl")


def copy_gui_evidence(test_config: TestConfig, output_dir: Path) -> None:
    guest_dir = output_dir / "guest"
    guest_dir.mkdir(parents=True, exist_ok=True)
    copy_if_exists(test_config.share_dir / "rcw-host-gui-session1-report.txt", guest_dir / "report.txt")
    copy_if_exists(test_config.share_dir / "rcw-host-gui-session1.stdout.log", guest_dir / "stdout.log")
    copy_if_exists(test_config.share_dir / "rcw-host-gui-session1.stderr.log", guest_dir / "stderr.log")
    copy_if_exists(test_config.share_dir / "rcw-host-gui-session1-audit.jsonl", guest_dir / "audit.jsonl")


def sync_guest_cleanup_script(test_config: TestConfig) -> str:
    source = test_config.repo_root / "scripts/debug/windows/stop-rcw-host-processes.ps1"
    share_name = source.name
    shutil.copy2(source, test_config.share_dir / share_name)
    return share_name


def cleanup_guest_hosts(test_config: TestConfig, output_dir: Path) -> None:
    share_name = sync_guest_cleanup_script(test_config)
    result = ssh(
        test_config.vm_host,
        f"pwsh -NoProfile -ExecutionPolicy Bypass -File Z:\\{share_name}",
    )
    (output_dir / "guest-cleanup.stdout.txt").write_text(result.stdout, encoding="utf-8")
    (output_dir / "guest-cleanup.stderr.txt").write_text(result.stderr, encoding="utf-8")


def assert_report_session1(report_path: Path) -> None:
    text = report_path.read_text(encoding="utf-8", errors="replace")
    assert "runnerSessionId=1" in text, f"runnerSessionId=1 missing in {report_path}"


def assert_console_report(report_path: Path) -> None:
    text = report_path.read_text(encoding="utf-8", errors="replace")
    assert "runnerSessionId=1" in text, f"runnerSessionId=1 missing in {report_path}"
    assert "childSessionId=1" in text, f"childSessionId=1 missing in {report_path}"
    assert "childName=rcw-host-debug.exe" in text, f"rcw-host-debug.exe missing in {report_path}"


def parse_runtime_values(stdout_log: Path) -> HostRuntime:
    text = stdout_log.read_text(encoding="utf-8", errors="replace")
    machine_match = re.findall(r"^Machine ID: (.+)$", text, re.MULTILINE)
    host_match = re.findall(r"^Host ID: (.+)$", text, re.MULTILINE)
    totp_match = re.findall(r"^Current TOTP: (.+)$", text, re.MULTILINE)
    assert machine_match, f"failed to parse Machine ID from {stdout_log}"
    assert host_match, f"failed to parse Host ID from {stdout_log}"
    assert totp_match, f"failed to parse TOTP from {stdout_log}"
    return HostRuntime(
        machine_id=machine_match[-1].strip(),
        host_id=host_match[-1].strip(),
        totp=totp_match[-1].strip(),
    )


def run_rcwctl_json(
    test_config: TestConfig,
    rcwctl_env: dict[str, str],
    args: list[str],
    output_path: Path,
) -> dict[str, Any]:
    result = run(
        ["cargo", "run", "-q", "-p", "rcwctl", "--", "--json", *args],
        cwd=test_config.repo_root,
        env=rcwctl_env,
    )
    output_path.write_text(result.stdout, encoding="utf-8")
    return json.loads(result.stdout)


def select_cdp_page_target(targets: list[dict[str, Any]]) -> dict[str, Any]:
    page_target = next(
        (
            target
            for target in targets
            if target.get("type") == "page" and target.get("webSocketDebuggerUrl")
        ),
        None,
    ) or next((target for target in targets if target.get("webSocketDebuggerUrl")), None)
    assert page_target is not None, "no CDP page target with webSocketDebuggerUrl found"
    return page_target


async def cdp_runtime_evaluate(
    websocket: websockets.ClientConnection,
    *,
    request_id: int,
    expression: str,
    await_promise: bool = False,
) -> dict[str, Any]:
    await websocket.send(
        json.dumps(
            {
                "id": request_id,
                "method": "Runtime.evaluate",
                "params": {
                    "expression": expression,
                    "awaitPromise": await_promise,
                    "returnByValue": True,
                },
            }
        )
    )
    return json.loads(await websocket.recv())


def parse_gui_connection_info(response: dict[str, Any]) -> GuiAutomationConnectionInfo:
    value = response["result"]["result"]["value"]
    return GuiAutomationConnectionInfo(
        server_url=value["server_url"],
        machine_id=value["machine_id"],
        host_id=value["host_id"],
        totp=value["totp"],
        totp_period_seconds=int(value["totp_period_seconds"]),
        totp_remaining_seconds=int(value["totp_remaining_seconds"]),
        listener_status=value["listener_status"],
        audit_path=value["audit_path"],
    )


async def capture_gui_cdp_probe(cdp_port: int, output_dir: Path) -> GuiCdpProbe:
    version = fetch_json_with_retry(f"http://127.0.0.1:{cdp_port}/json/version")
    targets = fetch_json_with_retry(f"http://127.0.0.1:{cdp_port}/json/list")
    page_target = select_cdp_page_target(targets)

    async with websockets.connect(page_target["webSocketDebuggerUrl"]) as websocket:
        evaluate = await cdp_runtime_evaluate(
            websocket,
            request_id=1,
            expression="({title: document.title, href: location.href, readyState: document.readyState})",
        )
        connection_info_response = await cdp_runtime_evaluate(
            websocket,
            request_id=2,
            expression="""
                (async () => {
                  if (!window.__RCW_HOST_GUI_DEBUG__) {
                    throw new Error("window.__RCW_HOST_GUI_DEBUG__ is missing");
                  }
                  return await window.__RCW_HOST_GUI_DEBUG__.getConnectionInfo();
                })()
            """,
            await_promise=True,
        )
        connection_info = parse_gui_connection_info(connection_info_response)
        if connection_info.totp_remaining_seconds <= 5:
            await asyncio.sleep(connection_info.totp_remaining_seconds + 1)
            connection_info_response = await cdp_runtime_evaluate(
                websocket,
                request_id=3,
                expression="""
                    (async () => {
                      if (!window.__RCW_HOST_GUI_DEBUG__) {
                        throw new Error("window.__RCW_HOST_GUI_DEBUG__ is missing");
                      }
                      return await window.__RCW_HOST_GUI_DEBUG__.getConnectionInfo();
                    })()
                """,
                await_promise=True,
            )
            connection_info = parse_gui_connection_info(connection_info_response)

    (output_dir / "version.json").write_text(json.dumps(version, indent=2) + "\n", encoding="utf-8")
    (output_dir / "targets.json").write_text(json.dumps(targets, indent=2) + "\n", encoding="utf-8")
    (output_dir / "evaluate.json").write_text(json.dumps(evaluate, indent=2) + "\n", encoding="utf-8")
    (output_dir / "connection-info.json").write_text(
        json.dumps(connection_info_response, indent=2) + "\n",
        encoding="utf-8",
    )

    return GuiCdpProbe(
        version=version,
        targets=targets,
        page_target=page_target,
        evaluate=evaluate,
        connection_info_response=connection_info_response,
        connection_info=connection_info,
    )
