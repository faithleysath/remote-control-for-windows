from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from mcp.client.session import ClientSession
from mcp.client.stdio import StdioServerParameters, stdio_client

from .harness import HostRuntime
from .harness import TestConfig


@pytest.mark.windows_vm
@pytest.mark.smoke
@pytest.mark.asyncio
async def test_mcp_smoke(
    test_config: TestConfig,
    smoke_output_dir: Path,
    console_host_runtime: HostRuntime,
) -> None:
    screenshot_path = smoke_output_dir / "mcp-host.png"
    stderr_log = (smoke_output_dir / "mcp.stderr.log").open("w", encoding="utf-8")
    server = StdioServerParameters(
        command="cargo",
        args=["run", "-q", "-p", "rcwctl", "--", "mcp"],
        cwd=str(test_config.repo_root),
        env={
            **dict(os.environ),
            "RCW_SERVER_URL": test_config.server_url,
            "RCW_CONTROL_TOKEN": test_config.control_token,
        },
    )

    try:
        async with stdio_client(server, errlog=stderr_log) as (read_stream, write_stream):
            async with ClientSession(read_stream, write_stream) as session:
                initialize = await session.initialize()
                tools = await session.list_tools()
                connect = await session.call_tool(
                    "connect",
                    {
                        "machine_id": console_host_runtime.machine_id,
                        "host_id": console_host_runtime.host_id,
                        "totp": console_host_runtime.totp,
                    },
                )
                status = await session.call_tool("status", {})
                exec_result = await session.call_tool(
                    "exec",
                    {
                        "program": "pwsh",
                        "argv": ["-NoProfile", "-Command", "hostname"],
                    },
                )
                windows = await session.call_tool("windows", {})
                screenshot = await session.call_tool(
                    "screenshot",
                    {
                        "output_path": str(screenshot_path),
                        "overwrite": True,
                    },
                )
                disconnect = await session.call_tool("disconnect", {})
    finally:
        stderr_log.close()

    summary = {
        "initialize": initialize.model_dump(mode="json", by_alias=True),
        "tool_count": len(tools.tools),
        "tool_names": [tool.name for tool in tools.tools],
        "connect": connect.model_dump(mode="json", by_alias=True),
        "status": status.model_dump(mode="json", by_alias=True),
        "exec": exec_result.model_dump(mode="json", by_alias=True),
        "windows": windows.model_dump(mode="json", by_alias=True),
        "screenshot": screenshot.model_dump(mode="json", by_alias=True),
        "disconnect": disconnect.model_dump(mode="json", by_alias=True),
    }
    (smoke_output_dir / "mcp-summary.json").write_text(
        json.dumps(summary, indent=2) + "\n",
        encoding="utf-8",
    )

    connect_sc = connect.structuredContent or {}
    status_sc = status.structuredContent or {}
    exec_sc = exec_result.structuredContent or {}
    windows_sc = windows.structuredContent or {}
    screenshot_sc = screenshot.structuredContent or {}
    disconnect_sc = disconnect.structuredContent or {}

    assert connect_sc.get("ok") is True
    assert status_sc.get("ok") is True
    assert status_sc.get("host_online") is True
    assert status_sc.get("session_active") is True
    assert exec_sc.get("status") == "completed" or (exec_sc.get("complete") or {}).get("ok") is True
    assert isinstance(windows_sc.get("windows"), list)
    assert screenshot_sc.get("ok") is True
    assert screenshot_path.is_file()
    assert disconnect_sc.get("ok") is True
