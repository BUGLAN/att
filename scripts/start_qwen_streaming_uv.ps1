param(
    [string]$EnvDir = ".venv",
    [string]$Host = "0.0.0.0",
    [int]$Port = 9000,
    [string]$Model = "Qwen/Qwen3-ASR-1.7B",
    [string]$CacheDir = "models/qwen"
)

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$pythonExe = Join-Path (Join-Path $projectRoot $EnvDir) "Scripts\python.exe"
$resolvedCacheDir = Join-Path $projectRoot $CacheDir

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    throw "uv was not found in PATH. Please install uv first."
}

if (-not (Test-Path $pythonExe)) {
    throw "Virtual environment Python was not found: $pythonExe. Run scripts/setup_qwen_streaming_windows_uv.ps1 first."
}

Push-Location $projectRoot
try {
    $env:QWEN_STREAMING_HOST = $Host
    $env:QWEN_STREAMING_PORT = $Port.ToString()
    $env:QWEN_STREAMING_MODEL = $Model
    $env:QWEN_STREAMING_CACHE_DIR = $resolvedCacheDir

    Write-Host "[qwen-start] Starting Qwen streaming service with uv"
    Write-Host "[qwen-start] Python: $pythonExe"
    Write-Host "[qwen-start] Host: $Host"
    Write-Host "[qwen-start] Port: $Port"
    Write-Host "[qwen-start] Model: $Model"
    Write-Host "[qwen-start] Cache: $resolvedCacheDir"

    & uv run --python $pythonExe python scripts/qwen_streaming_service.py
}
finally {
    Pop-Location
}
