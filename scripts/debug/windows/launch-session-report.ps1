$ErrorActionPreference = "Stop"

$taskName = "Codex-Launch-Session-Report"
$reportPath = "Z:\session-launch-report.txt"
$runnerPath = "C:\Windows\Temp\session-report-runner.ps1"

$runner = @'
$ErrorActionPreference = "Stop"
$out = "Z:\session-launch-report.txt"
Set-Content -Path $out -Value ("timestamp=" + (Get-Date -Format o))
Add-Content -Path $out -Value ("whoami=" + (whoami))
Add-Content -Path $out -Value ("sessionId=" + [System.Diagnostics.Process]::GetCurrentProcess().SessionId)
Add-Content -Path $out -Value ("processId=" + [System.Diagnostics.Process]::GetCurrentProcess().Id)
Add-Content -Path $out -Value ("parent=" + (Get-CimInstance Win32_Process -Filter ("ProcessId = " + $PID) | Select-Object -ExpandProperty ParentProcessId))
try {
    Add-Content -Path $out -Value "quser:"
    quser | Out-String | Add-Content -Path $out
} catch {
    Add-Content -Path $out -Value ("quser-error=" + $_.Exception.Message)
}
Start-Process notepad.exe
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
Start-Sleep -Seconds 5
Get-ScheduledTask -TaskName $taskName | Select-Object TaskName, State | Format-List
Get-ScheduledTaskInfo -TaskName $taskName | Select-Object LastRunTime, LastTaskResult | Format-List
if (Test-Path $reportPath) {
    Get-Content -Path $reportPath
}
