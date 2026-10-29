# installed by HiCodeer
# managed by HiCodeer; reinstalling or updating the integration overwrites this file.
# add custom hooks beside this file instead of editing it.
# HICODEER_AGENT_INTEGRATION_ID=antigravity_cli
# HICODEER_AGENT_INTEGRATION_VERSION=7

# Session-only: this hook reports the Antigravity conversation so HiCodeer can
# resume the pane. Lifecycle state comes from HiCodeer's screen detection.

param([string]$Action = "")

# Antigravity CLI expects a JSON object on stdout and this hook never injects
# anything, so every exit path emits an empty object.
function Exit-Hook {
    Write-Output "{}"
    exit 0
}

if ($Action -ne "session") { Exit-Hook }
if ($env:HICODEER_AGENT_ENV -ne "1") { Exit-Hook }
if ([string]::IsNullOrWhiteSpace($env:HICODEER_AGENT_PANE_ID)) { Exit-Hook }

# Read hook input as raw bytes so UTF-8 payloads decode correctly even when the
# console input code page is not UTF-8 (for example CP949 on Korean Windows).
$stdinReader = New-Object System.IO.StreamReader([Console]::OpenStandardInput(), [System.Text.Encoding]::UTF8)
$inputText = $stdinReader.ReadToEnd()
try {
    $payload = if ([string]::IsNullOrWhiteSpace($inputText)) { $null } else { $inputText | ConvertFrom-Json }
} catch {
    Exit-Hook
}

if ($null -eq $payload) { Exit-Hook }

$conversationId = if ($payload.conversationId -is [string]) { $payload.conversationId } else { $null }
if ([string]::IsNullOrWhiteSpace($conversationId)) { Exit-Hook }

$seq = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
$agentBin = if ([string]::IsNullOrWhiteSpace($env:HICODEER_AGENT_BIN_PATH)) { "hicodeer-agent" } else { $env:HICODEER_AGENT_BIN_PATH }
try {
    $sessionArgs = @(
        "pane",
        "report-agent-session",
        $env:HICODEER_AGENT_PANE_ID,
        "--source",
        "hicodeer:antigravity_cli",
        "--agent",
        "agy",
        "--seq",
        "$seq",
        "--agent-session-id",
        "$conversationId"
    )
    if ($payload.transcriptPath -is [string] -and -not [string]::IsNullOrWhiteSpace($payload.transcriptPath)) {
        $sessionArgs += @("--agent-session-path", "$($payload.transcriptPath)")
    }
    & $agentBin @sessionArgs 2>$null | Out-Null
} catch {
}

Exit-Hook
