# Builds the release exe and wraps it in dist\Farlens-<version>-setup.exe (needs Inno Setup 6).
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
$ver = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"').Matches[0].Groups[1].Value
cargo build --release
if ($LASTEXITCODE) { exit $LASTEXITCODE }
$iscc = @("$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe", "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
& $iscc "/DAppVersion=$ver" installer\farlens.iss
