<#
    Add or remove one directory in the *user* PATH, and tell the rest of Windows about it.

    Why this is not two lines of [Environment]::SetEnvironmentVariable: that API reads the
    variable back EXPANDED and writes it as a plain string. A user PATH containing, say,
    %USERPROFILE%\.local\bin comes back with the profile path baked in and is written back
    that way — so adding ADI to PATH would quietly freeze every other entry someone had
    written as a variable, and break them the day that variable changes. The registry is read
    and written directly here, unexpanded, in its own value kind.

    Used by the installer (Add) and the uninstaller (Remove); safe to run by hand.

      powershell -ExecutionPolicy Bypass -File path.ps1 -Action Add    -Directory "C:\...\ADI\bin"
      powershell -ExecutionPolicy Bypass -File path.ps1 -Action Remove -Directory "C:\...\ADI\bin"

    Idempotent both ways: adding what is already there changes nothing, and removing what was
    never there changes nothing.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][ValidateSet('Add', 'Remove')][string] $Action,
    [Parameter(Mandatory = $true)][string] $Directory
)

$ErrorActionPreference = 'Stop'

# Compared without a trailing separator and without case, because "C:\x\bin", "C:\x\bin\" and
# "c:\X\BIN" are one directory as far as Windows is concerned, and leaving a near-duplicate on
# PATH is how it grows a copy per upgrade.
function Normalize([string] $value) {
    return $value.Trim().TrimEnd('\').ToLowerInvariant()
}

$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Environment', $true)
if ($null -eq $key) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment')
}

try {
    $raw = $key.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $kind = [Microsoft.Win32.RegistryValueKind]::ExpandString
    try {
        # Keep whatever kind is already there: a PATH that is a plain string on this machine
        # should stay one, and only a value we create ourselves picks the default.
        if ($raw -ne '') { $kind = $key.GetValueKind('Path') }
    } catch { }

    $entries = @($raw -split ';' | Where-Object { $_.Trim() -ne '' })
    $target = Normalize $Directory
    $without = @($entries | Where-Object { (Normalize $_) -ne $target })

    $updated = if ($Action -eq 'Add') { @($without) + $Directory } else { $without }
    $new = ($updated -join ';')

    if ($new -eq $raw) {
        Write-Host "PATH already correct; nothing to do."
        exit 0
    }

    $key.SetValue('Path', $new, $kind)
    Write-Host "PATH updated ($Action $Directory)."
} finally {
    $key.Dispose()
}

# A registry write alone reaches only processes started afterwards. This is the broadcast that
# makes Explorer — and so every terminal opened from it — reload the environment.
$signature = @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
    IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
try {
    $native = Add-Type -MemberDefinition $signature -Name 'AdiEnv' -Namespace 'Adi' -PassThru
    $HWND_BROADCAST = [IntPtr] 0xffff
    $WM_SETTINGCHANGE = 0x1a
    $SMTO_ABORTIFHUNG = 0x2
    $result = [UIntPtr]::Zero
    [void] $native::SendMessageTimeout($HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero,
        'Environment', $SMTO_ABORTIFHUNG, 3000, [ref] $result)
} catch {
    # Best effort. The value is written either way; a new terminal will see it regardless, and
    # failing the install over a notification would be the wrong trade.
    Write-Host "note: could not broadcast the environment change ($($_.Exception.Message))"
}
