# Eludite's shell integration for PowerShell (brief 0041), dot-sourced through `-NoExit -Command` after your
# profile (nothing in your profile is changed): OSC 133 marks around prompts and commands (A prompt start, B command
# start, C output start, D;<exit code> command end) and the folder (OSC 7). Works in Windows PowerShell 5.1 and
# PowerShell 7.
if (-not $global:__EluditeIntegrated) {
    $global:__EluditeIntegrated = $true
    $global:__EluditeEsc = [char]27
    $global:__EluditeBel = [char]7
    $global:__EluditeLastHistory = -1
    $global:__EluditeUserPrompt = $function:prompt
    function global:prompt {
        $ok = $global:?
        $code = $global:LASTEXITCODE
        $e = $global:__EluditeEsc
        $b = $global:__EluditeBel
        $out = ''
        $last = Get-History -Count 1
        $id = if ($last) { $last.Id } else { 0 }
        if ($global:__EluditeLastHistory -ge 0 -and $id -ne $global:__EluditeLastHistory) {
            $status = if ($ok) { 0 } elseif ($code -is [int] -and $code -ne 0) { $code } else { 1 }
            $out += "$e]133;D;$status$b"
        }
        $global:__EluditeLastHistory = $id
        $folder = $PWD.ProviderPath -replace '\\', '/'
        if (-not $folder.StartsWith('/')) { $folder = '/' + $folder }
        $out += "$e]133;A$b$e]7;file://$env:COMPUTERNAME$folder$b"
        $out += & $global:__EluditeUserPrompt
        $out += "$e]133;B$b"
        $global:LASTEXITCODE = $code
        $out
    }
    if (Get-Module -Name PSReadLine) {
        Set-PSReadLineKeyHandler -Chord Enter -ScriptBlock {
            [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
            [Console]::Write("$($global:__EluditeEsc)]133;C$($global:__EluditeBel)")
        }
    }
}
