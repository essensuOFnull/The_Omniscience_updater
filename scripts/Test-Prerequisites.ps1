param(
    [switch]$Json
)

$ErrorActionPreference = "Stop"

function Get-ToolCheck {
    param(
        [string]$Name,
        [string]$CommandName,
        [string[]]$Arguments,
        [string]$Purpose
    )

    $command = Get-Command $CommandName -CommandType Application,ExternalScript -ErrorAction SilentlyContinue |
        Select-Object -First 1

    if (-not $command) {
        return [pscustomobject]@{
            Name = $Name
            Status = "Missing"
            Purpose = $Purpose
            Version = $null
            Path = $null
            Detail = "Command not found on PATH."
        }
    }

    if ($command.Source -match '(?i)[\\/]Microsoft[\\/]WindowsApps[\\/]') {
        return [pscustomobject]@{
            Name = $Name
            Status = "AliasOnly"
            Purpose = $Purpose
            Version = $null
            Path = $command.Source
            Detail = "App execution alias detected; it was not launched to avoid opening the Microsoft Store."
        }
    }

    try {
        $global:LASTEXITCODE = 0
        $output = @(& $command.Source @Arguments 2>$null)
        $exitCode = $LASTEXITCODE
        $version = ($output | ForEach-Object { "$_".Trim() } | Where-Object { $_ }) -join " "

        if ($exitCode -ne 0) {
            return [pscustomobject]@{
                Name = $Name
                Status = "Error"
                Purpose = $Purpose
                Version = $version
                Path = $command.Source
                Detail = "Version check exited with code $exitCode."
            }
        }

        return [pscustomobject]@{
            Name = $Name
            Status = "Present"
            Purpose = $Purpose
            Version = $version
            Path = $command.Source
            Detail = $null
        }
    } catch {
        return [pscustomobject]@{
            Name = $Name
            Status = "Error"
            Purpose = $Purpose
            Version = $null
            Path = $command.Source
            Detail = $_.Exception.Message
        }
    }
}

$checks = @(
    Get-ToolCheck -Name "Git" -CommandName "git" -Arguments @("--version") -Purpose "Required to fetch and manage source checkouts."
    Get-ToolCheck -Name "Node.js" -CommandName "node" -Arguments @("--version") -Purpose "Required for the Omniscience frontend and Node sidecar."
    Get-ToolCheck -Name "npm" -CommandName "npm" -Arguments @("--version") -Purpose "Required to install locked project dependencies."
    Get-ToolCheck -Name "Rust" -CommandName "rustc" -Arguments @("--version") -Purpose "Required to run Tauri Dev and build the native application."
    Get-ToolCheck -Name "Cargo" -CommandName "cargo" -Arguments @("--version") -Purpose "Required to run Tauri Dev and build the native application."
    Get-ToolCheck -Name "Python" -CommandName "python" -Arguments @("--version") -Purpose "May be needed by npm packages that build native modules."
)

$programFilesX86 = [Environment]::GetFolderPath("ProgramFilesX86")
$vsWherePath = Join-Path $programFilesX86 "Microsoft Visual Studio\Installer\vswhere.exe"
$vsInstallPath = $null
if (Test-Path -LiteralPath $vsWherePath -PathType Leaf) {
    $vsInstallPath = (& $vsWherePath -latest -products "*" -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null |
        Select-Object -First 1)
}

if ($vsInstallPath) {
    $checks += [pscustomobject]@{
        Name = "MSVC Build Tools"
        Status = "Present"
        Purpose = "Required to build Rust targets using the MSVC toolchain."
        Version = $null
        Path = $vsInstallPath.Trim()
        Detail = $null
    }
} else {
    $checks += [pscustomobject]@{
        Name = "MSVC Build Tools"
        Status = "Missing"
        Purpose = "Required to build Rust targets using the MSVC toolchain."
        Version = $null
        Path = $null
        Detail = "Visual Studio installation with the VC x86/x64 tools workload was not detected."
    }
}

$appInstaller = Get-AppxPackage -Name "Microsoft.DesktopAppInstaller" -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($appInstaller) {
    $checks += [pscustomobject]@{
        Name = "App Installer / winget"
        Status = "Present"
        Purpose = "Optional package manager for prerequisite installation."
        Version = [string]$appInstaller.Version
        Path = $appInstaller.InstallLocation
        Detail = "Detected from the registered Windows package; winget was not executed."
    }
} else {
    $checks += [pscustomobject]@{
        Name = "App Installer / winget"
        Status = "Missing"
        Purpose = "Optional package manager for prerequisite installation."
        Version = $null
        Path = $null
        Detail = "Windows package was not detected. No installation was attempted."
    }
}

$report = [pscustomobject]@{
    GeneratedAt = [DateTime]::UtcNow.ToString("o")
    OperatingSystem = [Environment]::OSVersion.VersionString
    Architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    Checks = $checks
    ReadOnly = $true
}

if ($Json) {
    $report | ConvertTo-Json -Depth 5
    exit 0
}

Write-Host "Omniscience environment check (read-only)" -ForegroundColor Cyan
Write-Host "OS: $($report.OperatingSystem) [$($report.Architecture)]"
Write-Host ""
$checks | Format-Table Name, Status, Version, Path -AutoSize
Write-Host ""
foreach ($check in $checks | Where-Object { $_.Detail }) {
    Write-Host "$($check.Name): $($check.Detail)" -ForegroundColor DarkGray
}
Write-Host ""
Write-Host "No packages were installed and no system settings were changed." -ForegroundColor Green
