<#
.SYNOPSIS
    Builds and boots the x86_64 kernel in QEMU with an E1000 NIC and user-mode NAT.
.DESCRIPTION
    Reuses the existing UEFI shim and bsp_hyperv_x86 feature; no Hyper-V VM, VHDX,
    administrator privileges, TAP adapter, or virtual switch is needed.
    Serial output, QEMU diagnostics, packet capture, and boot files live in target\qemu-x86_64.
.EXAMPLE
    .\run-qemu.ps1
.EXAMPLE
    .\run-qemu.ps1 -SkipBuild -Headless
#>
[CmdletBinding()]
param(
    [string]$QemuPath,
    [ValidateRange(1, 64)]
    [int]$Smp = 1,
    [ValidateRange(1024, 65535)]
    [int]$HostPort = 8080,
    [switch]$SkipBuild,
    [switch]$Headless,
    [switch]$PrepareOnly
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
$bootDir = Join-Path $runDir "esp"
$efiDir = Join-Path $bootDir "EFI\BOOT"
$firmwareVars = Join-Path $runDir "OVMF_VARS.fd"
$serialLog = Join-Path $runDir "serial.log"
$qemuLog = Join-Path $runDir "qemu.log"
$packetCapture = Join-Path $runDir "network.pcap"

if (-not $SkipBuild) {
    $savedRustFlags = $env:RUSTFLAGS
    $savedEncodedRustFlags = $env:CARGO_ENCODED_RUSTFLAGS
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

        Write-Host "Building x86_64 kernel (existing bsp_hyperv_x86 feature)..." -ForegroundColor Cyan
        $kernelFlags = @(
            "-C", "relocation-model=static",
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

            cargo build --release --no-default-features --features bsp_hyperv_x86,qemu `
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
}

foreach ($file in @($shimEfi, $kernelElf)) {
    if (-not (Test-Path -LiteralPath $file -PathType Leaf)) {
        throw "Missing build artifact: $file. Run again without -SkipBuild."
    }
}
New-Item -ItemType Directory -Path $efiDir -Force | Out-Null
Copy-Item -LiteralPath $shimEfi -Destination (Join-Path $efiDir "BOOTX64.EFI") -Force
Copy-Item -LiteralPath $kernelElf -Destination (Join-Path $efiDir "KERNEL.ELF") -Force
if (-not (Test-Path -LiteralPath $firmwareVars)) {
    Copy-Item -LiteralPath $firmwareTemplate -Destination $firmwareVars
}

# QEMU's key/value option parser requires literal commas in paths to be doubled.
$qemuArgs = @(
    "-name", "RustyPi5-QEMU",
    "-machine", "pc",
    "-accel", "tcg",
    "-cpu", "max",
    "-smp", "$Smp",
    "-m", "512",
    "-drive", "if=pflash,format=raw,unit=0,readonly=on,file=$($firmwareCode.Replace(',', ',,'))",
    "-drive", "if=pflash,format=raw,unit=1,file=$($firmwareVars.Replace(',', ',,'))",
    "-drive", "if=ide,format=raw,snapshot=on,file=fat:ro:$($bootDir.Replace(',', ',,'))",
    "-boot", "order=c,menu=off",
    "-netdev", "user,id=net0,net=10.0.2.0/24,dhcpstart=10.0.2.15,hostfwd=tcp:127.0.0.1:${HostPort}-10.0.2.15:8080",
    "-device", "e1000,netdev=net0,mac=52:54:00:12:34:56,addr=3",
    "-object", "filter-dump,id=capture0,netdev=net0,file=$($packetCapture.Replace(',', ',,'))",
    "-chardev", "stdio,id=com1,logfile=$($serialLog.Replace(',', ',,')),logappend=off",
    "-serial", "chardev:com1",
    "-monitor", "none",
    "-no-reboot",
    "-no-shutdown",
    "-d", "guest_errors,cpu_reset",
    "-D", $qemuLog
)
if ($Headless) { $qemuArgs += @("-display", "none") }

Write-Host "QEMU: x86_64 / TCG / $Smp CPU(s) / 512 MiB / E1000 at PCI 00:03.0" -ForegroundColor Cyan
Write-Host "Guest network: 10.0.2.15/24, gateway 10.0.2.2, DNS 10.0.2.3 (driver/stack still required)."
Write-Host "TCP forward: 127.0.0.1:$HostPort -> guest 10.0.2.15:8080 (once a guest listener exists)."
Write-Host "Serial: $serialLog"
Write-Host "Diagnostics: $qemuLog"
Write-Host "Packets: $packetCapture (overwritten each run)"
if ($PrepareOnly) {
    Write-Host "Prepared boot files; QEMU not started (-PrepareOnly)."
    return
}
Write-Host "Close the QEMU window or press Ctrl+C in this terminal to stop."
& $QemuPath @qemuArgs
if ($LASTEXITCODE -ne 0) { throw "QEMU exited with code $LASTEXITCODE. See $qemuLog and the terminal output." }
