$ErrorActionPreference = "Stop"

if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ne "X64") {
    throw "The bundled Microsoft Store package currently supports Windows x64 only."
}

$projectRoot = Split-Path $PSScriptRoot -Parent
$payloadDirectory = Join-Path $projectRoot "fragmenter\originals"
$restoreScript = Join-Path $PSScriptRoot "Restore-Payload.ps1"
$storeName = "Microsoft.WindowsStore"
$existingStore = Get-AppxPackage -Name $storeName -ErrorAction SilentlyContinue |
    Select-Object -First 1

if ($existingStore) {
    Write-Host "Microsoft Store is already registered for this user (version $($existingStore.Version))."
    Write-Host "No update or downgrade was attempted."
    exit 0
}

$requiredNames = @(
    "Microsoft.WindowsStore_22610.1401.8.0_neutral_~_8wekyb3d8bbwe.Msixbundle",
    "Microsoft.NET.Native.Framework.1.7_1.7.27413.0_x64__8wekyb3d8bbwe.Appx",
    "Microsoft.NET.Native.Runtime.1.7.appx",
    "Microsoft.UI.Xaml.2.4_2.42007.9001.0_x64__8wekyb3d8bbwe.appx",
    "Microsoft.VCLibs.140.00_14.0.33519.0_x64.appx"
)
$pattern = "^(?:" + (($requiredNames | ForEach-Object { [regex]::Escape($_) }) -join "|") + ")$"

Write-Host "Reassembling the local Microsoft Store package and its dependencies..."
& $restoreScript -NamePattern $pattern

$bundlePath = Join-Path $payloadDirectory $requiredNames[0]
$dependencyPaths = @(
    $requiredNames[1..($requiredNames.Count - 1)] | ForEach-Object {
        Join-Path $payloadDirectory $_
    }
)
$missingFiles = @($requiredNames | Where-Object { -not (Test-Path -LiteralPath (Join-Path $payloadDirectory $_) -PathType Leaf) })
if ($missingFiles.Count -gt 0) {
    throw "Required local Microsoft Store files are missing: $($missingFiles -join ', ')"
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
function Get-AppxIdentity {
    param([string]$Path)

    $archive = [System.IO.Compression.ZipFile]::OpenRead($Path)
    try {
        $entry = $archive.GetEntry("AppxManifest.xml")
        if (-not $entry) {
            throw "AppxManifest.xml was not found in $Path"
        }
        $reader = New-Object System.IO.StreamReader($entry.Open())
        try {
            [xml]$manifest = $reader.ReadToEnd()
        } finally {
            $reader.Dispose()
        }
        $identity = $manifest.Package.Identity
        if (-not $identity.Name -or -not $identity.Version) {
            throw "Package identity is missing in $Path"
        }
        return [pscustomobject]@{
            Name = [string]$identity.Name
            Version = [version]$identity.Version
        }
    } finally {
        $archive.Dispose()
    }
}

foreach ($dependencyPath in $dependencyPaths) {
    $identity = Get-AppxIdentity -Path $dependencyPath
    $installed = Get-AppxPackage -Name $identity.Name -ErrorAction SilentlyContinue |
        Where-Object { [version]$_.Version -ge $identity.Version } |
        Select-Object -First 1
    if ($installed) {
        Write-Host "$($identity.Name) version $($installed.Version) is already available."
        continue
    }
    Write-Host "Installing dependency $($identity.Name) version $($identity.Version)..."
    Add-AppxPackage -Path $dependencyPath -ErrorAction Stop
}

Write-Host "Installing Microsoft Store from local files. Windows will verify the package signature."
Add-AppxPackage -Path $bundlePath -DependencyPath $dependencyPaths -ErrorAction Stop

$installedStore = Get-AppxPackage -Name $storeName -ErrorAction SilentlyContinue |
    Select-Object -First 1
if (-not $installedStore) {
    throw "Microsoft Store installation completed without a registered package."
}

Write-Host "Microsoft Store is installed for this user (version $($installedStore.Version))."
