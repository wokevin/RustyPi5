# RustyPi5FS v1: first read-only milestone

The QEMU PC guest now uses one dedicated **194 MiB raw GPT disk**:

| Partition | Starting LBA | Length (512-byte sectors) | Purpose |
|---|---:|---:|---|
| EFI system partition | 2048 | 131072 | FAT32; UEFI shim and kernel ELF |
| RustyPi5 root | 133120 | 262144 | Custom RustyPi5FS; programs, configuration, user data |

The custom GPT partition type is `57a01418-268e-4cb0-9e97-783acd856305`.
The disk has primary and backup GPT headers/partition arrays and a protective MBR.
The EFI boot files are `EFI\BOOT\BOOTX64.EFI` and `EFI\BOOT\KERNEL.ELF`.
The existing UEFI shim and linker are still shared with the Hyper-V work; that
platform separation is not part of this filesystem milestone.

## Creating and running the disk

From `20_timer_callbacks`, with Python 3, Rust, and QEMU installed:

```powershell
.\run-qemu.ps1 -CreateDisk
```

The launcher defaults to four virtual CPUs. CPU 0 is reserved for the kernel;
user execution requires an online secondary CPU. See [CPU and memory policy](CPU-MEMORY.md).

`-CreateDisk` is an explicit formatting action and **refuses an existing image**.
Subsequent runs use `run-qemu.ps1` normally. The disk is
`20_timer_callbacks\rustypi5.img`, beside the launcher and outside `target`.
Back it up separately: it is local data, not tracked by Git. Neither a general
Cargo clean nor the launcher's mandatory build cleans delete this disk.

Each launch validates the known disk layout, then updates **only the EFI
partition** with the selected build artifacts. `-SkipBuild` reuses existing
artifacts; it does not skip disk validation. The root is never reformatted by a
normal launch. Boot-file replacement is not transactional: interruption may
require rerunning the launcher, but does not touch the custom root partition.
Do not update an image while a VM has it open.

The formatter operates on a regular image file, not a physical disk. Formatting
and image updates fail explicitly on invalid metadata or insufficient ESP space.
The kernel filesystem is read-only. QEMU IDE requires a writable disk node, so
`snapshot=on` puts any firmware/device writes in a disposable overlay rather than
changing the base image. Guest writes are not persistent in this milestone;
remove that protection only when durable filesystem writes are implemented.
There is deliberately **no write syscall**, raw-device syscall, automatic repair,
or automatic format-on-error. A mount failure is a boot error, not an empty or
fabricated filesystem.

## Shell

```text
pwd
ls
ls /etc
cat /etc/os-release
cd /home/usera
ls
cat welcome.txt
cd programs
pwd
cd ..
```

Paths can be absolute or relative; repeated `/`, `.` and `..` are supported.
`cd` with no argument goes to `/home/usera`; `ls` defaults to the current directory.
Paths are limited to 127 printable ASCII bytes. No quoting, globbing, pipelines,
links, executable loader, or executable search path is implemented yet. The
QEMU shell can interpret readable script files with `run`, using a separate
session-local script PATH; see [SCRIPTING.md](SCRIPTING.md). File loading uses
the same identity and access checks as browsing. The guest still cannot write
or create script files. `cat` displays text, replacing non-text/control bytes
with `.` rather than allowing a file to send arbitrary terminal escape sequences.

The initial layout is:

```text
/
  programs/                root-owned, 0750
  etc/                     root-owned, 0755
    os-release             0644
  home/                    root-owned, 0755
    usera/                 UID/GID 1000, 0700
      programs/            UID/GID 1000, 0700
      welcome.txt          UID/GID 1000, 0600
  tmp/                     root-owned, 01777 (writes not implemented)
  system/                  root-owned, 0700
    private.txt            root-owned, 0600
  README.txt               root-owned, 0644
```

### Authorization

The demo session's UID/GID (1000/1000) and current directory are kernel-owned;
user syscalls cannot supply or change an identity. This is still one demo
session, **not login/account management or proof of complete kernel isolation**.

Directory access requires **both read and search** permission. File browsing
requires read permission. Owner, group, then other bits are selected without
falling back to more permissive bits. Root has no special bypass in this reader.
Listings omit inaccessible entries. Path resolution checks each directory
before traversal, including before `..`; guessed names do not bypass checks.
Nonexistent and unauthorized paths both report `Path unavailable`.
`/programs` and `/system` are therefore absent from User A's root listing.
User A's private `programs` directory is visible to that user, but is initially
empty. Program grants and links are deferred; when implemented, links must
check both the link and target authorization rather than grant access themselves.

Syscalls 3/4/5/6 are provisional demo operations for `ls`/`pwd`/`cd`/`cat`.
They take a validated user path buffer and length, copied into kernel storage;
output is emitted through the kernel console and errors are reported explicitly.
These are not a finalized POSIX ABI or a file-descriptor interface.

## On-disk format

All integers are little-endian. Filesystem block size is 4096 bytes; block
numbers are relative to the custom partition. CRCs use IEEE CRC-32 (as in
Python `zlib.crc32`). CRCs detect accidental corruption, not malicious tampering.

| Block(s) | Contents |
|---|---|
| 0 | Superblock; unused bytes reserved/zero |
| 1 | Inode allocation bitmap, least significant bit first |
| 2 | Block allocation bitmap, least significant bit first |
| 3-10 | 256 inode slots of 128 bytes each |
| 11 onward | Contiguous file data extents |

The single block bitmap limits v1 to 32768 blocks (128 MiB). Directory entries
are represented by each inode's parent and name, not separate directory data
blocks. Inode IDs are dense from 0 to `inode_count - 1`; parents precede children.
This deliberately small first format is bounded and does not require the
kernel's currently tiny heap. Extents appear in nonoverlapping inode order.

Superblock offsets:

| Offset | Type | Field |
|---:|---|---|
| 0 | 8 bytes | `RPI5FS01` |
| 8 | u32 | Version = 1 |
| 12 | u32 | Block size = 4096 |
| 16 | u64 | Total filesystem blocks |
| 24 | u32 | Inode count, 1-256 |
| 28 | u32 | Inode table block = 3 |
| 32 | u32 | First data block = 11 |
| 36 | u32 | Root inode = 0 |
| 40 | u32 | CRC-32 of bytes 0-39 |

Inode offsets:

| Offset | Type | Field |
|---:|---|---|
| 0 | u32 | Kind: 1 directory, 2 regular file |
| 4, 8 | u32 each | UID, GID |
| 12 | u32 | Mode bits (07777 mask) |
| 16 | u32 | Parent inode |
| 20 | u32 | Name length, 1-63 (root alone has an empty name) |
| 24 | u64 | First data block; zero for directories/empty files |
| 32 | u64 | File byte length; zero for directories |
| 40 | 64 bytes | ASCII name, no `/`, whitespace, `.` or `..` names |
| 104 | 20 bytes | Reserved |
| 124 | u32 | CRC-32 of bytes 0-123 |

The guest validates primary GPT checksums, partition bounds, version, filesystem
bounds, inode CRCs, allocated metadata/data, inode types, parent order, names,
duplicate siblings, and file extent overflow/overlap before mounting. Unknown
versions/types fail rather than being interpreted as defaults. The initial
reader uses bounded, polled primary-master IDE PIO (LBA28) for this QEMU PC
configuration, with disk-error and timeout reporting. It is not a general SATA,
NVMe, virtio, Raspberry Pi, or Hyper-V storage driver.

Data-content checksums, durable writes, allocation/reclamation, crash recovery,
format migration, links, per-process filesystem state, login, and executable
loading remain future work. Do not enable guest writes until allocation and
recovery are implemented and tested.

## Tests

Host format tests (standard library only):

```powershell
python -m unittest discover -s .\tools -p test_disk_image.py -v
```

The standalone Rust tests compile the **same reader source** without the kernel
harness. Point `RUSTYPI5_TEST_IMAGE` at a formatter-created image (read-only tests
use in-memory sector overlays for corruptions). Clean the test output before
each build, per the project's clean-build requirement:

```powershell
New-Item -ItemType Directory -Force .\target\filesystem-tests | Out-Null
Remove-Item .\target\filesystem-tests\filesystem-tests.exe -ErrorAction SilentlyContinue
Remove-Item .\target\filesystem-tests\filesystem-tests.pdb -ErrorAction SilentlyContinue
rustc --edition=2021 --test .\tools\filesystem_tests.rs -o .\target\filesystem-tests\filesystem-tests.exe
$env:RUSTYPI5_TEST_IMAGE = (Resolve-Path .\rustypi5.img).Path
.\target\filesystem-tests\filesystem-tests.exe
```

Tests cover both GPT copies and FAT chains, exact boot-file contents, existing
image refusal, byte-for-byte root preservation during boot updates, real
filesystem output, hidden/direct-path denial, identity bit selection, malformed
metadata, extent overflow/overlap, and injected disk I/O errors.
