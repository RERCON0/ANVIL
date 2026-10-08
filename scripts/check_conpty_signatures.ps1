$ErrorActionPreference = 'Stop'
$vendorRoot = Join-Path $PSScriptRoot '../vendor/conpty'
$pins = Get-Content -LiteralPath (Join-Path $vendorRoot 'publisher.json') -Raw | ConvertFrom-Json
foreach ($name in @('conpty.dll', 'OpenConsole.exe')) {
    $signature = Get-AuthenticodeSignature -LiteralPath (Join-Path $vendorRoot "x64/$name")
    if ($signature.Status -ne 'Valid' -or
        $signature.SignerCertificate.Thumbprint -ne $pins.thumbprint -or
        $signature.SignerCertificate.Subject -ne $pins.subject) {
        throw "ConPTY publisher verification failed: $name ($($signature.Status))"
    }
}
Write-Output 'Verified Microsoft Authenticode signatures for vendored ConPTY'
