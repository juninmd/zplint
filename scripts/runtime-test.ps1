[CmdletBinding()]
param(
    [string]$Amxxpc = $env:AMXXPC,
    [string]$IncludeDir = $env:AMXX_INCLUDE_DIR,
    [string]$AmxxCore = $env:AMXX_CORE,
    [string]$Image = "ghcr.io/juninmd/server-do-junin:latest",
    [ValidateRange(10, 120)]
    [int]$TimeoutSeconds = 45
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repo = Split-Path -Parent $PSScriptRoot
$fixture = Join-Path $repo "crates/zpc/tests/runtime/compiler_smoke.sma"
$pluginsIni = Join-Path $repo "crates/zpc/tests/runtime/plugins.ini"
$defaultAmxx = Join-Path $repo "..\zplague-addons\cstrike\addons\amxmodx"

if ([string]::IsNullOrWhiteSpace($Amxxpc)) {
    $Amxxpc = Join-Path $defaultAmxx "scripting/amxxpc.exe"
}
if ([string]::IsNullOrWhiteSpace($IncludeDir)) {
    $IncludeDir = Join-Path $defaultAmxx "scripting/include"
}
if ([string]::IsNullOrWhiteSpace($AmxxCore)) {
    $AmxxCore = Join-Path $defaultAmxx "dlls/amxmodx_mm_i386.so"
}

foreach ($required in @($Amxxpc, $IncludeDir, $AmxxCore, $fixture, $pluginsIni)) {
    if (-not (Test-Path -LiteralPath $required)) {
        throw "required runtime-test input not found: $required"
    }
}

docker info --format "{{.ServerVersion}}" | Out-Null
if ($LASTEXITCODE -ne 0) {
    throw "Docker daemon is unavailable"
}

Push-Location $repo
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build --release failed"
    }
}
finally {
    Pop-Location
}

$zplint = Join-Path $repo "target/release/zplint.exe"
if (-not (Test-Path -LiteralPath $zplint)) {
    $zplint = Join-Path $repo "target/release/zplint"
}
if (-not (Test-Path -LiteralPath $zplint)) {
    throw "release zplint binary not found"
}

$tempBase = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
$tempRoot = Join-Path $tempBase ("zplint-runtime-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempRoot | Out-Null
$refPlugin = Join-Path $tempRoot "compiler_smoke.ref.amxx"
$ourPlugin = Join-Path $tempRoot "compiler_smoke.ours.amxx"
$containers = [Collections.Generic.List[string]]::new()

function Invoke-RuntimeCase {
    param(
        [Parameter(Mandatory)]
        [string]$Name,
        [Parameter(Mandatory)]
        [string]$Plugin
    )

    $container = "zplint-runtime-$Name-$([guid]::NewGuid().ToString('N').Substring(0, 8))"
    $containers.Add($container)
    $args = @(
        "run", "-d", "--name", $container,
        "-e", "SV_LAN=1",
        "--mount", "type=bind,source=$Plugin,target=/serverfiles/cstrike/addons/amxmodx/plugins/compiler_smoke.amxx,readonly",
        "--mount", "type=bind,source=$pluginsIni,target=/serverfiles/cstrike/addons/amxmodx/configs/plugins.ini,readonly",
        "--mount", "type=bind,source=$AmxxCore,target=/serverfiles/cstrike/addons/amxmodx/dlls/amxmodx_mm_i386.so,readonly",
        $Image,
        "sh", "-c",
        "rm -f cstrike/addons/amxmodx/configs/plugins-*.ini; exec ./hlds_run +sv_lan 1 -console -game cstrike +map zm_dust2_2k15 +maxplayers 4"
    )

    & docker @args | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "Docker failed to start runtime case '$Name'"
    }

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $logs = ""
    do {
        Start-Sleep -Seconds 1
        $logs = (& docker logs $container 2>&1) -join "`n"
        if ($logs.Contains("ZPLINT_RUNTIME_PASS")) {
            Write-Output "runtime ${Name}: PASS"
            return
        }
        if ($logs -match '(?im)(Load error.*compiler_smoke|Run time error.*compiler_smoke|Plugin "compiler_smoke\.amxx" failed|Invalid plugin.*compiler_smoke)') {
            $evidence = [regex]::Matches(
                $logs,
                '(?im)^.*(?:Load error|Run time error|Plugin "compiler_smoke\.amxx" failed|Invalid plugin).*$'
            ) | Select-Object -First 10 | ForEach-Object Value
            throw "runtime ${Name} failed:`n$($evidence -join "`n")"
        }
    } while ((Get-Date) -lt $deadline)

    throw "runtime ${Name} timed out after $TimeoutSeconds seconds without PASS marker"
}

try {
    & $Amxxpc "-o$refPlugin" "-i$IncludeDir" $fixture | Out-Null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $refPlugin)) {
        throw "reference amxxpc failed to compile runtime fixture"
    }

    & $zplint compile $fixture --output $ourPlugin --include $IncludeDir | Out-Null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $ourPlugin)) {
        throw "zplint failed to compile runtime fixture"
    }

    Invoke-RuntimeCase -Name "amxxpc-control" -Plugin $refPlugin
    Invoke-RuntimeCase -Name "zplint" -Plugin $ourPlugin
    Write-Output "runtime gate: PASS (reference and zplint)"
}
finally {
    foreach ($container in $containers) {
        & docker rm -f $container 2>$null | Out-Null
    }
    $resolvedTemp = [IO.Path]::GetFullPath($tempRoot)
    if ($resolvedTemp.StartsWith($tempBase, [StringComparison]::OrdinalIgnoreCase) -and
        (Test-Path -LiteralPath $resolvedTemp)) {
        Remove-Item -LiteralPath $resolvedTemp -Recurse -Force
    }
}
