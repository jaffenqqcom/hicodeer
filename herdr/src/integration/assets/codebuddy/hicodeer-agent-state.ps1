# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HICODEER_INTEGRATION_ID=codebuddy
# HICODEER_INTEGRATION_VERSION=7

param([string]$Action = "")

if ($Action -notin @("session", "working", "blocked", "idle", "notification")) { exit 0 }
if ($env:HICODEER_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HICODEER_PANE_ID)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    exit 0
}

$propertyNames = @($payload.PSObject.Properties.Name)
if ((Test-Path Env:CURSOR_VERSION) -or $propertyNames -ccontains "cursor_version") { exit 0 }
if (-not [string]::IsNullOrWhiteSpace($payload.agent_id)) { exit 0 }

$sessionId = $payload.session_id
if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }

# CodeBuddy fires PermissionRequest for every tool call, whether or not it then
# approves the call itself, so the event does not mean the turn is waiting. Real
# permission prompts arrive as a Notification with a permission type, and
# questions arrive as AskUserQuestion, so this event always reports working.
if ($Action -eq "blocked" -and $payload.hook_event_name -ceq "PermissionRequest") {
    $Action = "working"
}

if ($Action -eq "notification") {
    $nt = ("$($payload.notification_type)$($payload.notificationType)").ToLowerInvariant()
    if ($nt.Contains("permission") -or $nt.Contains("elicitation")) { $Action = "blocked" }
    elseif ($nt.Contains("idle")) { $Action = "idle" }
    else { exit 0 }
}
$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$hicodeer = if ([string]::IsNullOrWhiteSpace($env:HICODEER_BIN_PATH)) { "hicodeerd" } else { $env:HICODEER_BIN_PATH }
try {
    if ($Action -eq "session") {
        if ($payload.hook_event_name -cne "SessionStart") { exit 0 }
        $args = @(
            "pane",
            "report-agent-session",
            $env:HICODEER_PANE_ID,
            "--source",
            "hicodeer:codebuddy",
            "--agent",
            "codebuddy",
            "--seq",
            "$seq",
            "--agent-session-id",
            "$sessionId"
        )
        if ($payload.transcript_path -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.transcript_path)) {
            $args += @("--agent-session-path", "$($payload.transcript_path)")
        }
        if ($payload.source -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.source)) {
            $args += @("--session-start-source", "$($payload.source)")
        }
    } else {
        $args = @(
            "pane",
            "report-agent",
            $env:HICODEER_PANE_ID,
            "--source",
            "hicodeer:codebuddy",
            "--agent",
            "codebuddy",
            "--state",
            "$Action",
            "--seq",
            "$seq",
            "--agent-session-id",
            "$sessionId"
        )
        if ($payload.transcript_path -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.transcript_path)) {
            $args += @("--agent-session-path", "$($payload.transcript_path)")
        }
    }
    & $hicodeer @args 2>$null | Out-Null
} catch {
}
