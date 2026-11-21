param(
    [ValidateSet('Release', 'Debug', 'Test')][string]$Mode = 'Release',
    [string]$TargetDir = ''
)
$ErrorActionPreference = 'Stop'
$Root = $PSScriptRoot
if (-not $TargetDir) {
    $TargetDir = Join-Path (Split-Path $Root -Parent) 'rust-netfile-target'
}
$TargetDir = [System.IO.Path]::GetFullPath($TargetDir)
$oldTarget = $env:CARGO_TARGET_DIR
Push-Location $Root
try {
    $env:CARGO_TARGET_DIR = $TargetDir
    if ($Mode -eq 'Test') {
        & cargo test --locked
    } elseif ($Mode -eq 'Debug') {
        & cargo build --locked
    } else {
        & cargo build --release --locked
    }
    if ($LASTEXITCODE -ne 0) { throw "Cargo exited with code $LASTEXITCODE" }

    if ($Mode -ne 'Test') {
        $profile = if ($Mode -eq 'Release') { 'release' } else { 'debug' }
        $dist = Join-Path $Root 'dist'
        New-Item -ItemType Directory -Path $dist -Force | Out-Null

        $desktop = Join-Path $TargetDir "$profile/nb-file-manager.exe"
        if (-not (Test-Path -LiteralPath $desktop)) { throw "未找到桌面端产物：$desktop" }
        Copy-Item -LiteralPath $desktop -Destination (Join-Path $dist 'nb-file-manager.exe') -Force

        $versionLine = Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
        if ($versionLine.Matches.Count -gt 0) {
            $version = $versionLine.Matches[0].Groups[1].Value
            Copy-Item -LiteralPath $desktop -Destination (Join-Path $dist "NB-File-Manager-v$version.exe") -Force
        }

        $loader = Join-Path $TargetDir "$profile/WebView2Loader.dll"
        if (Test-Path -LiteralPath $loader) {
            Write-Warning '检测到 WebView2Loader.dll。GitHub Actions 构建应静态链接该库；本地产物如需分发请确认系统已安装 WebView2 Runtime。'
        }

        & cargo build --release --locked --no-default-features --bin nb-file-server
        if ($LASTEXITCODE -ne 0) { throw "Cargo (headless) exited with code $LASTEXITCODE" }
        $server = Join-Path $TargetDir 'release/nb-file-server.exe'
        if (Test-Path -LiteralPath $server) {
            Copy-Item -LiteralPath $server -Destination (Join-Path $dist 'nb-file-server.exe') -Force
        }

        Write-Output "桌面端：$(Join-Path $dist 'nb-file-manager.exe')"
        Write-Output "命令行服务：$(Join-Path $dist 'nb-file-server.exe')"
    }
} finally {
    $env:CARGO_TARGET_DIR = $oldTarget
    Pop-Location
}
