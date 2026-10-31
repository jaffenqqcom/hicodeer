# installed by HiCodeer
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=qwen
# HERDR_INTEGRATION_VERSION=5

param([string]$Action = "")

if ($Action -ne "session" -and $Action -ne "working" -and $Action -ne "blocked" -and $Action -ne "idle" -and $Action -ne "notification") { exit 0 }
if ($env:HERDR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8.
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    $payload = $null
}

if ($Action -eq "notification") {
    $nt = ("$($payload.notification_type)$($payload.notificationType)").ToLowerInvariant()
    if ($nt.Contains("permission") -or $nt.Contains("elicitation")) { $Action = "blocked" }
    elseif ($nt.Contains("idle")) { $Action = "idle" }
    else { exit 0 }
}
$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$herdr = if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) { "herdr" } else { $env:HERDR_BIN_PATH }
try {
    if ($Action -eq "session") {
        if ($null -eq $payload -or [string]::IsNullOrWhiteSpace($payload.session_id)) { exit 0 }
        & $herdr pane report-agent-session $env:HERDR_PANE_ID --source hicodeer:qwen --agent qwen --agent-session-id $payload.session_id --seq $seq 2>$null | Out-Null
    } else {
        & $herdr pane report-agent $env:HERDR_PANE_ID --source hicodeer:qwen --agent qwen --state $Action --seq $seq 2>$null | Out-Null
    }
} catch {
}
