$ErrorActionPreference = "Stop"

$taskName = "Codex-Launch-RcwHost-Interactive"
$reportPath = "Z:\rcw-host-session1-report.txt"
$stdoutPath = "Z:\rcw-host-session1.stdout.log"
$stderrPath = "Z:\rcw-host-session1.stderr.log"
$auditPath = "Z:\rcw-host-session1-audit.jsonl"
$runnerPath = "C:\Windows\Temp\rcw-host-session1-runner.ps1"

$runner = @'
$ErrorActionPreference = "Stop"

$reportPath = "Z:\rcw-host-session1-report.txt"
$stdoutPath = "Z:\rcw-host-session1.stdout.log"
$stderrPath = "Z:\rcw-host-session1.stderr.log"
$auditPath = "Z:\rcw-host-session1-audit.jsonl"
$exePath = "Z:\rcw-host-debug.exe"
$serverUrl = "ws://192.168.122.1:17800"

Set-Content -Path $reportPath -Value ("timestamp=" + (Get-Date -Format o))
Add-Content -Path $reportPath -Value ("whoami=" + (whoami))
Add-Content -Path $reportPath -Value ("runnerSessionId=" + [System.Diagnostics.Process]::GetCurrentProcess().SessionId)

Stop-Process -Name "rcw-host-debug" -Force -ErrorAction SilentlyContinue
Remove-Item -Path $stdoutPath, $stderrPath, $auditPath -Force -ErrorAction SilentlyContinue

$proc = Start-Process -FilePath $exePath `
    -ArgumentList @("--server", $serverUrl, "--audit-log", $auditPath) `
    -RedirectStandardOutput $stdoutPath `
    -RedirectStandardError $stderrPath `
    -PassThru

Start-Sleep -Seconds 3

$procInfo = Get-CimInstance Win32_Process -Filter ("ProcessId = " + $proc.Id)
Add-Content -Path $reportPath -Value ("childProcessId=" + $proc.Id)
Add-Content -Path $reportPath -Value ("childSessionId=" + $procInfo.SessionId)
Add-Content -Path $reportPath -Value ("childParentProcessId=" + $procInfo.ParentProcessId)
Add-Content -Path $reportPath -Value ("childName=" + $procInfo.Name)
Add-Content -Path $reportPath -Value ("childCommandLine=" + $procInfo.CommandLine)

if (Test-Path $stdoutPath) {
    Add-Content -Path $reportPath -Value "stdout:"
    Get-Content -Path $stdoutPath | Add-Content -Path $reportPath
}

if (Test-Path $stderrPath) {
    Add-Content -Path $reportPath -Value "stderr:"
    Get-Content -Path $stderrPath | Add-Content -Path $reportPath
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
Start-Sleep -Seconds 6

Get-ScheduledTask -TaskName $taskName | Select-Object TaskName, State | Format-List
Get-ScheduledTaskInfo -TaskName $taskName | Select-Object LastRunTime, LastTaskResult | Format-List
if (Test-Path $reportPath) {
    Get-Content -Path $reportPath
}
