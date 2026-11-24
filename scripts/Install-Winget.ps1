$ErrorActionPreference = "Stop"

if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ne "X64") {
    throw "The bundled App Installer package currently supports Windows x64 only."
}

$projectRoot = Split-Path $PSScriptRoot -Parent
$payloadDirectory = Join-Path $projectRoot "fragmenter\originals"
$restoreScript = Join-Path $PSScriptRoot "Restore-Payload.ps1"
$packageName = "Microsoft.DesktopAppInstaller"
$existingPackage = Get-AppxPackage -Name $packageName -ErrorAction SilentlyContinue |
    Select-Object -First 1

if ($existingPackage) {
    Write-Host "App Installer is already registered for this user (version $($existingPackage.Version))."
    Write-Host "No update or downgrade was attempted."
    exit 0
}

$requiredNames = @(
    "Microsoft.DesktopAppInstaller_8wekyb3d8bbwe.msixbundle",
    "Microsoft.VCLibs.140.00.UWPDesktop_14.0.33728.0_x64.appx",
    "Microsoft.UI.Xaml.2.8_8.2501.31001.0_x64__8wekyb3d8bbwe.Appx",
    "Microsoft.NET.Native.Framework.2.2_2.2.29512.0_x64__8wekyb3d8bbwe.Appx",
    "Microsoft.NET.Native.Runtime.2.2_2.2.28604.0_x64__8wekyb3d8bbwe.Appx"
)
$pattern = "^(?:" + (($requiredNames | ForEach-Object { [regex]::Escape($_) }) -join "|") + ")$"

Write-Host "Reassembling the local App Installer package and its dependencies..."
& $restoreScript -NamePattern $pattern

$bundlePath = Join-Path $payloadDirectory $requiredNames[0]
$dependencyPaths = @(
    $requiredNames[1..($requiredNames.Count - 1)] | ForEach-Object {
        Join-Path $payloadDirectory $_
    }
)
$missingFiles = @($requiredNames | Where-Object { -not (Test-Path -LiteralPath (Join-Path $payloadDirectory $_) -PathType Leaf) })
if ($missingFiles.Count -gt 0) {
    throw "Required local App Installer files are missing: $($missingFiles -join ', ')"
}

Write-Host "Installing App Installer from local files. Windows will verify the package signature."
Add-AppxPackage -Path $bundlePath -DependencyPath $dependencyPaths -ErrorAction Stop

$installedPackage = Get-AppxPackage -Name $packageName -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $installedPackage) {
    throw "App Installer installation completed without a registered package."
}

Write-Host "App Installer is installed for this user (version $($installedPackage.Version))."
Write-Host "The winget command alias may become available after signing out or restarting the app."
