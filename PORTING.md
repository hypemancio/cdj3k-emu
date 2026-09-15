# PORTING.md — cdj3k-emu on Intel Mac (x86_64 host, QEMU TCG)

This branch (`intel-port`) ports cdj3k-emu to Intel Macs by replacing HVF
acceleration with QEMU TCG (full binary translation). The guest stays
aarch64 — the real CDJ-3000 firmware is unchanged. Reference machine:
MacBook Pro 15" mid-2014 (i7-4770HQ, 16 GB, macOS Sequoia 15.7.9 via
OpenCore Legacy Patcher).

Motivation: network-protocol reverse engineering (Pro DJ Link, mixer side).
Packet correctness is the only priority; a 20-50x guest slowdown is
accepted. Audio is intentionally unsupported under TCG (see below).

Every change on this branch is tagged `[intel-port]` in a comment, so each
piece can be extracted into an upstream PR.

## 1. Where the Apple Silicon assumption lives

| Place | Assumption | Fate |
|---|---|---|
| `qemu/build.sh` configure block | `--enable-hvf` — hard configure failure on x86_64 (QEMU builds HVF only when host ISA == target ISA) | Gated on `uname -m` |
| `crates/cdj3k-emu-runtime/src/config.rs` (`QemuConfig::new`) | HVF default unless `CDJ3K_EMU_TCG=1`; HVF branch passes `-cpu host` | Default now `host::tcg_active()`-driven; TCG branch emits explicit `-accel tcg,thread=…` |
| `crates/cdj3k-emu-runtime/src/config.rs` (`build_argv`, GIC selection) | in-kernel vGIC via `hv_gic_create` on macOS 15+ | Already conditional on `self.hvf` — self-disables under TCG (`kernel-irqchip=off`) |
| `bundle.sh` (socket_vmnet download) | Hardcoded `socket_vmnet-*-arm64.tar.gz` + pinned SHA | **Deferred** — only needed for bridged networking on a physical interface (phase 2). The user-mode SLIRP fallback needs no helper |
| `bundle.sh` (Info.plist + codesign) | `com.apple.security.hypervisor` entitlement | Left in place — unused under TCG, harmless |
| Host-side timeouts (`crates/cdj3k-emu-runtime/src/instance.rs`, `crates/cdj3k-emu-ui/src/app.rs`) | Waits sized for a near-native-speed guest (8 s EP122, 20 s ACPI, 5 s QMP quit, 35 s UI shutdown watchdog) | Scaled by `host::guest_time_scale()` — see §3, the most important change on the branch |
| `qemu/patches/12-hvf-vcpu-qos.patch` | Patches `accel/hvf/hvf-accel-ops.c` (vCPU thread QoS) | Still applies cleanly; dead code under `--disable-hvf` (file never compiled) |
| `boot.sh` (dev-only path) | `-accel hvf -cpu host` hardcoded; the parsed `--no-hvf` flag is dead | **Deferred** — not used by the .app |

**What is *not* arch-bound** (verified):

- The Rust workspace has **no `target_arch` cfg anywhere** — every gate is
  `target_os = "macos"`. The app builds host-native (`bundle.sh` passes no
  `--target`), so an Intel Mac produces an x86_64 binary with zero changes.
- QEMU patches 01–11 (ivshmem enablement, shm-display, coreaudio bypass,
  virtio-snd bypass ring, main-loop QoS, virtio-blk removable, console-vc):
  portable C, `CONFIG_DARWIN`-gated where Darwin-specific, no NEON/arm asm.
- The guest build (`docker/`, `guest/`, `build.sh`) targets `linux/arm64`
  by definition and must stay that way.
- The firmware wizard, including the `smc→hvc` rewrite in
  `crates/cdj3k-emu-firmware/src/extract.rs` — PSCI on the `virt` machine
  goes through HVC regardless of accelerator, so the patch is still needed
  under TCG.
- vmnet/tapbridge plumbing: `vmnet.framework` and `socket_vmnet` exist on
  Intel Macs; `vmnet.rs` already probes the Intel Homebrew prefix
  (`/usr/local/bin/socket_vmnet`).

## 2. What changed and why

1. **`crates/cdj3k-emu-platform/src/host.rs`** — new predicates, single
   source of truth: `hvf_supported()` (compile-time:
   `target_os=macos && target_arch=aarch64`), `tcg_active()` (no HVF, or
   `CDJ3K_EMU_TCG=1` — pre-existing semantics preserved),
   `guest_time_scale()` (1 under HVF; default **50** under TCG, override
   `CDJ3K_EMU_TCG_SCALE`).
2. **`config.rs`** — `hvf` default flows from `tcg_active()`; the TCG
   branch emits an explicit `-accel tcg,thread=<mode>` (`multi` default,
   `CDJ3K_EMU_TCG_THREAD=single` fallback, see §5) and keeps
   `-cpu cortex-a72` (matches the RK3399 big cores; avoids `-cpu max`'s
   pathologically slow pointer-auth emulation under TCG).
3. **`instance.rs` / `ui/src/app.rs`** — guest-bounded waits multiplied by
   `guest_time_scale()` (see §3).
4. **`app/cdj3k-emu/src/main.rs` + `runtime_worker.rs`** — audio forced off
   under TCG regardless of saved settings (see §4).
5. **`qemu/build.sh`** — `--enable-hvf` / `--disable-hvf` gated on
   `uname -m`; `--enable-slirp` made explicit (the app's fallback netdev is
   user-mode; a silently missing libslirp would only fail at QEMU runtime).
6. **`build.sh --artifacts-only`** + **`.github/workflows/guest-artifacts.yml`**
   + **`scripts/install-guest-artifacts.sh`** — the Docker guest stage needs
   only the repo + submodule, so CI on `ubuntu-24.04-arm` (native arm64,
   free on public repos) builds the kernel/modules/tools and publishes them
   as an artifact; the install script drops them where `bundle.sh` expects.
   The proprietary Pioneer rootfs never enters CI.

## 3. Timeout scaling — the change that actually matters

Under TCG every guest-side operation takes 20-50x longer in wall-clock. The
host-side shutdown sequence (`instance.rs::shutdown_sequence`) budgets
8 s + 20 s + 5 s and then falls through to SIGKILL; the UI shutdown
watchdog (`app.rs`) caps the whole worker at 35 s before SIGKILL. Unscaled,
**every** app exit under TCG would take the SIGKILL path and risk a dirty
eMMC qcow2 — silent image corruption, the worst possible failure mode for a
long-running RE setup.

Scaled waits: `EP122_CLEANUP_WAIT`, `ACPI_SHUTDOWN_WAIT`, `QMP_QUIT_WAIT`,
`SHUTDOWN_WATCHDOG`, and (mildly, capped at 4x — the QMP listener comes up
during host-speed machine init) the QMP boot connect. NOT scaled:
`SIGTERM_GRACE` and the poll cadences, which bound host-side process
signalling.

The default factor is 50 — deliberately the high end of the observed
range. Every scaled site is an early-exit poll: a clean shutdown still
returns in seconds; only the worst case stretches (ACPI wait worst case
≈ 17 min). A long worst case is preferred over a corrupted image. Tune
with `CDJ3K_EMU_TCG_SCALE=<n>` once real boot/shutdown times are known.

## 4. What breaks or degrades under TCG

- **Speed**: 20-50x slower guest. Boot to full firmware may take tens of
  minutes. Accepted by design.
- **Audio: unsupported, cut at the root.** With `config.audio = false` no
  `virtio-sound-device` is emitted; the guest auto-loads `snd-dummy` so
  EP122/JUCE still sees an ALSA card (upstream-supported state, see the
  comment in `build_argv`). The guest-side pipeline-depth watchdog lives in
  `guest/modules/virtio_snd/virtio_snd.c` and never runs without the
  device (its probe never fires); even when it does run it only forces an
  ALSA xrun — it never kills the instance. The TCG builds force audio off
  even if a stale `audio_enabled=1` survives in instance settings.
- **Shutdown/boot windows multiplied** (§3): worst-case waits are minutes,
  not seconds. The typical path is unaffected.
- **`qemu/patches/12-hvf-vcpu-qos.patch` is inert** (applies at source
  level, never compiled). A TCG equivalent would target
  `accel/tcg/tcg-accel-ops-*.c`; not needed with audio off.
- **Jog "wait" badge flicker**: `jog_stream.rs` flags the jog stream stale
  after 2 s — cosmetic only under TCG.

### Troubleshooting an inexplicably hung boot

First move: retry with `CDJ3K_EMU_TCG_THREAD=single`. MTTCG (multi-threaded
TCG) is the default and is formally sound for an aarch64 guest on an
x86_64 host (the host's x86-TSO memory model is stronger than the guest's
— the safe direction), but MTTCG has a history of subtle guest-visible
bugs. `thread=single` serialises all vCPUs on one thread: slower, but a
different (older, better-tested) execution model. No rebuild needed.

## 5. Guest time and Pro DJ Link timing  ← central for the RE goal

There is **no time virtualisation** on this branch: the guest reads real
wall-clock time (no `-icount`, no `-rtc clock=vm`) while executing 20-50x
slower. Consequences for Pro DJ Link work:

- Timestamps *inside* captured packets and inter-packet intervals of
  guest-originated traffic will not match real-hardware cadence: the
  firmware will *try* to hit its keepalive/announce/beat intervals on
  wall-clock but will systematically miss them when the CPU can't keep up.
- Real peers on a bridged network may treat the emulated player as
  unstable or time it out.
- Passive decoding of the *format* of mixer packets (field layout,
  opcodes, state machines) is unaffected — only timing-sensitive behaviour
  is distorted.

The alternative is `-icount` (deterministic guest-virtual time), which
would make guest-side intervals self-consistent — at the cost of forcing
single-threaded TCG (slower still) and decoupling guest time from
wall-clock entirely (breaks interaction with real peers).

**Decision policy (per project owner): no icount decision on estimates —
measure first.** When the RE phase starts, capture with pcap on the
emulated link and quantify the drift: expected-vs-observed keepalive
intervals (protocol nominal vs measured deltas), announce cadence, beat
packet spacing at a known BPM. Only with those numbers on the table decide
between wall-clock (status quo), `-icount`, or a hybrid workflow (e.g.
wall-clock for interactive capture, icount for cadence studies).

## 6. Environment knobs

| Variable | Effect |
|---|---|
| `CDJ3K_EMU_TCG=1` | Force TCG even where HVF is available (pre-existing) |
| `CDJ3K_EMU_TCG_SCALE=<n>` | Timeout multiplier under TCG (default 50) |
| `CDJ3K_EMU_TCG_THREAD=multi\|single` | TCG threading mode (default `multi`); `single` is the first fallback for a hung boot |
| `CDJ3K_SERIAL_LOG=1` | Log the guest PL011 console to `/tmp/cdj3k-emu-<UID>/instance-<id>/serial.log` (same as `--serial-log`) |

## 7. Build recipe (Intel Mac)

```bash
# 0. Host deps (Intel Homebrew in /usr/local)
xcode-select --install
brew install meson ninja pkg-config glib pixman libslirp
# Rust stable via rustup (defaults to x86_64-apple-darwin)

# 1. Clone this fork, intel-port branch
git clone --recurse-submodules <fork-url>
cd cdj3k-emu && git checkout intel-port

# 2. Guest artifacts — either download the "guest-artifacts" artifact from
#    the GitHub Actions run and install it:
./scripts/install-guest-artifacts.sh ~/Downloads/guest-artifacts.zip
#    ...or build locally (slow: Docker emulates arm64 via binfmt on x86):
# ./build.sh --artifacts-only

# 3. QEMU (TCG-only configure on Intel; expect 30-60 min on a 2014 MBP)
./qemu/build.sh
#    Check: configure summary shows slirp support and no HVF;
#    qemu/install/lib/libcdj3k-emu-qemu.dylib and bin/qemu-img exist.

# 4. App bundle (ad-hoc signing; the hypervisor entitlement is unused)
./bundle.sh          # → dist/CDJ3K Emulator.app

# 5. First run: the firmware wizard provisions from the .UPD + key
#    (host-speed, minutes), then auto-boots under TCG.
CDJ3K_SERIAL_LOG=1 "./dist/CDJ3K Emulator.app/Contents/MacOS/cdj3k-emu"

# 6. Watch the boot (second terminal). First kernel lines within minutes;
#    full userspace possibly 30-40 min.
tail -f /tmp/cdj3k-emu-$(id -u)/instance-1/serial.log
```

Boot success looks like: `Booting Linux on physical CPU 0x0` →
`Linux version 6.6.x` → virtio-mmio/PL011 probes → `Freeing unused kernel
memory` → first init/systemd lines.

Sanity-check the spawned argv (printed on spawn): it must contain
`-accel tcg,thread=multi`, `-cpu cortex-a72`, `kernel-irqchip=off`,
`-netdev user`, and **no** `-audiodev` / `virtio-sound-device`.

## 8. Upstreaming

All changes are tagged `[intel-port]`. Suggested PR split, each
independently mergeable:

1. `qemu/build.sh`: arch-gated HVF + explicit slirp.
2. `host.rs` predicates + `config.rs`/`main.rs`/`runtime_worker.rs`
   consumers (TCG default, explicit accel, audio guard).
3. Timeout scaling (`instance.rs`, `ui/src/app.rs`).
4. `build.sh --artifacts-only` + CI workflow + install script.
5. Deferred items as they land (socket_vmnet arch-select in `bundle.sh`,
   `boot.sh` `--no-hvf` wiring, `usb.rs` sleep scaling).
