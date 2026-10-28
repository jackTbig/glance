# Install the English speech pack for the WinRT speech engine (Microsoft David / Zira Natural).
#
# Why a separate file instead of a one-liner in the installer's [Run] section:
# the command needs PowerShell script blocks, and Inno's brace escaping mangles them.
# In "Parameters" Inno turns "{{" into "{" but leaves "}}" as "}}" -- the 1.2.0 installer
# shipped a command with 3 "{" and 6 "}", PowerShell failed to parse it, a console
# window flashed with a red error, and the pack was never installed (exit code 1).
# With -File there are no braces in the parameters at all.
#
# Two capabilities must go in together: Language.Basic is a dependency of
# Language.TextToSpeech. Installing only the latter hangs for ~30 minutes and then
# fails with 0x80073712 (verified 2026-10-23).
#
# ASCII on purpose: PowerShell 5.1 reads a BOM-less UTF-8 script as ANSI.
#
# Exit code: 0 = installed or already there; 1 = failed (a marker file is written
# so the installer can tell the user instead of failing silently).

$ErrorActionPreference = 'Continue'
$log = Join-Path $PSScriptRoot 'english-voice-install.log'
$marker = Join-Path $PSScriptRoot 'english-voice-failed.txt'

function Say([string]$m) {
    $line = "[{0}] {1}" -f (Get-Date -Format 'yyyy-MM-dd HH:mm:ss'), $m
    Add-Content -Path $log -Value $line -Encoding ASCII
}

# Is the en-US voice there? Look for the OneCore voice token whose language is 0x409.
# This script runs as a 64-bit PowerShell on x64 Windows, so it sees the native registry view.
function Get-EnUsVoiceInstalled {
    $tokens = 'HKLM:\SOFTWARE\Microsoft\Speech_OneCore\Voices\Tokens'
    if (-not (Test-Path $tokens)) { return $false }
    foreach ($t in Get-ChildItem $tokens -ErrorAction SilentlyContinue) {
        $attr = Get-ItemProperty (Join-Path $t.PSPath 'Attributes') -ErrorAction SilentlyContinue
        if ($attr -and $attr.Language -eq 409) { return $true }
    }
    return $false
}

if (Get-EnUsVoiceInstalled) {
    Say 'en-US voice already installed, nothing to do'
    exit 0
}

Say 'installing Language.Basic + Language.TextToSpeech (en-US)'
$job = Start-Job {
    Add-WindowsCapability -Online -Name 'Language.Basic~~~en-US~0.0.1.0' | Out-Null
    Add-WindowsCapability -Online -Name 'Language.TextToSpeech~~~en-US~0.0.1.0' | Out-Null
}
# Cap it at 10 minutes: no network means the job would sit there forever.
if (-not (Wait-Job $job -Timeout 600)) {
    Stop-Job $job -ErrorAction SilentlyContinue
    Remove-Job $job -Force -ErrorAction SilentlyContinue
    Say 'timeout after 600s (probably no access to Windows Update)'
    Set-Content -Path $marker -Value 'timeout' -Encoding ASCII
    exit 1
}
$state = $job.State
$out = (Receive-Job $job 2>&1 | Out-String).Trim()
Remove-Job $job -Force -ErrorAction SilentlyContinue
Say "job finished, state=$state"
if ($out) { Say "output: $out" }

# Judge by the actual state, not by the exit code: the pack counts as installed
# only when the en-US voice token shows up.
if (Get-EnUsVoiceInstalled) {
    Say 'en-US voice is in place'
    exit 0
}
Say 'en-US voice still missing after the install'
Set-Content -Path $marker -Value $state -Encoding ASCII
exit 1
