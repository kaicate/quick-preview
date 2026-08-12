[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$CertificatePath,
    [Parameter(Mandatory = $true)][securestring]$CertificatePassword,
    [string]$Repository
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw "GitHub CLI (gh) is required. Install it and run 'gh auth login' first."
}
if (-not (Test-Path -LiteralPath $CertificatePath -PathType Leaf)) {
    throw "CertificatePath does not point to a file: $CertificatePath"
}

& gh auth status
if ($LASTEXITCODE -ne 0) { throw "GitHub CLI is not authenticated." }

if (-not $Repository) {
    $Repository = (& gh repo view --json nameWithOwner --jq .nameWithOwner).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $Repository) {
        throw "Could not determine the GitHub repository. Pass -Repository owner/name."
    }
}
if ($Repository -notmatch '^[^/]+/[^/]+$') {
    throw "Repository must use the owner/name format."
}

$Credential = New-Object System.Management.Automation.PSCredential("unused", $CertificatePassword)
$PlainPassword = $Credential.GetNetworkCredential().Password
$ResolvedCertificatePath = (Resolve-Path -LiteralPath $CertificatePath).Path
$Certificate = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
    $ResolvedCertificatePath,
    $PlainPassword,
    [System.Security.Cryptography.X509Certificates.X509KeyStorageFlags]::EphemeralKeySet
)
try {
    if (-not $Certificate.HasPrivateKey) {
        throw "The PFX does not contain a private key."
    }
    $Publisher = $Certificate.Subject
} finally {
    $Certificate.Dispose()
}

$CertificateBase64 = [Convert]::ToBase64String([IO.File]::ReadAllBytes($ResolvedCertificatePath))

function Set-GitHubEnvironmentSecret([string]$Name, [string]$Value) {
    $StartInfo = New-Object System.Diagnostics.ProcessStartInfo
    $StartInfo.FileName = (Get-Command gh).Source
    $StartInfo.Arguments = "secret set $Name --env release --repo $Repository"
    $StartInfo.UseShellExecute = $false
    $StartInfo.RedirectStandardInput = $true

    $Process = New-Object System.Diagnostics.Process
    $Process.StartInfo = $StartInfo
    try {
        if (-not $Process.Start()) { throw "Could not start GitHub CLI." }
        $Process.StandardInput.Write($Value)
        $Process.StandardInput.Close()
        $Process.WaitForExit()
        if ($Process.ExitCode -ne 0) { throw "Could not set $Name." }
    } finally {
        $Process.Dispose()
    }
}

& gh api --method PUT "repos/$Repository/environments/release" --silent
if ($LASTEXITCODE -ne 0) { throw "Could not create the release environment." }

& gh variable set WINDOWS_PUBLISHER --env release --repo $Repository --body $Publisher
if ($LASTEXITCODE -ne 0) { throw "Could not set WINDOWS_PUBLISHER." }

Set-GitHubEnvironmentSecret "WINDOWS_CERTIFICATE_BASE64" $CertificateBase64
Set-GitHubEnvironmentSecret "WINDOWS_CERTIFICATE_PASSWORD" $PlainPassword

Write-Host "Configured the release environment for $Repository."
Write-Host "WINDOWS_PUBLISHER=$Publisher"
