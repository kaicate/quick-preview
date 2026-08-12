[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Publisher,
    [string]$Version = "0.1.0.0",
    [string]$CertificatePath,
    [securestring]$CertificatePassword,
    [string]$AppInstallerUri = "https://example.invalid/QuickPreview.appinstaller",
    [string]$MsixUri = "https://example.invalid/QuickPreview.msix",
    [switch]$SkipSign
)

$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Stage = Join-Path $ProjectRoot "target\msix-stage"
$Output = Join-Path $ProjectRoot "target\package"

if (Test-Path $Stage) { Remove-Item -Recurse -Force $Stage }
if (Test-Path $Output) { Remove-Item -Recurse -Force $Output }
New-Item -ItemType Directory -Path (Join-Path $Stage "Assets") -Force | Out-Null
New-Item -ItemType Directory -Path $Output -Force | Out-Null

Push-Location $ProjectRoot
try {
    cargo build --release --target x86_64-pc-windows-msvc
} finally {
    Pop-Location
}

Copy-Item (Join-Path $ProjectRoot "target\x86_64-pc-windows-msvc\release\QuickPreview.exe") $Stage

$ManifestTemplate = Get-Content (Join-Path $PSScriptRoot "AppxManifest.xml.in") -Raw
$Manifest = $ManifestTemplate.Replace("@@PUBLISHER@@", $Publisher).Replace("@@VERSION@@", $Version)
Set-Content (Join-Path $Stage "AppxManifest.xml") $Manifest -Encoding UTF8

Add-Type -AssemblyName System.Drawing
function New-Logo([string]$Path, [int]$Width, [int]$Height) {
    $Bitmap = New-Object System.Drawing.Bitmap($Width, $Height)
    $Graphics = [System.Drawing.Graphics]::FromImage($Bitmap)
    try {
        $Graphics.Clear([System.Drawing.Color]::FromArgb(32, 105, 180))
        $FontSize = [Math]::Max(10, [Math]::Min($Width, $Height) * 0.42)
        $Font = New-Object System.Drawing.Font("Segoe UI", $FontSize, [System.Drawing.FontStyle]::Bold)
        try {
            $Format = New-Object System.Drawing.StringFormat
            $Format.Alignment = [System.Drawing.StringAlignment]::Center
            $Format.LineAlignment = [System.Drawing.StringAlignment]::Center
            $Graphics.DrawString("Q", $Font, [System.Drawing.Brushes]::White, (New-Object System.Drawing.RectangleF(0, 0, $Width, $Height)), $Format)
        } finally { $Font.Dispose() }
        $Bitmap.Save($Path, [System.Drawing.Imaging.ImageFormat]::Png)
    } finally { $Graphics.Dispose(); $Bitmap.Dispose() }
}

New-Logo (Join-Path $Stage "Assets\StoreLogo.png") 50 50
New-Logo (Join-Path $Stage "Assets\Square44x44Logo.png") 44 44
New-Logo (Join-Path $Stage "Assets\Square150x150Logo.png") 150 150
New-Logo (Join-Path $Stage "Assets\Wide310x150Logo.png") 310 150

$MakeAppx = Get-Command makeappx.exe -ErrorAction Stop
$MsixPath = Join-Path $Output "QuickPreview.msix"
& $MakeAppx.Source pack /d $Stage /p $MsixPath /o
if ($LASTEXITCODE -ne 0) { throw "makeappx.exe failed with exit code $LASTEXITCODE" }

if (-not $SkipSign) {
    if (-not $CertificatePath) { throw "CertificatePath is required unless -SkipSign is used." }
    $SignTool = Get-Command signtool.exe -ErrorAction Stop
    $Arguments = @("sign", "/fd", "SHA256", "/f", $CertificatePath)
    if ($CertificatePassword) {
        $Credential = New-Object System.Management.Automation.PSCredential("unused", $CertificatePassword)
        $Arguments += @("/p", $Credential.GetNetworkCredential().Password)
    }
    $Arguments += $MsixPath
    & $SignTool.Source @Arguments
    if ($LASTEXITCODE -ne 0) { throw "signtool.exe failed with exit code $LASTEXITCODE" }
}

$InstallerTemplate = Get-Content (Join-Path $PSScriptRoot "QuickPreview.appinstaller.in") -Raw
$Installer = $InstallerTemplate.Replace("@@PUBLISHER@@", $Publisher).Replace("@@VERSION@@", $Version).Replace("@@APPINSTALLER_URI@@", $AppInstallerUri).Replace("@@MSIX_URI@@", $MsixUri)
Set-Content (Join-Path $Output "QuickPreview.appinstaller") $Installer -Encoding UTF8
Write-Host "Package output: $Output"

