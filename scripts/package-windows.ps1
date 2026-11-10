# 打包 Windows 安装程序（NSIS，按当前用户安装，无需管理员）。
# 用法：scripts/package-windows.ps1 <版本号> [输出目录]
# 前置：已构建 target/release/apkinfoplus；makensis 在 PATH 或 NSIS 默认安装目录。
param(
    [Parameter(Mandatory = $true, Position = 0)][string]$Version,
    [Parameter(Position = 1)][string]$OutDir = "dist"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root "target\release\apkinfoplus.exe"
if (-not (Test-Path $exe)) {
    throw "未找到 $exe，请先运行 cargo build --release -p apkinfoplus"
}

$makensis = (Get-Command makensis -ErrorAction SilentlyContinue).Source
if (-not $makensis) {
    $makensis = @(
        "$env:ProgramFiles\NSIS\makensis.exe",
        "${env:ProgramFiles(x86)}\NSIS\makensis.exe"
    ) | Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (-not $makensis) {
    throw "未找到 makensis，请先安装 NSIS（https://nsis.sourceforge.io）"
}

$out = Join-Path $root $OutDir
New-Item -ItemType Directory -Force $out | Out-Null
$versionNum = if ($Version -match '^\d+\.\d+\.\d+$') { "$Version.0" } else { $Version }
$outFile = Join-Path $out "ApkInfoPlus-$Version-windows-x86_64-setup.exe"

& $makensis /V2 `
    "/DAPP_VERSION=$Version" `
    "/DAPP_VERSION_NUM=$versionNum" `
    "/DROOT=$root" `
    "/DSOURCE_EXE=$exe" `
    "/DOUT_FILE=$outFile" `
    (Join-Path $root "packaging\windows\installer.nsi")
if ($LASTEXITCODE -ne 0) {
    throw "makensis 失败（退出码 $LASTEXITCODE）"
}
Write-Host "已生成 $outFile"
