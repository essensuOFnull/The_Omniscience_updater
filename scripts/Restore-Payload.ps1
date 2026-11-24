param(
    [string]$NamePattern = '.*'
)

$ErrorActionPreference = "Stop"

$projectRoot = Split-Path $PSScriptRoot -Parent
$fragmentsDirectory = Join-Path $projectRoot "fragmenter\fragments"
$outputDirectory = Join-Path $projectRoot "fragmenter\originals"

if (-not (Test-Path -LiteralPath $fragmentsDirectory -PathType Container)) {
    throw "Fragment directory was not found: $fragmentsDirectory"
}

$groups = @{}
foreach ($file in Get-ChildItem -LiteralPath $fragmentsDirectory -File) {
    if ($file.Name -match '^(?<base>.+)\.(?<part>\d+)$') {
        $baseName = $Matches.base
        $partNumber = [int]$Matches.part
        if (-not $groups.ContainsKey($baseName)) {
            $groups[$baseName] = @{}
        }
        if ($groups[$baseName].ContainsKey($partNumber)) {
            throw "Duplicate part $partNumber for $baseName"
        }
        $groups[$baseName][$partNumber] = $file.FullName
    }
}

$selectedGroups = @{}
foreach ($baseName in $groups.Keys) {
    if ($baseName -match $NamePattern) {
        $selectedGroups[$baseName] = $groups[$baseName]
    }
}
$groups = $selectedGroups

if ($groups.Count -eq 0) {
    throw "No numbered payload fragments were found in $fragmentsDirectory"
}

New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null

foreach ($baseName in ($groups.Keys | Sort-Object)) {
    $parts = $groups[$baseName]
    $partNumbers = @($parts.Keys | Sort-Object)
    for ($index = 0; $index -lt $partNumbers.Count; $index++) {
        if ($partNumbers[$index] -ne $index) {
            throw "Missing fragment $index for $baseName"
        }
    }

    $destination = Join-Path $outputDirectory $baseName
    $temporary = "$destination.$([guid]::NewGuid().ToString('N')).tmp"
    $output = $null

    try {
        $output = [System.IO.File]::Open(
            $temporary,
            [System.IO.FileMode]::CreateNew,
            [System.IO.FileAccess]::Write,
            [System.IO.FileShare]::None
        )

        foreach ($partNumber in $partNumbers) {
            $input = [System.IO.File]::OpenRead($parts[$partNumber])
            try {
                $input.CopyTo($output)
            } finally {
                $input.Dispose()
            }
        }

        $output.Flush($true)
        $output.Dispose()
        $output = $null

        Move-Item -LiteralPath $temporary -Destination $destination -Force
        Write-Host "Restored $baseName" -ForegroundColor Green
    } finally {
        if ($output) {
            $output.Dispose()
        }
        if (Test-Path -LiteralPath $temporary) {
            Remove-Item -LiteralPath $temporary -Force
        }
    }
}
