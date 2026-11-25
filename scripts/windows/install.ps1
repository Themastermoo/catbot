# Installs Catbot for this Windows user (no administrator needed):
#   the program in %LOCALAPPDATA%\Programs\Catbot, and Catbot in the Start menu and on the desktop.
#   Versions sit side by side there (stage 28.3): Catbot.exe is the launcher, versions\<v>\ each
#   version, versions.json which one is current. Installing over an older copy keeps the versions
#   already there, so Settings can still go back to them.
#   .\install.ps1          install, or update over an older copy
#   .\install.ps1 -Remove  take it away again (your bots, chats and memory vault stay)
param([switch]$Remove)
$ErrorActionPreference = "Stop"

$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$dest = Join-Path $env:LOCALAPPDATA "Programs\Catbot"
$startMenu = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs\Catbot.lnk"
$desktop = Join-Path ([Environment]::GetFolderPath("Desktop")) "Catbot.lnk"

# A running Catbot holds its files; close it first.
Get-Process -Name "Catbot", "catbot-server" -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

if ($Remove) {
    Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
    Remove-Item -Force $startMenu, $desktop -ErrorAction SilentlyContinue
    Write-Host "Catbot is removed. Your bots and chats stay in $env:LOCALAPPDATA\Catbot, and the memory vault in $env:USERPROFILE\CatbotMemory."
    exit 0
}

$about = Get-Content -Raw (Join-Path $here "app\version.json") | ConvertFrom-Json
$version = [string]$about.version
$versions = Join-Path $dest "versions"
$list = Join-Path $dest "versions.json"
Write-Host "Installing Catbot $version in $dest ..."
New-Item -ItemType Directory -Force $versions | Out-Null
# Anything else in the folder is an older layout's (before stage 28) or the old launcher.
Get-ChildItem $dest | Where-Object { $_.Name -ne "versions" -and $_.Name -ne "versions.json" } | Remove-Item -Recurse -Force
$mine = Join-Path $versions $version
if (Test-Path $mine) { Remove-Item -Recurse -Force $mine }
New-Item -ItemType Directory -Force $mine | Out-Null
Copy-Item -Recurse -Force (Join-Path $here "app\*") $mine
Copy-Item -Force (Join-Path $here "Catbot.exe") (Join-Path $dest "Catbot.exe")
Copy-Item -Force (Join-Path $here "app\catbot.ico") (Join-Path $dest "catbot.ico")
# Files from a downloaded zip are marked as from the internet; these are Catbot's own.
Get-ChildItem -Recurse -File $dest | Unblock-File

# versions.json: this version is current; the one it replaces can be gone back to.
$state = @{ kept = @{} }
if (Test-Path $list) {
    $old = Get-Content -Raw $list | ConvertFrom-Json
    if ($old.current) { $state.current = [string]$old.current }
    if ($old.kept) { foreach ($p in $old.kept.PSObject.Properties) { $state.kept[$p.Name] = @{ store = $p.Value.store } } }
}
if ($state.current -and $state.current -ne $version -and (Test-Path (Join-Path $versions "$($state.current)\Catbot.exe"))) {
    $state.previous = $state.current
}
$state.current = $version
$state.kept[$version] = @{ store = $about.store }
foreach ($name in @($state.kept.Keys)) {
    if (-not (Test-Path (Join-Path $versions "$name\Catbot.exe"))) { $state.kept.Remove($name) }
}
# Written without a byte-order mark, which Catbot's JSON reader doesn't expect.
[IO.File]::WriteAllText($list, ($state | ConvertTo-Json -Depth 5))

$shell = New-Object -ComObject WScript.Shell
foreach ($place in @($startMenu, $desktop)) {
    $link = $shell.CreateShortcut($place)
    $link.TargetPath = Join-Path $dest "Catbot.exe"
    $link.WorkingDirectory = $dest
    $link.IconLocation = Join-Path $dest "catbot.ico"
    $link.Description = "Catbot: your team of AI bots"
    $link.Save()
}
Write-Host "Catbot is in the Start menu and on the desktop."

# Catbot's own bots run on Claude Code.
$claude = (Get-Command claude.exe -ErrorAction SilentlyContinue) -or (Test-Path "$env:USERPROFILE\.local\bin\claude.exe") -or (Test-Path "$env:APPDATA\npm\node_modules\@anthropic-ai\claude-code\cli.js")
if (-not $claude) {
    Write-Host ""
    Write-Host "Claude Code isn't installed on this PC yet, so this PC's own bots can't answer." -ForegroundColor Yellow
    Write-Host "To install it, run this in PowerShell, then run 'claude' once and sign in:"
    Write-Host "    irm https://claude.ai/install.ps1 | iex"
    Write-Host "(Home's bots work without it.)"
}

# WebView2 draws Catbot's window; Windows 10 and 11 normally have it.
$webview = Get-ItemProperty "HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue
if (-not $webview) { $webview = Get-ItemProperty "HKCU:\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" -ErrorAction SilentlyContinue }
if (-not $webview) {
    Write-Host ""
    Write-Host "This PC may not have Microsoft's WebView2 Runtime, which Catbot's window needs." -ForegroundColor Yellow
    Write-Host "If Catbot doesn't open, install it from https://developer.microsoft.com/microsoft-edge/webview2/"
}
