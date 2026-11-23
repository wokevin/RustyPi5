# Copilot instructions

## Project direction

Treat the following as a staged roadmap, not a request to implement every subsystem in one change:

- **Security:** Establish a documented threat model and build layered protections with explicit trust boundaries, least privilege, safe handling of untrusted input, and fail-closed behavior.
- **OS updates:** Design an authenticated, integrity-checked update path that verifies packages before installation and supports interruption recovery and return to a known-good system. Never silently weaken verification or replace recovery behavior with a success-shaped fallback.
- **Filesystem:** Extend the existing filesystem work only after inspecting `20_timer_callbacks/kernel/src/filesystem.rs` and `20_timer_callbacks/FILESYSTEM.md`. Define storage and failure semantics, and test persistence, malformed input, and recovery where applicable.
- **On-device coding assistant:** Explore a request-driven assistant that proposes OS features as reviewable changes. Generated code must be isolated and tested; it must not be integrated, installed, or given privileged execution without explicit admin/user approval. Do not send source code or device data to an external service without approval.
- **Clang/LLVM:** The goal is to enable compilation on the OS itself. Start with a feasibility assessment and incremental prototype for the actual target; measure memory, storage, and performance constraints. Do not silently substitute remote compilation or a different compiler for on-device Clang/LLVM.

## Implementation guidance

- Inspect the current architecture and supported hardware before making platform-specific changes. The README describes Raspberry Pi 3/4 support; do not claim Raspberry Pi 5 support without verifying and testing it.
- Make each roadmap step small and independently reviewable. Explain security and compatibility tradeoffs, document limitations, and ask before making decisions that change the trust model, update policy, execution model, or target platform.
- Keep generated or downloaded code and update artifacts untrusted until explicitly verified. Avoid implicit privileged actions, unsafe parsing, and silent error handling.
- Add focused tests for new behavior and failure cases. Use the repository's existing QEMU and hardware-testing guidance where applicable, and state clearly when hardware validation was not performed.
