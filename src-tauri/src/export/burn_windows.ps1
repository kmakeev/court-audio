# Прожиг экспортного пакета на data-DVD через IMAPI2 (Windows), этап 10.2.
# Запускается через `powershell.exe -Command` из `export::dvd::ImapiBurner`.
# Не редактировать без синхронной правки `export/dvd.rs::ImapiBurner::burn`
# (порядок и имена параметров должны совпадать).
param(
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$DriveIndex,
    [Parameter(Mandatory = $true)][string]$VolumeLabel
)

$ErrorActionPreference = "Stop"

$fsi = New-Object -ComObject IMAPI2FS.MsftFileSystemImage
$fsi.VolumeName = $VolumeLabel
$fsi.FileSystemsToCreate = 0x7 # ISO9660 + Joliet + UDF — совместимость с целевыми ОС (Windows/Linux)
$fsi.Root.AddTree($SourceDir, $false)

$result = $fsi.CreateResultImage()
$stream = $result.ImageStream

$discMaster = New-Object -ComObject IMAPI2.MsftDiscMaster2
$recorderId = $discMaster.Item([int]$DriveIndex)
$recorder = New-Object -ComObject IMAPI2.MsftDiscRecorder2
$recorder.InitializeDiscRecorder($recorderId)

$writer = New-Object -ComObject IMAPI2.MsftDiscFormat2Data
$writer.Recorder = $recorder
$writer.Write($stream)
