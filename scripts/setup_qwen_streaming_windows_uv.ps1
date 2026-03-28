param(
    [string]$EnvDir = ".venv",
    [string]$PythonVersion = "3.12",
    [string]$TorchVersion = "2.8.0+cu128",
    [string]$TorchIndexUrl = "https://download.pytorch.org/whl/cu128",
    [string]$VllmVersion = "0.17.0",
    [string]$VllmWheel = ""
)

$ErrorActionPreference = "Stop"

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$envPath = Join-Path $projectRoot $EnvDir
$pythonExe = Join-Path $envPath "Scripts\python.exe"
$wheelsDir = Join-Path $projectRoot "wheels"
$defaultIndexUrl = "https://pypi.org/simple"
$torchCheckCode = "import sys, torch; print('python', sys.version); print('torch', torch.__version__); print('cuda_available', torch.cuda.is_available()); print('cuda_version', torch.version.cuda)"
$torchValidationCode = "import sys, torch; cv = torch.version.cuda or ''; print('python', sys.version); print('torch', torch.__version__); print('cuda_available', torch.cuda.is_available()); print('cuda_version', cv); sys.exit(0 if sys.version_info[:2] == (3, 12) and torch.__version__ == '$TorchVersion' and torch.cuda.is_available() and cv.startswith('12.8') else 1)"
$runtimeValidationCode = "import importlib.util, importlib.metadata, sys; missing = [name for name in ('qwen_asr', 'vllm') if importlib.util.find_spec(name) is None]; print('missing', missing); print('vllm_version', importlib.metadata.version('vllm') if not missing else 'missing'); sys.exit(0 if not missing else 1)"

function Write-Step {
    param([string]$Message)
    Write-Host "[qwen-setup] $Message"
}

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

function Resolve-VllmWheelPath {
    param(
        [string]$WheelArg,
        [string]$Version,
        [string]$SearchDir
    )

    if ($WheelArg) {
        $candidate = $WheelArg
        if (-not [System.IO.Path]::IsPathRooted($candidate)) {
            $candidate = Join-Path $projectRoot $candidate
        }

        if (-not (Test-Path $candidate)) {
            throw "Specified vLLM wheel was not found: $candidate"
        }

        return (Resolve-Path $candidate).Path
    }

    if (-not (Test-Path $SearchDir)) {
        throw "No vLLM wheel was provided and wheels directory was not found: $SearchDir"
    }

    $pattern = "vllm-$Version*.whl"
    $wheel = Get-ChildItem -Path $SearchDir -File -Filter $pattern |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1

    if (-not $wheel) {
        throw (
            "No vLLM wheel matching $pattern was found in $SearchDir. " +
            "Place the vllm-windows wheel there or pass -VllmWheel <path-to-wheel>."
        )
    }

    return $wheel.FullName
}

if (-not (Get-Command uv -ErrorAction SilentlyContinue)) {
    throw "uv was not found in PATH. Please install uv first."
}

Push-Location $projectRoot
try {
    Write-Step "Creating Python $PythonVersion virtual environment at $EnvDir"
    Invoke-Checked "Creating Python $PythonVersion virtual environment" {
        uv venv $envPath --python $PythonVersion
    }

    if (-not (Test-Path $pythonExe)) {
        throw "Virtual environment Python was not found: $pythonExe"
    }

    Write-Step "Installing project dependencies from pyproject.toml"
    Invoke-Checked "Installing project dependencies from pyproject.toml" {
        uv pip install --python $pythonExe -r pyproject.toml
    }

    $resolvedVllmWheel = Resolve-VllmWheelPath -WheelArg $VllmWheel -Version $VllmVersion -SearchDir $wheelsDir
    Write-Step "Installing vLLM wheel from $resolvedVllmWheel"
    Invoke-Checked "Installing vLLM wheel from $resolvedVllmWheel" {
        uv pip install --python $pythonExe `
            --index-url $defaultIndexUrl `
            --extra-index-url $TorchIndexUrl `
            --index-strategy unsafe-best-match `
            $resolvedVllmWheel
    }

    Write-Step "Checking CUDA torch runtime"
    Invoke-Checked "Checking CUDA torch runtime" {
        uv run --python $pythonExe python -c $torchCheckCode
    }

    Write-Step "Validating CUDA torch runtime"
    Invoke-Checked "Validating CUDA torch runtime" {
        uv run --python $pythonExe python -c $torchValidationCode
    }

    Write-Step "Validating Qwen streaming runtime"
    Invoke-Checked "Validating Qwen streaming runtime" {
        uv run --python $pythonExe python -c $runtimeValidationCode
    }

    Write-Step "Setup completed"
    Write-Host ""
    Write-Host "Start the Qwen streaming service with:"
    Write-Host "powershell -ExecutionPolicy Bypass -File .\scripts\start_qwen_streaming_uv.ps1 -EnvDir `"$EnvDir`""
}
finally {
    Pop-Location
}
