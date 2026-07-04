# Windows: route the `.adi` domain to the local resolver via the Name Resolution
# Policy Table (NRPT) — the closest equivalent to macOS /etc/resolver.
#
# Only names ending in `.adi` are sent to 127.0.0.1; all other DNS is untouched,
# so the resolver's forwarding feature is unused in this split-DNS mode. To make
# adi-dns your PRIMARY resolver (exercising forwarding too), instead set the
# adapter's DNS server to 127.0.0.1:
#   Set-DnsClientServerAddress -InterfaceAlias 'Ethernet' -ServerAddresses 127.0.0.1
#
# Run in an elevated (Administrator) PowerShell.
#
# Usage:  .\windows-install.ps1 [-Domain adi]
param(
    [string]$Domain = "adi"
)

$ErrorActionPreference = "Stop"

# NRPT matches by DNS suffix; ".adi" covers adi and every *.adi name.
$namespace = ".$Domain"

# Replace any existing rule for this namespace so re-running is idempotent.
Get-DnsClientNrptRule |
    Where-Object { $_.Namespace -contains $namespace } |
    ForEach-Object { Remove-DnsClientNrptRule -Name $_.Name -Force }

Add-DnsClientNrptRule -Namespace $namespace -NameServers "127.0.0.1"
Clear-DnsClientCache

Write-Host "Installed NRPT rule: *$namespace -> 127.0.0.1"
Write-Host "Verify:  Resolve-DnsName test.$Domain"
Write-Host "Inspect: Get-DnsClientNrptRule | Where-Object { `$_.Namespace -contains '$namespace' }"
Write-Host "Remove:  Get-DnsClientNrptRule | Where-Object { `$_.Namespace -contains '$namespace' } | Remove-DnsClientNrptRule -Force"
