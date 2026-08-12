[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$Publisher,
    [string]$Version = "0.1.0.0",
    [string]$CertificatePath,
    [securestring]$CertificatePassword,
    [Parameter(Mandatory = $true)][uri]$AppInstallerUri,
    [Parameter(Mandatory = $true)][uri]$MsixUri,
    [uri]$TimestampUri = "http://timestamp.digicert.com",
    [switch]$SkipSign
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$ProjectRoot = Split-Path -Parent $PSScriptRoot
$Stage = Join-Path $ProjectRoot "target\msix-stage"
$Output = Join-Path $ProjectRoot "target\package"

if ($Version -notmatch '^\d+\.\d+\.\d+\.\d+$') {
    throw "Version must contain four numeric components, for example 0.1.0.0."
}
if (@($Version.Split('.') | Where-Object { [uint32]$_ -gt 65535 }).Count -gt 0) {
    throw "Each Version component must be between 0 and 65535."
}
if ([string]::IsNullOrWhiteSpace($Publisher)) {
    throw "Publisher is required."
}
if ($AppInstallerUri.Scheme -ne "https" -or $MsixUri.Scheme -ne "https") {
    throw "AppInstallerUri and MsixUri must use HTTPS."
}

function ConvertTo-XmlAttribute([string]$Value) {
    return [Security.SecurityElement]::Escape($Value)
}

function Resolve-WindowsSdkTool([string]$Name) {
    $Command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($Command) { return $Command.Source }

    $SdkBin = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    $Candidate = Get-ChildItem (Join-Path $SdkBin "*\x64\$Name") -File -ErrorAction SilentlyContinue |
        Sort-Object { [version]$_.Directory.Parent.Name } -Descending |
        Select-Object -First 1
    if (-not $Candidate) {
        throw "$Name was not found. Install the Windows SDK or add its bin directory to PATH."
    }
    return $Candidate.FullName
}

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
$Manifest = $ManifestTemplate.Replace("@@PUBLISHER@@", (ConvertTo-XmlAttribute $Publisher)).Replace("@@VERSION@@", $Version)
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

$MakeAppx = Resolve-WindowsSdkTool "makeappx.exe"
$MsixPath = Join-Path $Output "QuickPreview-$Version-x64.msix"
& $MakeAppx pack /d $Stage /p $MsixPath /o
if ($LASTEXITCODE -ne 0) { throw "makeappx.exe failed with exit code $LASTEXITCODE" }

if (-not $SkipSign) {
    if (-not $CertificatePath) { throw "CertificatePath is required unless -SkipSign is used." }
    if (-not (Test-Path -LiteralPath $CertificatePath -PathType Leaf)) {
        throw "CertificatePath does not point to a file: $CertificatePath"
    }
    $SignTool = Resolve-WindowsSdkTool "signtool.exe"
    $Arguments = @("sign", "/fd", "SHA256", "/f", $CertificatePath)
    if ($CertificatePassword) {
        $Credential = New-Object System.Management.Automation.PSCredential("unused", $CertificatePassword)
        $Arguments += @("/p", $Credential.GetNetworkCredential().Password)
    }
    if ($TimestampUri) {
        $Arguments += @("/tr", $TimestampUri.AbsoluteUri, "/td", "SHA256")
    }
    $Arguments += $MsixPath
    & $SignTool @Arguments
    if ($LASTEXITCODE -ne 0) { throw "signtool.exe failed with exit code $LASTEXITCODE" }
}

$InstallerTemplate = Get-Content (Join-Path $PSScriptRoot "QuickPreview.appinstaller.in") -Raw
$Installer = $InstallerTemplate.Replace("@@PUBLISHER@@", (ConvertTo-XmlAttribute $Publisher)).Replace("@@VERSION@@", $Version).Replace("@@APPINSTALLER_URI@@", (ConvertTo-XmlAttribute $AppInstallerUri.AbsoluteUri)).Replace("@@MSIX_URI@@", (ConvertTo-XmlAttribute $MsixUri.AbsoluteUri))
Set-Content (Join-Path $Output "QuickPreview.appinstaller") $Installer -Encoding UTF8
Write-Host "Package output: $Output"
