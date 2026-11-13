# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HICODEER_INTEGRATION_ID=grok
# HICODEER_INTEGRATION_VERSION=5

param([string]$Action = "")

if ($Action -ne "session" -and $Action -ne "working" -and $Action -ne "blocked" -and $Action -ne "idle") { exit 0 }
if ($env:HICODEER_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HICODEER_PANE_ID)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    $payload = $null
}

$event = if ($null -ne $payload -and $payload.hook_event_name -is [string]) {
    $payload.hook_event_name
} elseif ($null -ne $payload -and $payload.hookEventName -is [string]) {
    $payload.hookEventName
} else {
    $null
}
if ($Action -eq "session" -and $null -ne $event -and $event -notin @("session_start", "SessionStart", "sessionStart")) { exit 0 }

$sessionStartSource = if ($null -ne $payload -and $payload.source -is [string]) {
    $payload.source
} else {
    $null
}

$sessionId = $env:GROK_SESSION_ID
if ([string]::IsNullOrWhiteSpace($sessionId) -and $null -ne $payload) {
    if ($payload.session_id -is [string]) { $sessionId = $payload.session_id }
    elseif ($payload.sessionId -is [string]) { $sessionId = $payload.sessionId }
}
$seq = [DateTime]::UtcNow.Ticks
$hicodeer = if ([string]::IsNullOrWhiteSpace($env:HICODEER_BIN_PATH)) { "hicodeerd" } else { $env:HICODEER_BIN_PATH }
try {
    if ($Action -eq "session") {
        if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }
        Args = @(
            "pane", "report-agent-session", $env:HICODEER_PANE_ID,
            "--source", "hicodeer:grok",
            "--agent", "grok",
            "--seq", "$seq",
            "--agent-session-id", "$sessionId"
        )
        if (-not [string]::IsNullOrWhiteSpace($sessionStartSource)) {
            Args += @("--session-start-source", "$sessionStartSource")
        }
        & $hicodeer @herdrArgs 2>$null | Out-Null
    } else {
        Args = @(
            "pane", "report-agent", $env:HICODEER_PANE_ID,
            "--source", "hicodeer:grok",
            "--agent", "grok",
            "--state", "$Action",
            "--seq", "$seq"
        )
        if (-not [string]::IsNullOrWhiteSpace($sessionId)) {
            Args += @("--agent-session-id", "$sessionId")
        }
        & $hicodeer @herdrArgs 2>$null | Out-Null
    }
} catch {
}
