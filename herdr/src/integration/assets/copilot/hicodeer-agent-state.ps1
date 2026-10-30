# installed by HiCodeer
# managed by herdr; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HERDR_INTEGRATION_ID=copilot
# HERDR_INTEGRATION_VERSION=6

if ($env:HERDR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HICODEER_AGENT_SOCKET_PATH)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { @{} } else { $inputText | ConvertFrom-Json }
} catch {
    $payload = @{}
}

function First-Text {
    param([object[]]$Names)
    foreach ($name in $Names) {
        $value = $payload.$name
        if ($value -is [string] -and -not [string]::IsNullOrWhiteSpace($value)) {
            return $value
        }
    }
    return $null
}

function Normalize-Event {
    param([string]$Event)
    if ([string]::IsNullOrWhiteSpace($Event)) { return "" }
    return $Event.Replace("_", "").Replace("-", "").ToLowerInvariant()
}

$eventName = First-Text @("hook_event_name", "hookEventName")
$normalized = Normalize-Event $eventName

# Event -> the pane state it reports; see the sh hook for the reasoning.
$stateByEvent = @{
    "userpromptsubmitted" = "working"
    "pretooluse"          = "working"
    "posttooluse"         = "working"
    "permissionrequest"   = "blocked"
    "agentstop"           = "idle"
}

if ($normalized -eq "sessionstart") {
    $Action = "session"
} elseif ($stateByEvent.ContainsKey($normalized)) {
    $Action = $stateByEvent[$normalized]
} elseif ($normalized -eq "notification") {
    $notificationType = First-Text @("notification_type", "notificationType")
    if ($notificationType -and ($notificationType.ToLowerInvariant().Contains("elicitation") -or $notificationType.ToLowerInvariant().Contains("permission"))) {
        $Action = "blocked"
    } else {
        exit 0
    }
} else {
    exit 0
}

$sessionId = $payload.session_id
if ([string]::IsNullOrWhiteSpace($sessionId)) {
    $sessionId = $payload.sessionId
}

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$herdr = if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) { "herdr" } else { $env:HERDR_BIN_PATH }
if ($Action -eq "session") {
    if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }
    & $herdr pane report-agent-session $env:HERDR_PANE_ID --source herdr:copilot --agent copilot --agent-session-id $sessionId --seq $seq 2>$null | Out-Null
} else {
    & $herdr pane report-agent $env:HERDR_PANE_ID --source herdr:copilot --agent copilot --state $Action --seq $seq 2>$null | Out-Null
}
