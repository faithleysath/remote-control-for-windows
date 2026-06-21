from __future__ import annotations

import os
import shutil
import signal
import subprocess
import time
from pathlib import Path

import pytest

from .harness import GuiAutomationConnectionInfo
from .harness import GuiCdpProbe
from .harness import HostRuntime
from .harness import ServerHandle
from .harness import TestConfig
from .harness import assert_console_report
from .harness import assert_report_session1
from .harness import capture_gui_cdp_probe
from .harness import clear_console_share_artifacts
from .harness import clear_gui_share_artifacts
from .harness import copy_console_evidence
from .harness import copy_console_payload
from .harness import copy_gui_evidence
from .harness import copy_gui_payload
from .harness import cleanup_guest_hosts
from .harness import env_bool
from .harness import log
from .harness import parse_runtime_values
from .harness import render_console_launcher
from .harness import render_gui_launcher
from .harness import repo_root
from .harness import run
from .harness import server_port
from .harness import ssh
from .harness import wait_for_file
from .harness import wait_http_ready


@pytest.fixture(scope="session")
def test_config() -> TestConfig:
    return TestConfig(
        repo_root=repo_root(),
        output_root=Path(os.environ.get("RCW_TEST_OUTPUT_ROOT", "/tmp/rcw-testing/pytest")),
        vm_host=os.environ.get("RCW_TEST_VM_HOST", "win11-main"),
        share_dir=Path(os.environ.get("RCW_TEST_SHARE_DIR", "/data/libvirt/work/share")),
        server_url=os.environ.get("RCW_TEST_SERVER_URL", "ws://127.0.0.1:17800"),
        guest_server_url=os.environ.get("RCW_TEST_GUEST_SERVER_URL", "ws://192.168.122.1:17800"),
        control_token=os.environ.get("RCW_TEST_CONTROL_TOKEN", "debug-token-rcw"),
        totp_period_seconds=int(os.environ.get("RCW_TEST_TOTP_PERIOD_SECONDS", "120")),
        start_server=env_bool("RCW_TEST_START_SERVER"),
        build_host=env_bool("RCW_TEST_BUILD_HOST"),
        build_gui=env_bool("RCW_TEST_BUILD_GUI"),
        gui_cdp_port=int(os.environ.get("RCW_TEST_GUI_CDP_PORT", "9222")),
        local_cdp_port=int(os.environ.get("RCW_TEST_LOCAL_CDP_PORT", "9223")),
    )


@pytest.fixture(scope="session", autouse=True)
def ensure_output_root(test_config: TestConfig) -> Path:
    test_config.output_root.mkdir(parents=True, exist_ok=True)
    return test_config.output_root


@pytest.fixture(scope="session")
def server_handle(test_config: TestConfig) -> ServerHandle | None:
    if not test_config.start_server:
        yield None
        return

    server_dir = test_config.output_root / "server"
    server_dir.mkdir(parents=True, exist_ok=True)
    stdout_log = server_dir / "stdout.log"
    stderr_log = server_dir / "stderr.log"
    audit_log = server_dir / "audit.jsonl"
    pid_file = server_dir / "server.pid"

    env = os.environ.copy()
    env["RCW_BIND_ADDR"] = f"0.0.0.0:{server_port(test_config.server_url)}"
    env["RCW_CONTROL_TOKEN"] = test_config.control_token
    env["RCW_AUDIT_LOG"] = str(audit_log)

    log("starting rcw-server on", test_config.server_url)
    stdout_fh = stdout_log.open("w", encoding="utf-8")
    stderr_fh = stderr_log.open("w", encoding="utf-8")
    proc = subprocess.Popen(
        ["cargo", "run", "-q", "-p", "rcw-server"],
        cwd=test_config.repo_root,
        env=env,
        stdout=stdout_fh,
        stderr=stderr_fh,
        text=True,
    )
    pid_file.write_text(f"{proc.pid}\n", encoding="utf-8")

    try:
        wait_http_ready(f"http://127.0.0.1:{server_port(test_config.server_url)}/healthz")
    except Exception:
        proc.terminate()
        raise

    handle = ServerHandle(
        process=proc,
        pid_file=pid_file,
        stdout_log=stdout_log,
        stderr_log=stderr_log,
        audit_log=audit_log,
    )
    yield handle

    if proc.poll() is None:
        proc.terminate()
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=10)
    stdout_fh.close()
    stderr_fh.close()


@pytest.fixture(scope="session")
def build_console_if_requested(test_config: TestConfig) -> None:
    if not test_config.build_host:
        return
    log("building rcw-host for Windows")
    env = os.environ.copy()
    env["RUSTFLAGS"] = "-C target-feature=+crt-static"
    run(
        [
            "cargo",
            "xwin",
            "build",
            "-q",
            "-p",
            "rcw-host",
            "--target",
            "x86_64-pc-windows-msvc",
            "--release",
        ],
        cwd=test_config.repo_root,
        env=env,
    )


@pytest.fixture(scope="session")
def build_gui_if_requested(test_config: TestConfig) -> None:
    if not test_config.build_gui:
        return
    log("building rcw-host-gui for Windows")
    run(
        ["npm", "--prefix", "crates/rcw-host-gui", "run", "tauri:build:windows:x64"],
        cwd=test_config.repo_root,
    )


@pytest.fixture
def smoke_output_dir(test_config: TestConfig, request: pytest.FixtureRequest) -> Path:
    path = test_config.output_root / request.node.name.replace("[", "_").replace("]", "_")
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True, exist_ok=True)
    return path


@pytest.fixture
def console_host_runtime(
    test_config: TestConfig,
    server_handle: ServerHandle | None,
    build_console_if_requested: None,
    smoke_output_dir: Path,
) -> HostRuntime:
    del server_handle
    copy_console_payload(test_config)
    clear_console_share_artifacts(test_config)
    launcher_name = render_console_launcher(test_config, smoke_output_dir)

    log("launching interactive console host in", test_config.vm_host)
    launch_result = ssh(
        test_config.vm_host,
        f"pwsh -NoProfile -ExecutionPolicy Bypass -File Z:\\{launcher_name}",
    )
    (smoke_output_dir / "guest-launch.stdout.txt").write_text(launch_result.stdout, encoding="utf-8")
    (smoke_output_dir / "guest-launch.stderr.txt").write_text(launch_result.stderr, encoding="utf-8")

    wait_for_file(test_config.share_dir / "rcw-host-session1-report.txt")
    copy_console_evidence(test_config, smoke_output_dir)
    assert_console_report(smoke_output_dir / "guest/report.txt")
    runtime = parse_runtime_values(smoke_output_dir / "guest/stdout.log")
    try:
        yield runtime
    finally:
        cleanup_guest_hosts(test_config, smoke_output_dir)
        copy_console_evidence(test_config, smoke_output_dir)


@pytest.fixture
def gui_host_output(
    test_config: TestConfig,
    server_handle: ServerHandle | None,
    build_gui_if_requested: None,
    smoke_output_dir: Path,
) -> Path:
    del server_handle
    copy_gui_payload(test_config)
    clear_gui_share_artifacts(test_config)
    launcher_name = render_gui_launcher(test_config, smoke_output_dir)

    log("launching interactive GUI host in", test_config.vm_host)
    launch_result = ssh(
        test_config.vm_host,
        f"pwsh -NoProfile -ExecutionPolicy Bypass -File Z:\\{launcher_name}",
    )
    (smoke_output_dir / "guest-launch.stdout.txt").write_text(launch_result.stdout, encoding="utf-8")
    (smoke_output_dir / "guest-launch.stderr.txt").write_text(launch_result.stderr, encoding="utf-8")

    wait_for_file(test_config.share_dir / "rcw-host-gui-session1-report.txt")
    copy_gui_evidence(test_config, smoke_output_dir)
    assert_report_session1(smoke_output_dir / "guest/report.txt")
    try:
        yield smoke_output_dir
    finally:
        cleanup_guest_hosts(test_config, smoke_output_dir)
        copy_gui_evidence(test_config, smoke_output_dir)


@pytest.fixture
def rcwctl_env(test_config: TestConfig) -> dict[str, str]:
    env = os.environ.copy()
    env["RCW_SERVER_URL"] = test_config.server_url
    env["RCW_CONTROL_TOKEN"] = test_config.control_token
    return env


@pytest.fixture
def cdp_tunnel(test_config: TestConfig, gui_host_output: Path) -> int:
    del gui_host_output
    log("opening local SSH tunnel for WebView2 CDP")
    proc = subprocess.Popen(
        [
            "ssh",
            "-o",
            "BatchMode=yes",
            "-o",
            "ExitOnForwardFailure=yes",
            "-N",
            "-L",
            f"127.0.0.1:{test_config.local_cdp_port}:127.0.0.1:{test_config.gui_cdp_port}",
            test_config.vm_host,
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    time.sleep(2)
    try:
        yield test_config.local_cdp_port
    finally:
        if proc.poll() is None:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=10)


@pytest.fixture
async def gui_cdp_probe(gui_host_output: Path, cdp_tunnel: int) -> GuiCdpProbe:
    return await capture_gui_cdp_probe(cdp_tunnel, gui_host_output)


@pytest.fixture
async def gui_host_runtime(gui_cdp_probe: GuiCdpProbe) -> GuiAutomationConnectionInfo:
    return gui_cdp_probe.connection_info
