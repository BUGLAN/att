param(
    [string]$EnvDir = ".venv",
    [string]$PythonExe = "3.12",
    [string]$TorchVersion = "2.8.0+cu128"
)

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$envPath = Join-Path $projectRoot $EnvDir
$pythonPath = Join-Path $envPath "Scripts\python.exe"
$validateBackendRuntime = "import importlib.util, sys; missing = [name for name in ('qwen_asr',) if importlib.util.find_spec(name) is None]; print('missing', missing); sys.exit(0 if not missing else 1)"
$validatePythonAndTorch = "import sys, torch; cv = torch.version.cuda or ''; print('python', sys.version); print('torch', torch.__version__); print('cuda_available', torch.cuda.is_available()); print('cuda_version', cv); sys.exit(0 if sys.version_info[:2] == (3, 12) and torch.__version__ == '$TorchVersion' and torch.cuda.is_available() and cv.startswith('12.8') else 1)"

function Invoke-Checked {
    param(
        [string]$Description,
        [scriptblock]$Action
    )

    & $Action
    if ($LASTEXITCODE -ne 0) {
        throw "$Description failed with exit code $LASTEXITCODE"
    }
}

function Write-Step {
    param([string]$Message)
    Write-Host "[backend-uv] $Message"
}

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    throw "uv was not found in PATH. Please install uv first."
}

Push-Location $projectRoot
try {
    $env:UV_CACHE_DIR = Join-Path $projectRoot "tmp\uv-cache"
    $env:UV_PROJECT_ENVIRONMENT = $envPath

    Write-Step "Removing broken environment if it exists"
    if (Test-Path $envPath) {
        Remove-Item -Recurse -Force $envPath
    }

    Write-Step "Creating local uv environment at $EnvDir"
    Invoke-Checked "Creating local uv environment" {
        uv venv $envPath --python $PythonExe
    }

    if (-not (Test-Path $pythonPath)) {
        throw "Local uv environment was not created correctly: $pythonPath"
    }

    Write-Step "Installing project dependencies from pyproject.toml"
    Invoke-Checked "Installing project dependencies from pyproject.toml" {
        uv pip install --python $pythonPath -r pyproject.toml
    }

    Write-Step "Validating Python and CUDA torch alignment"
    Invoke-Checked "Validating Python and CUDA torch alignment" {
        uv run --python $pythonPath python -c $validatePythonAndTorch
    }

    Write-Step "Validating backend runtime dependencies"
    Invoke-Checked "Validating backend runtime dependencies" {
        uv run --python $pythonPath python -c $validateBackendRuntime
    }

    Write-Step "Backend uv environment is ready"
}
finally {
    Pop-Location
}
