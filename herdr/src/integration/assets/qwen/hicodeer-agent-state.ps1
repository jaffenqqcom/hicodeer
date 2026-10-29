# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HICODEER_AGENT_INTEGRATION_ID=qwen
# HICODEER_AGENT_INTEGRATION_VERSION=7

param([string]$Action = "")

if ($Action -ne "session" -and $Action -ne "working" -and $Action -ne "blocked" -and $Action -ne "idle" -and $Action -ne "notification") { exit 0 }
if ($env:HICODEER_AGENT_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HICODEER_AGENT_PANE_ID)) { exit 0 }

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
$agentBin = if ([string]::IsNullOrWhiteSpace($env:HICODEER_AGENT_BIN_PATH)) { "hicodeer-agent" } else { $env:HICODEER_AGENT_BIN_PATH }
try {
    if ($Action -eq "session") {
        if ($null -eq $payload -or [string]::IsNullOrWhiteSpace($payload.session_id)) { exit 0 }
        & $agentBin pane report-agent-session $env:HICODEER_AGENT_PANE_ID --source hicodeer:qwen --agent qwen --agent-session-id $payload.session_id --seq $seq 2>$null | Out-Null
    } else {
        & $agentBin pane report-agent $env:HICODEER_AGENT_PANE_ID --source hicodeer:qwen --agent qwen --state $Action --seq $seq 2>$null | Out-Null
    }
} catch {
}
