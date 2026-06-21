$ErrorActionPreference = "Stop"

$taskName = "Codex-Launch-RcwHostGui-Interactive"
$reportPath = "Z:\rcw-host-gui-session1-report.txt"
$runnerPath = "C:\Windows\Temp\rcw-host-gui-session1-runner.ps1"

$runner = @'
$ErrorActionPreference = "Stop"

$reportPath = "Z:\rcw-host-gui-session1-report.txt"
$auditPath = "Z:\rcw-host-gui-session1-audit.jsonl"
$stdoutPath = "Z:\rcw-host-gui-session1.stdout.log"
$stderrPath = "Z:\rcw-host-gui-session1.stderr.log"
$exePath = "Z:\rcw-host-gui-debug.exe"
$serverUrl = "ws://192.168.122.1:17800"
$cdpPort = 9222

Set-Content -Path $reportPath -Value ("timestamp=" + (Get-Date -Format o))
Add-Content -Path $reportPath -Value ("whoami=" + (whoami))
Add-Content -Path $reportPath -Value ("runnerSessionId=" + [System.Diagnostics.Process]::GetCurrentProcess().SessionId)

Stop-Process -Name "rcw-host-debug" -Force -ErrorAction SilentlyContinue
Stop-Process -Name "rcw-host-gui-debug" -Force -ErrorAction SilentlyContinue
Remove-Item -Path $auditPath -Force -ErrorAction SilentlyContinue
Remove-Item -Path $stdoutPath -Force -ErrorAction SilentlyContinue
Remove-Item -Path $stderrPath -Force -ErrorAction SilentlyContinue

$appData = Join-Path $env:APPDATA "dev.laysath.remote-control-for-windows.host-gui"
New-Item -ItemType Directory -Force -Path $appData | Out-Null

$config = @{
    server_url = $serverUrl
    totp_period_seconds = 120
    audit_log_path = $auditPath
    auto_listen = $true
} | ConvertTo-Json

$configPath = Join-Path $appData "host-gui.json"
[System.IO.File]::WriteAllText(
    $configPath,
    $config,
    [System.Text.UTF8Encoding]::new($false)
)

$env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=$cdpPort"
$env:RUST_BACKTRACE = "1"
$env:RUST_LOG = "info"

$proc = Start-Process -FilePath $exePath -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -PassThru
$null = $proc.WaitForExit(8000)

Add-Content -Path $reportPath -Value ("childProcessId=" + $proc.Id)
Add-Content -Path $reportPath -Value ("childHasExited=" + $proc.HasExited)
if ($proc.HasExited) {
    Add-Content -Path $reportPath -Value ("childExitCode=" + $proc.ExitCode)
}

$procInfo = Get-CimInstance Win32_Process -Filter ("ProcessId = " + $proc.Id) -ErrorAction SilentlyContinue
if ($null -ne $procInfo) {
    Add-Content -Path $reportPath -Value ("childSessionId=" + $procInfo.SessionId)
    Add-Content -Path $reportPath -Value ("childParentProcessId=" + $procInfo.ParentProcessId)
    Add-Content -Path $reportPath -Value ("childName=" + $procInfo.Name)
    Add-Content -Path $reportPath -Value ("childCommandLine=" + $procInfo.CommandLine)
} else {
    Add-Content -Path $reportPath -Value "childSessionId="
    Add-Content -Path $reportPath -Value "childParentProcessId="
    Add-Content -Path $reportPath -Value "childName="
    Add-Content -Path $reportPath -Value "childCommandLine="
}

Add-Content -Path $reportPath -Value "webview2-processes:"
Get-CimInstance Win32_Process |
    Where-Object { $_.Name -match "rcw-host-gui|msedgewebview2" } |
    Select-Object Name, ProcessId, SessionId, ParentProcessId, CommandLine |
    Sort-Object Name, ProcessId |
    Format-Table -Wrap -AutoSize |
    Out-String |
    Add-Content -Path $reportPath

Add-Content -Path $reportPath -Value "tcp-9222:"
Get-NetTCPConnection -LocalPort $cdpPort -ErrorAction SilentlyContinue |
    Select-Object LocalAddress, LocalPort, RemoteAddress, RemotePort, State, OwningProcess |
    Format-Table -AutoSize |
    Out-String |
    Add-Content -Path $reportPath

try {
    $resp = Invoke-WebRequest -UseBasicParsing -Uri ("http://127.0.0.1:" + $cdpPort + "/json/version") -TimeoutSec 5
    Add-Content -Path $reportPath -Value "cdp-version:"
    Add-Content -Path $reportPath -Value $resp.Content
} catch {
    Add-Content -Path $reportPath -Value ("cdp-version-error=" + $_.Exception.Message)
}

if (Test-Path $auditPath) {
    Add-Content -Path $reportPath -Value "audit-tail:"
    Get-Content -Path $auditPath -Tail 20 | Add-Content -Path $reportPath
}
if (Test-Path $stdoutPath) {
    Add-Content -Path $reportPath -Value "stdout-tail:"
    Get-Content -Path $stdoutPath -Tail 50 | Add-Content -Path $reportPath
}
if (Test-Path $stderrPath) {
    Add-Content -Path $reportPath -Value "stderr-tail:"
    Get-Content -Path $stderrPath -Tail 50 | Add-Content -Path $reportPath
}
'@

Set-Content -Path $runnerPath -Value $runner -Encoding UTF8

$action = New-ScheduledTaskAction -Execute "C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe" -Argument "-NoProfile -ExecutionPolicy Bypass -File `"$runnerPath`""
$trigger = New-ScheduledTaskTrigger -Once -At ((Get-Date).AddMinutes(5))
$principal = New-ScheduledTaskPrincipal -UserId "jgtty" -LogonType Interactive -RunLevel Highest

try {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
} catch {
}

Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal | Out-Null
Start-ScheduledTask -TaskName $taskName
Start-Sleep -Seconds 12

Get-ScheduledTask -TaskName $taskName | Select-Object TaskName, State | Format-List
Get-ScheduledTaskInfo -TaskName $taskName | Select-Object LastRunTime, LastTaskResult | Format-List
if (Test-Path $reportPath) {
    Get-Content -Path $reportPath
}
