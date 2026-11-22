<#
.SYNOPSIS
    Clean-builds and boots the x86_64 kernel with networking off or an isolated host-only test peer.
.DESCRIPTION
    Uses bsp_qemu_x86 and the existing UEFI shim. The dedicated GPT disk has a
    FAT32 EFI partition and a read-only RustyPi5FS root. Python 3 is required.
    Builds always clean their build artifacts, never the disk. -CreateDisk is
    required for first use and refuses to overwrite an existing image.
    The persistent disk is rustypi5.img beside this script; serial output,
    QEMU diagnostics and packet capture live in target\qemu-x86_64.
    Defaults to four virtual CPUs: the detected boot CPU is reserved for kernel
    services. At least one online secondary CPU is required for the user shell.
.EXAMPLE
    .\run-qemu.ps1 -CreateDisk
.EXAMPLE
    .\run-qemu.ps1 -NetworkMode HostOnly
    # The launcher starts a loopback-only Ethernet echo peer; there is no WAN backend.
    # Loopback sockets are not authenticated, so the local host must be trusted.
#>
[CmdletBinding()]
param(
    [string]$QemuPath,
    [ValidateRange(1, 64)]
    [int]$Smp = 4,
    [ValidateSet("Off", "HostOnly")]
    [string]$NetworkMode = "Off",
    [switch]$Headless,
    [switch]$PrepareOnly,
    [switch]$CreateDisk
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

if (-not $QemuPath) {
    $qemuCommand = Get-Command qemu-system-x86_64.exe -ErrorAction SilentlyContinue
    if ($qemuCommand) {
        $QemuPath = $qemuCommand.Source
    }
    else {
        $QemuPath = Join-Path $env:ProgramFiles "qemu\qemu-system-x86_64.exe"
    }
}
if (-not (Test-Path -LiteralPath $QemuPath -PathType Leaf)) {
    throw "QEMU not found at '$QemuPath'. Install QEMU for Windows or pass -QemuPath <full executable path>."
}
$QemuPath = (Resolve-Path -LiteralPath $QemuPath).Path
$firmwareDir = Join-Path (Split-Path -Parent $QemuPath) "share"
$firmwareCode = Join-Path $firmwareDir "edk2-x86_64-code.fd"
$firmwareTemplate = Join-Path $firmwareDir "edk2-i386-vars.fd"
foreach ($file in @($firmwareCode, $firmwareTemplate)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Missing QEMU UEFI firmware: $file. Install the QEMU EDK2 firmware files alongside QEMU."
    }
}

$repoRoot = (Resolve-Path "$PSScriptRoot\..").Path
$shimDir = Join-Path $repoRoot "Shims\hyperv-shim"
$kernelDir = Join-Path $PSScriptRoot "kernel"
$runDir = Join-Path $PSScriptRoot "target\qemu-x86_64"
$shimTarget = Join-Path $runDir "shim"
$kernelTarget = Join-Path $runDir "kernel"
$shimEfi = Join-Path $shimTarget "x86_64-unknown-uefi\debug\hyperv-shim.efi"
$kernelElf = Join-Path $kernelTarget "x86_64-unknown-none\release\RustyPi5"
$diskImage = Join-Path $PSScriptRoot "rustypi5.img"
$diskTool = Join-Path $PSScriptRoot "tools\disk_image.py"
$hostPeer = Join-Path $PSScriptRoot "tools\network_host_peer.py"
$firmwareVars = Join-Path $runDir "OVMF_VARS.fd"
$serialLog = Join-Path $runDir "serial.log"
$qemuLog = Join-Path $runDir "qemu.log"
$packetCapture = Join-Path $runDir "network.pcap"
$python = (Get-Command python -ErrorAction Stop).Source
$null = Get-Command cargo -ErrorAction Stop
New-Item -ItemType Directory -Path $runDir -Force | Out-Null
$runtimeLock = $null
$process = $null
$processStarted = $false
$peerProcess = $null
$peerStarted = $false
$savedRustFlags = $env:RUSTFLAGS
$savedEncodedRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
try {
# The persistent lock file is deliberately retained: deleting it after closing creates a race
# with another launcher acquiring it. Only an exclusively held handle denotes ownership.
$runtimeLock = [IO.File]::Open((Join-Path $runDir "launcher.lock"),
    [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
if (Test-Path -LiteralPath $diskImage) {
    $diskCheck = [IO.File]::Open($diskImage, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $diskCheck.Dispose()
}
if ($CreateDisk -and (Test-Path -LiteralPath $diskImage)) {
    throw "Disk already exists: $diskImage. -CreateDisk never overwrites an image."
}
if (-not $CreateDisk -and -not (Test-Path -LiteralPath $diskImage -PathType Leaf)) {
    throw "Disk not found: $diskImage. Use -CreateDisk once to explicitly create it."
}
if ($NetworkMode -eq "HostOnly" -and -not (Test-Path -LiteralPath $hostPeer -PathType Leaf)) {
    throw "Host-only network peer not found: $hostPeer"
}

try {
        Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
        Remove-Item Env:\CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
        Write-Host "Building UEFI shim..." -ForegroundColor Cyan
        Push-Location $shimDir
        try {
            Write-Host "Cleaning UEFI shim build artifacts..." -ForegroundColor Cyan
            cargo clean --target-dir $shimTarget
            if ($LASTEXITCODE -ne 0) { throw "UEFI shim clean failed." }

            cargo build --features qemu --target x86_64-unknown-uefi --target-dir $shimTarget `
                -Z build-std=core,alloc,compiler_builtins
            if ($LASTEXITCODE -ne 0) { throw "UEFI shim build failed." }
        }
        finally { Pop-Location }

        Write-Host "Building x86_64 kernel (bsp_qemu_x86)..." -ForegroundColor Cyan
        # Rust's generated jump tables otherwise live in supervisor-only rodata.
        $kernelFlags = @(
            "-C", "relocation-model=static",
            "-C", "llvm-args=--min-jump-table-entries=1000000",
            "-C", "link-arg=-no-pie",
            "-C", "link-arg=--entry=_start",
            "-C", "link-arg=--no-gc-sections",
            "-C", "link-arg=--defsym=__kernel_load_addr=0x2000000",
            "-C", "link-arg=-T$kernelDir\hyperv_x86.ld"
        )
        $env:CARGO_ENCODED_RUSTFLAGS = $kernelFlags -join [char]0x1F
        Push-Location $kernelDir
        try {
            Write-Host "Cleaning x86_64 kernel build artifacts..." -ForegroundColor Cyan
            cargo clean --target-dir $kernelTarget
            if ($LASTEXITCODE -ne 0) { throw "Kernel clean failed." }

            cargo build --release --no-default-features --features bsp_qemu_x86 `
                --target x86_64-unknown-none --target-dir $kernelTarget `
                -Z build-std=core,alloc,compiler_builtins
            if ($LASTEXITCODE -ne 0) { throw "Kernel build failed." }
        }
        finally { Pop-Location }
    }
    finally {
        $env:RUSTFLAGS = $savedRustFlags
        $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedRustFlags
    }

foreach ($file in @($shimEfi, $kernelElf)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Clean build did not produce the required artifact: $file"
    }
}
New-Item -ItemType Directory -Path $runDir -Force | Out-Null
$diskOperation = "update-boot"
if ($CreateDisk) { $diskOperation = "create" }
if (Test-Path -LiteralPath $diskImage) {
    $diskCheck = [IO.File]::Open($diskImage, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    $diskCheck.Dispose()
}
& $python $diskTool $diskOperation $diskImage --shim $shimEfi --kernel $kernelElf
if ($LASTEXITCODE -ne 0) { throw "Disk preparation failed. Root partition was not reformatted." }
if (-not (Test-Path -LiteralPath $firmwareVars)) {
    Copy-Item -LiteralPath $firmwareTemplate -Destination $firmwareVars
}

$hostPort = 0
if ($NetworkMode -eq "HostOnly" -and -not $PrepareOnly) {
    $peerInfo = New-Object Diagnostics.ProcessStartInfo
    $peerInfo.FileName = $python
    $peerInfo.Arguments = '"' + ($hostPeer -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
    $peerInfo.UseShellExecute = $false
    $peerInfo.RedirectStandardOutput = $true
    $peerProcess = New-Object Diagnostics.Process
    $peerProcess.StartInfo = $peerInfo
    $peerStarted = $peerProcess.Start()
    if (-not $peerStarted) { throw "Could not start the host-only network peer." }
    $portRead = $peerProcess.StandardOutput.ReadLineAsync()
    if (-not $portRead.Wait(10000)) { throw "Host-only network peer did not become ready." }
    if (-not [int]::TryParse($portRead.Result, [ref]$hostPort) -or $hostPort -lt 1 -or $hostPort -gt 65535 -or $peerProcess.HasExited) {
        throw "Host-only network peer failed to bind a loopback port."
    }
}

# QEMU's key/value option parser requires literal commas in paths to be doubled.
switch ($NetworkMode) {
    "Off" { $networkArgs = @("-nic", "none") }
    "HostOnly" {
        $networkArgs = @("-netdev", "socket,id=net0,connect=127.0.0.1:$hostPort")
    }
}
if ($NetworkMode -ne "Off") {
    $networkArgs += @(
        "-device", "e1000,netdev=net0,mac=52:54:00:12:34:56,addr=3",
        "-object", "filter-dump,id=capture0,netdev=net0,file=$($packetCapture.Replace(',', ',,'))"
    )
}
$qemuArgs = @(
    "-name", "RustyPi5-QEMU",
    "-machine", "pc",
    "-accel", "tcg",
    "-cpu", "max",
    "-smp", "$Smp,sockets=1,cores=$Smp,threads=1",
    "-m", "512",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$($firmwareCode.Replace(',', ',,'))",
    "-drive", "if=pflash,format=raw,unit=1,file=$($firmwareVars.Replace(',', ',,'))",
    "-drive", "if=ide,index=0,media=disk,format=raw,snapshot=on,file=$($diskImage.Replace(',', ',,'))",
    "-boot", "order=c,menu=off",
    "-chardev", "stdio,id=com1,signal=off,logfile=$($serialLog.Replace(',', ',,'))",
    "-serial", "chardev:com1",
    "-monitor", "none",
    "-no-reboot",
    "-no-shutdown",
    "-d", "guest_errors,cpu_reset",
    "-D", $qemuLog
)
$qemuArgs += $networkArgs
if ($Headless) { $qemuArgs += @("-display", "none") }

Write-Host "QEMU: x86_64 / TCG / $Smp CPU(s) / 512 MiB / network=$NetworkMode" -ForegroundColor Cyan
if ($NetworkMode -eq "HostOnly") {
    Write-Host "Host-only echo peer; no NAT, bridge, external firewall or port forwarding."
    Write-Host "In the guest: net status; udp 10.0.2.2 8081 hello"
}
if ($NetworkMode -ne "Off") {
    Write-Host "RustyPi5 kernel firewall: default deny; UDP 10.0.2.15:49152 -> 10.0.2.2:8081 only."
}
Write-Host "Serial: $serialLog"
Write-Host "Diagnostics: $qemuLog"
if ($NetworkMode -ne "Off") { Write-Host "Packets: $packetCapture (overwritten each run)" }
if ($PrepareOnly) {
    Write-Host "Prepared boot files; QEMU not started (-PrepareOnly)."
    return
}
Write-Host "Close the QEMU window or press Ctrl+C in this terminal to stop (forced stop, not guest poweroff)."
$startInfo = New-Object Diagnostics.ProcessStartInfo
$startInfo.FileName = $QemuPath
$startInfo.UseShellExecute = $false
# Quote each argv element using Windows CRT rules; preserve paths containing spaces.
$startInfo.Arguments = ($qemuArgs | ForEach-Object {
    '"' + ($_ -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}) -join ' '
$process = New-Object Diagnostics.Process
$process.StartInfo = $startInfo
$processStarted = $process.Start()
if (-not $processStarted) { throw "Could not start QEMU." }
$peerExitReported = $false
while (-not $process.WaitForExit(250)) {
    if ($peerStarted -and -not $peerExitReported -and $peerProcess.HasExited) {
        Write-Warning "Host-only network peer exited with code $($peerProcess.ExitCode); guest requests may time out."
        $peerExitReported = $true
    }
}
if ($process.ExitCode -ne 0) {
    throw "QEMU exited with code $($process.ExitCode). See $qemuLog and the terminal output."
}
}
finally {
    try {
        if ($null -ne $process) {
            try {
                if ($processStarted -and -not $process.HasExited) {
                    $process.Kill()
                    if (-not $process.WaitForExit(10000)) { throw "Owned QEMU process did not stop." }
                }
            }
            finally { $process.Dispose() }
        }
    }
    finally {
        try {
            if ($null -ne $peerProcess) {
                try {
                    if ($peerStarted -and -not $peerProcess.HasExited) {
                        $peerProcess.Kill()
                        if (-not $peerProcess.WaitForExit(10000)) { throw "Owned host-only peer did not stop." }
                    }
                }
                finally { $peerProcess.Dispose() }
            }
        }
        finally {
            $env:RUSTFLAGS = $savedRustFlags
            $env:CARGO_ENCODED_RUSTFLAGS = $savedEncodedRustFlags
            if ($null -ne $runtimeLock) { $runtimeLock.Dispose() }
        }
    }
}
