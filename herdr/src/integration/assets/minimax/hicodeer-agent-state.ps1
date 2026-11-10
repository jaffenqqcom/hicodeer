# installed by HiCodeer
# managed by herdr; reinstalling or updating the integration overwrites this file.
# HERDR_INTEGRATION_ID=minimax
# HERDR_INTEGRATION_VERSION=1

param([string]$Action = "")

if ($Action -notin @("session", "working", "blocked", "idle")) { exit 0 }
if ($env:HERDR_ENV -ne "1") { exit 0 }
if ([string]::IsNullOrWhiteSpace($env:HERDR_PANE_ID)) { exit 0 }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8.
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    exit 0
}

# A subagent turn reports on the subagent, not on the pane's own agent.
if (-not [string]::IsNullOrWhiteSpace($payload.agent_id)) { exit 0 }
if (-not [string]::IsNullOrWhiteSpace($payload.subagent_id)) { exit 0 }

$sessionId = $payload.session_id
$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$herdr = if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) { "herdr" } else { $env:HERDR_BIN_PATH }
try {
    if ($Action -eq "session") {
        if ([string]::IsNullOrWhiteSpace($sessionId)) { exit 0 }
        $reportArgs = @(
            "pane",
            "report-agent-session",
            $env:HERDR_PANE_ID,
            "--source",
            "hicodeer:minimax",
            "--agent",
            "minimax",
            "--seq",
            "$seq",
            "--agent-session-id",
            "$sessionId"
        )
    } else {
        $reportArgs = @(
            "pane",
            "report-agent",
            $env:HERDR_PANE_ID,
            "--source",
            "hicodeer:minimax",
            "--agent",
            "minimax",
            "--state",
            "$Action",
            "--seq",
            "$seq"
        )
        if (-not [string]::IsNullOrWhiteSpace($sessionId)) {
            $reportArgs += @("--agent-session-id", "$sessionId")
        }
        # Only the two states that need attention carry text: a wait states the
        # question, a finished turn states its reply.
        if ($Action -eq "blocked") {
            $text = $payload.question
            if ([string]::IsNullOrWhiteSpace($text)) { $text = $payload.message }
            if (-not [string]::IsNullOrWhiteSpace($text)) { $reportArgs += @("--message", "$text") }
        } elseif ($Action -eq "idle") {
            $text = $payload.last_assistant_message
            if ([string]::IsNullOrWhiteSpace($text)) { $text = $payload.lastAssistantMessage }
            if (-not [string]::IsNullOrWhiteSpace($text)) { $reportArgs += @("--message", "$text") }
        }
    }
    & $herdr @reportArgs 2>$null | Out-Null
} catch {
}
