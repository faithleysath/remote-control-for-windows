from __future__ import annotations

import json

import pytest

from .harness import HostRuntime
from .harness import TestConfig
from .harness import run_rcwctl_json


@pytest.mark.windows_vm
@pytest.mark.smoke
def test_console_smoke(
    test_config: TestConfig,
    smoke_output_dir,
    console_host_runtime: HostRuntime,
    rcwctl_env: dict[str, str],
) -> None:
    connect = run_rcwctl_json(
        test_config,
        rcwctl_env,
        [
            "connect",
            "--id",
            console_host_runtime.machine_id,
            "--host-id",
            console_host_runtime.host_id,
            "--totp",
            console_host_runtime.totp,
        ],
        smoke_output_dir / "connect.json",
    )
    status = run_rcwctl_json(test_config, rcwctl_env, ["status"], smoke_output_dir / "status.json")
    windows = run_rcwctl_json(test_config, rcwctl_env, ["windows"], smoke_output_dir / "windows.json")
    screenshot = run_rcwctl_json(
        test_config,
        rcwctl_env,
        ["screenshot", "--output", str(smoke_output_dir / "console-host.png")],
        smoke_output_dir / "screenshot.json",
    )
    exec_result = run_rcwctl_json(
        test_config,
        rcwctl_env,
        ["exec", "--", "pwsh", "-NoProfile", "-Command", "hostname"],
        smoke_output_dir / "exec.json",
    )
    disconnect = run_rcwctl_json(
        test_config,
        rcwctl_env,
        ["disconnect"],
        smoke_output_dir / "disconnect.json",
    )

    assert connect["ok"] is True
    assert status["ok"] is True
    assert status["host_online"] is True
    assert status["session_active"] is True
    assert windows["ok"] is True
    assert len(windows["windows"]) >= 1
    assert screenshot["ok"] is True
    assert (smoke_output_dir / "console-host.png").is_file()
    assert exec_result["status"] == "completed"
    assert exec_result["complete"]["ok"] is True
    assert "stdout" in exec_result
    assert disconnect["ok"] is True

    summary = {
        "machine_id": console_host_runtime.machine_id,
        "host_id": console_host_runtime.host_id,
        "connect_request_id": connect["request_id"],
        "status_request_id": status["request_id"],
        "windows_count": len(windows["windows"]),
        "screenshot_path": str(smoke_output_dir / "console-host.png"),
        "exec_task_id": exec_result["task_id"],
    }
    (smoke_output_dir / "summary.json").write_text(
        json.dumps(summary, indent=2) + "\n",
        encoding="utf-8",
    )
