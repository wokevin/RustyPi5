<#
.SYNOPSIS
	Rebuilds the hyperv-shim UEFI loader and the RustyPi5 kernel for bsp_hyperv_x86, deploys both
	to the boot VHDX, and (optionally) starts/connects to the Hyper-V VM.

.DESCRIPTION
	This mirrors the manual steps used to get the kernel booting under Hyper-V, in an order that
	never needs to touch a VHDX that's already attached to a running/saved VM:

	  1. If the VHDX doesn't exist yet, create and format a new FAT32 EFI System Partition VHDX
		 (skip with -NoAutoInit).
	  2. Build `Shims/hyperv-shim` for x86_64-unknown-uefi.
	  3. Build the kernel (`20_timer_callbacks/kernel`) for x86_64-unknown-none with
		 `bsp_hyperv_x86`.
	  4. If the VM already exists, stop it (it owns the VHDX while Running/Saved/Paused).
	  5. Mount the boot VHDX directly, copy the shim to \EFI\BOOT\BOOTX64.EFI and the kernel ELF
		 to \EFI\BOOT\KERNEL.ELF, then dismount.
	  6. If the VM doesn't exist yet, create a Generation 2 Hyper-V VM (now pointing at a VHDX
		 that already has a valid boot loader on it), disable Secure Boot, and wire up a COM1
		 serial pipe (skip with -NoAutoInit).
	  7. Start the VM and open a console window.

.NOTES
	Run this from an elevated PowerShell prompt — mounting VHDX files and controlling Hyper-V VMs
	both require administrator privileges.

	Fill in $VmName / $VhdxPath below to match your actual Hyper-V VM once, then just re-run this
	script after making kernel changes.

	First run and every run after that:

		.\setupvm.ps1

	The VHDX and VM are created automatically the first time (since they don't exist yet) and
	reused on every subsequent run. Pass -NoAutoInit if you'd rather this fail loudly instead of
	auto-creating anything (e.g. once you're pointing $VmName/$VhdxPath at a hand-built VM).

	Kernel output goes to COM1, not the vmconnect video console (this kernel never touches the
	framebuffer). Connect to the printed named pipe path to see kernel output.
#>

param(
	[string]$VmName    = "RustyPi5-HyperV",
	[string]$VhdxPath  = "$PSScriptRoot\..\Shims\hyperv-shim\boot.vhdx",
	[UInt64]$VhdxSizeBytes = 256MB,
	[switch]$NoAutoInit,
	[switch]$SkipStart,
	[switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$RepoRoot   = Resolve-Path "$PSScriptRoot\.."
$ShimDir    = Join-Path $RepoRoot "Shims\hyperv-shim"
$KernelDir  = Join-Path $RepoRoot "20_timer_callbacks\kernel"
$ComPipePath = "\\.\pipe\$VmName-com1"

function Write-Step($msg) {
	Write-Host ""
	Write-Host "==> $msg" -ForegroundColor Cyan
}

function Assert-Admin {
	$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
	$principal = New-Object Security.Principal.WindowsPrincipal($identity)
	if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
		throw "This script must be run from an elevated (Administrator) PowerShell prompt."
	}
}

Assert-Admin

##--------------------------------------------------------------------------------------------------
## 1. VHDX creation (host-side; no VM exists yet, or it isn't attached to this file specifically
##    at this point, so this is always safe).
##--------------------------------------------------------------------------------------------------
if (-not $NoAutoInit -and -not (Test-Path $VhdxPath)) {
	Write-Step "VHDX not found; creating: $VhdxPath ($VhdxSizeBytes bytes, dynamic)"
	$vhdxDir = Split-Path -Parent $VhdxPath
	New-Item -ItemType Directory -Path $vhdxDir -Force | Out-Null
	New-VHD -Path $VhdxPath -SizeBytes $VhdxSizeBytes -Dynamic | Out-Null

	Write-Step "Initializing VHDX as GPT with a FAT32 EFI System Partition"
	$disk = Mount-VHD -Path $VhdxPath -Passthru | Get-Disk
	Initialize-Disk -Number $disk.Number -PartitionStyle GPT
	$partition = New-Partition -DiskNumber $disk.Number -UseMaximumSize `
		-GptType '{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}' -AssignDriveLetter
	Format-Volume -Partition $partition -FileSystem FAT32 -NewFileSystemLabel "EFIBOOT" -Confirm:$false | Out-Null
	Dismount-VHD -Path $VhdxPath

	Write-Step "VHDX created and formatted."
}

if (-not (Test-Path $VhdxPath)) {
	throw "VHDX not found at: $VhdxPath (remove -NoAutoInit to create it automatically, or update `$VhdxPath at the top of this script)"
}

##--------------------------------------------------------------------------------------------------
## 2-3. Build the shim and kernel.
##--------------------------------------------------------------------------------------------------
if (-not $SkipBuild) {
	Write-Step "Building hyperv-shim (x86_64-unknown-uefi)"
	Push-Location $ShimDir
	try {
		cargo build -Z build-std=core,alloc,compiler_builtins --target x86_64-unknown-uefi
		if ($LASTEXITCODE -ne 0) { throw "hyperv-shim build failed." }
	}
	finally {
		Pop-Location
	}

	Write-Step "Building kernel (bsp_hyperv_x86, x86_64-unknown-none, release)"
	Push-Location $KernelDir
	try {
		# Force a static, non-position-independent executable (ET_EXEC, e_entry = real address),
		# with the entry point explicitly set to our `_start` symbol, and section garbage
		# collection disabled. Without `--entry=_start`, the linker defaults to its usual `_start`
		# lookup via the C runtime convention, which doesn't exist in this no_std/no_main binary,
		# so it silently falls back to entry = 0; and without `--no-gc-sections`, `--gc-sections`
		# strips away everything not transitively reachable from that nonexistent entry, leaving
		# only ~232 bytes of ELF header/metadata and none of the kernel's actual code.
		$env:RUSTFLAGS = "-C relocation-model=static -C link-args=-no-pie -C link-args=--entry=_start -C link-args=--no-gc-sections -C link-args=-T$KernelDir\hyperv_x86.ld"
		try {
			cargo build --release --features bsp_hyperv_x86 --target x86_64-unknown-none `
				-Z build-std=core,alloc,compiler_builtins
			if ($LASTEXITCODE -ne 0) { throw "Kernel build failed." }
		}
		finally {
			Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
		}
	}
	finally {
		Pop-Location
	}
}
else {
	Write-Step "Skipping build (SkipBuild specified)"
}

$ShimEfi = Join-Path $ShimDir "target\x86_64-unknown-uefi\debug\hyperv-shim.efi"
if (-not (Test-Path $ShimEfi)) {
	throw "Could not find built shim at: $ShimEfi (build output layout may have changed)"
}

$KernelElf = Join-Path (Join-Path $RepoRoot "20_timer_callbacks") "target\x86_64-unknown-none\release\RustyPi5"
if (-not (Test-Path $KernelElf)) {
	throw "Could not find built kernel ELF at: $KernelElf (build output layout may have changed)"
}

##--------------------------------------------------------------------------------------------------
## 4. If the VM already exists, it holds an exclusive lock on the VHDX whenever it's not fully
##    powered off (Running, Saved, or Paused). Stop it before mounting the disk ourselves.
##--------------------------------------------------------------------------------------------------
$existingVm = Get-VM -Name $VmName -ErrorAction SilentlyContinue
if ($existingVm -and $existingVm.State -ne "Off") {
	Write-Step "Stopping VM '$VmName' (currently $($existingVm.State)) so its VHDX can be mounted"

	try {
		Stop-VM -Name $VmName -TurnOff -Force -ErrorAction Stop
	} catch {
		Write-Host "Stop-VM threw an error: $_" -ForegroundColor Red
		throw
	}

	# Stop-VM -Force can return before the VHDX lock is actually released (a race between the
	# command returning and Hyper-V's worker process fully tearing down), which can cause
	# Mount-VHD below to either fail outright or -- worse -- silently mount a disk that's still
	# mid-release, leading to the VM later getting stuck in a state that needs manual
	# intervention via Hyper-V Manager. Poll until State is confirmed Off before proceeding.
	$deadline = (Get-Date).AddSeconds(30)
	while ((Get-VM -Name $VmName).State -ne "Off") {
		if ((Get-Date) -ge $deadline) {
			Write-Host "VM state is still '$((Get-VM -Name $VmName).State)' after 30s." -ForegroundColor Red
			throw "VM '$VmName' did not reach the Off state within 30 seconds after Stop-VM. This matches needing to manually 'Turn Off' via Hyper-V Manager -- investigate why Stop-VM -TurnOff -Force isn't taking effect from this script (permissions? a hung VMWP process? check Hyper-V-Worker-Admin event log for this VM's ID around this timestamp)."
		}
		Start-Sleep -Milliseconds 250
	}
	Write-Step "VM '$VmName' confirmed Off"
}

##--------------------------------------------------------------------------------------------------
## 5. Stage the shim + kernel onto the VHDX.
##--------------------------------------------------------------------------------------------------
Write-Step "Mounting VHDX: $VhdxPath"
$disk = Mount-VHD -Path $VhdxPath -Passthru | Get-Disk
$partition = $disk | Get-Partition | Where-Object { $_.DriveLetter } | Select-Object -First 1

if (-not $partition) {
	Dismount-VHD -Path $VhdxPath
	throw "Could not find a mounted, drive-lettered partition on $VhdxPath (expected a FAT32 EFI system partition; remove -NoAutoInit to create one automatically)."
}

$driveLetter = $partition.DriveLetter
$efiBootDir  = "${driveLetter}:\EFI\BOOT"

try {
	Write-Step "Copying shim -> ${efiBootDir}\BOOTX64.EFI"
	New-Item -ItemType Directory -Path $efiBootDir -Force | Out-Null
	Copy-Item -Path $ShimEfi -Destination (Join-Path $efiBootDir "BOOTX64.EFI") -Force

	Write-Step "Copying kernel -> ${efiBootDir}\KERNEL.ELF"
	Copy-Item -Path $KernelElf -Destination (Join-Path $efiBootDir "KERNEL.ELF") -Force
}
finally {
	Write-Step "Dismounting VHDX"
	Dismount-VHD -Path $VhdxPath
}

##--------------------------------------------------------------------------------------------------
## 6. Create the VM now, if it doesn't exist yet — the VHDX already has a valid boot loader on it,
##    so there's no need to stage files a second time after VM creation.
##--------------------------------------------------------------------------------------------------
if (-not $NoAutoInit -and -not $existingVm) {
	Write-Step "VM '$VmName' not found; creating Generation 2 VM"
	New-VM -Name $VmName -Generation 2 -MemoryStartupBytes 512MB -VHDPath $VhdxPath | Out-Null

	Write-Step "Disabling Secure Boot (shim is unsigned)"
	Set-VMFirmware -VMName $VmName -EnableSecureBoot Off

	# New-VM's implicit boot entry can end up pointing at the generic "Network Adapter" or
	# "DVD Drive" boot source instead of the VHDX, especially since `-VHDPath` only attaches the
	# disk — it does not guarantee the disk is first (or even present) in the firmware's boot
	# order. If the VHD entry isn't first, Hyper-V's UEFI firmware shows its own blank/logo splash
	# and never touches BOOTX64.EFI, which is exactly why efi_main was never being reached no
	# matter what tracing was added to main.rs.
	Write-Step "Setting boot order to boot from the VHDX first"
	$vhdBootEntry = Get-VMFirmware -VMName $VmName | Select-Object -ExpandProperty BootOrder |
		Where-Object { $_.Device -is [Microsoft.HyperV.PowerShell.HardDiskDrive] } |
		Select-Object -First 1
	if ($vhdBootEntry) {
		Set-VMFirmware -VMName $VmName -FirstBootDevice $vhdBootEntry.Device
	}

	Write-Step "Disabling automatic checkpoints (avoids VHDX snapshot surprises during iteration)"
	Set-VM -Name $VmName -AutomaticCheckpointsEnabled $false -ErrorAction SilentlyContinue

	Write-Step "Wiring COM1 -> $ComPipePath"
	Set-VMComPort -VMName $VmName -Number 1 -Path $ComPipePath

	Write-Step "VM '$VmName' created."
}

$vm = Get-VM -Name $VmName -ErrorAction SilentlyContinue
if (-not $vm) {
	throw "VM '$VmName' not found. Update `$VmName at the top of this script to match your actual Hyper-V VM, or remove -NoAutoInit to create it automatically."
}

# COM1 wiring only applies while the VM is off; make sure it's set even on a pre-existing VM that
# was created before this script started managing the serial port.
if ($vm.State -eq "Off") {
	Set-VMComPort -VMName $VmName -Number 1 -Path $ComPipePath
}

if ($SkipStart) {
	Write-Step "Done (SkipStart specified; VM not started)"
	return
}

# Call Start-SerialCapture BEFORE Start-VM, not after.
function Start-SerialCapture {
	param(
		[string]$PipeName = "$VmName-com1",
		[string]$LogFile  = "$PSScriptRoot\serial.log"
	)

	Remove-Item $LogFile -ErrorAction SilentlyContinue

	$job = Start-Job -ScriptBlock {
		param($PipeName, $LogFile)
		$pipe = New-Object System.IO.Pipes.NamedPipeClientStream(".", $PipeName, [System.IO.Pipes.PipeDirection]::In)
		# Block until the server side (Hyper-V) accepts the connection.
		$pipe.Connect()

		$reader = New-Object System.IO.StreamReader($pipe)
		$sw = New-Object System.IO.StreamWriter($LogFile, $true)
		$sw.AutoFlush = $true
		while (-not $reader.EndOfStream) {
			$line = $reader.ReadLine()
			$sw.WriteLine($line)
		}
	} -ArgumentList $PipeName, $LogFile

	Write-Host "Serial capture job started (Id=$($job.Id)), waiting for VM to connect..."
	return $job
}

##--------------------------------------------------------------------------------------------------
## 7. Start the VM and open a console.
##--------------------------------------------------------------------------------------------------
Write-Step "Starting VM: $VmName"

# Start the capture job BEFORE Start-VM: NamedPipeClientStream.Connect() blocks until Hyper-V's
# pipe server is ready, which only happens once the VM's emulated COM port initializes. Starting
# the listener first (instead of racing to attach after Start-VM returns) guarantees we don't
# miss any early kernel/shim output.
$serialJob = Start-SerialCapture

if ($vm.State -ne "Running") {
	Start-VM -Name $VmName
}

Write-Step "Opening VM console (vmconnect)"
vmconnect.exe localhost $VmName

Write-Step "Kernel COM1 output will not appear in the vmconnect window (it only shows the video console, which this kernel never touches)."
Write-Step "Tailing serial output below (Ctrl+C to stop). Log file: $PSScriptRoot\serial.log"

Write-Step "Done."

Get-Content "$PSScriptRoot\serial.log" -Wait
