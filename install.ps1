$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

function Ok($msg) {
    Write-Host '  ' -NoNewline
    Write-Host ([char]0x2713) -ForegroundColor Green -NoNewline
    Write-Host " $msg"
}

# only an x64 build exists; Windows on Arm runs it through emulation
$asset = 'skybin-windows-x86_64.exe'
$dir = "$env:LOCALAPPDATA\Programs\Skybin"
$exe = "$dir\skybin.exe"
$lnk = "$env:APPDATA\Microsoft\Windows\Start Menu\Programs\Skybin.lnk"
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Skybin'

Write-Host ''
Write-Host '  Skybin installer' -ForegroundColor White
Write-Host ''

New-Item -ItemType Directory -Force $dir | Out-Null
try {
    Invoke-WebRequest -UseBasicParsing "https://github.com/realChriss/skybin/releases/latest/download/$asset" -OutFile "$exe.new"
} catch {
    Write-Host '  ' -NoNewline
    Write-Host 'x' -ForegroundColor Red -NoNewline
    Write-Host " Download failed: $($_.Exception.Message)"
    Write-Host ''
    return
}
Ok ("Downloaded $asset ({0:N1} MB)" -f ((Get-Item "$exe.new").Length / 1MB))
Remove-Item "$exe.old" -ErrorAction SilentlyContinue
if (Test-Path $exe) { Move-Item $exe "$exe.old" }
Move-Item "$exe.new" $exe
Ok "Installed to $dir"

$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($lnk)
$shortcut.TargetPath = $exe
$shortcut.WorkingDirectory = $env:USERPROFILE
$shortcut.Save()
Ok 'Added to the Start menu'

function Quote($s) { "'" + $s.Replace("'", "''") + "'" }
$uninstall = "Stop-Process -Name skybin -Force -ErrorAction SilentlyContinue; Start-Sleep 1; " +
    "Remove-Item -Recurse -Force $(Quote $dir), $(Quote $lnk), $(Quote $key) -ErrorAction SilentlyContinue"
New-Item -Force $key | Out-Null
$values = @{
    DisplayName     = 'Skybin'
    DisplayIcon     = $exe
    Publisher       = 'realChriss'
    URLInfoAbout    = 'https://github.com/realChriss/skybin'
    InstallLocation = $dir
    UninstallString = "powershell.exe -NoProfile -WindowStyle Hidden -Command `"$uninstall`""
}
foreach ($name in $values.Keys) { New-ItemProperty $key -Name $name -Value $values[$name] -Force | Out-Null }
foreach ($name in 'NoModify', 'NoRepair') { New-ItemProperty $key -Name $name -Value 1 -PropertyType DWord -Force | Out-Null }
New-ItemProperty $key -Name EstimatedSize -Value ([int]((Get-Item $exe).Length / 1KB)) -PropertyType DWord -Force | Out-Null
Ok 'Added to Installed apps'

Write-Host ''
Write-Host '  Open ' -NoNewline
Write-Host 'Skybin' -ForegroundColor Cyan -NoNewline
Write-Host ' from the Start menu.'
Write-Host ''
