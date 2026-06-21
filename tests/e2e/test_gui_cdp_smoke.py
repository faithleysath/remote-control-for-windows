from __future__ import annotations

import json

import pytest


@pytest.mark.windows_vm
@pytest.mark.smoke
@pytest.mark.asyncio
async def test_gui_cdp_smoke(gui_host_output, gui_cdp_probe) -> None:
    output_dir = gui_host_output
    probe = gui_cdp_probe
    version = probe.version
    page_target = probe.page_target
    evaluate_value = probe.evaluate["result"]["result"]["value"]
    connection_info = probe.connection_info

    assert version["Browser"]
    assert evaluate_value["readyState"] in {"interactive", "complete"}
    assert connection_info.machine_id
    assert connection_info.host_id
    assert connection_info.totp
    assert connection_info.listener_status in {
        "stopped",
        "connecting",
        "connected",
        "reconnecting",
        "stopping",
        "error",
    }

    summary = {
        "browser": version["Browser"],
        "target_title": page_target.get("title"),
        "target_url": page_target.get("url"),
        "ready_state": evaluate_value["readyState"],
        "machine_id": connection_info.machine_id,
        "host_id": connection_info.host_id,
        "listener_status": connection_info.listener_status,
    }
    (output_dir / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
