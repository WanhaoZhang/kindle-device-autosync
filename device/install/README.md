# PW2 device installation (experimental, root required)

These scripts do not jailbreak a device. They require a separately verified jailbreak/root environment on **B0D4 PW2 / firmware 5.12.2.1.1**, and reject other firmware or machines. Nothing here automatically deploys from a Mac. Do not upgrade firmware to make an unrelated package fit.

1. Build using `device/build/build.sh` on a development computer with NiLuJe `kindlepw2` C toolchain, Rust 1.91.0 and an ARM musl C compiler for bundled SQLite. Missing cross tools cause a hard error; Mac/Linux host binaries are not relabeled as Kindle binaries. ELF inspection checks ARM32, static linkage and soft-float C ABI. Generated packages have checksums but **are not signed releases and are not device-validated**.
2. After reviewing and verifying the package, copy the candidate directory to `/mnt/us/kindle-native-sync-package` (USB root `kindle-native-sync-package/`) and copy its `kual/` contents to `/mnt/us/extensions/kindle-native-sync/`. KUAL provides **Export root compatibility probe** and **Install / upgrade (collection only)** entries for first installation; no SSH is required if KUAL is already working. Safely eject USB, then run `sh PATH/install/probe.sh` from an existing root shell / trusted script launcher. The probe does not write; it prints a model prefix of only four bytes, never a full serial.
3. Review probe results, battery/space and recovery arrangements. `sh PATH/install/install.sh PATH` measures the complete candidate and recovery set against the current private-partition budget; it no longer assumes 64 MiB of free space on a roughly 64 MiB partition. See the budget below. The install briefly remounts root writable only for two independent Upstart jobs, then restores it read-only. It never stops Amazon's reader/framework/power/USB daemons.
4. Binaries, credentials and ledger live in `/var/local/kindle-native-sync`, never on a FAT partition. Upstart runs collector and event adapter at low priority. Optional KUAL files are in `kual/`; copying these to `/mnt/us/extensions/kindle-native-sync/` exposes start, stop, status, import, safe export and uninstall actions.
5. Default mode is **collection only**. The worker requires independent protocol and device validation before automatic uploads. The installer never manufactures those validations. Root self-test command: `/var/local/kindle-native-sync/bin/ksync-worker --root /var/local/kindle-native-sync self-test`.

## Handoff and credentials

The Mac bridge freezes Mac writes before handing over, requires a separate QR login/deviceId and keeps any prior unknown uploads isolated. Device collection starts strictly after the immutable cutoff; do not move old snapshots to a new empty ledger. The `.kindle-native-sync-inbox` USB directory is **temporary, unprotected credential staging**; FAT cannot enforce Unix file permissions. After safe eject, choose Import USB configuration. Only the worker may validate the hashes, atomically install auth under private root permissions, then remove USB `auth.json`. If import fails, keep Mac fenced and remove the staging auth manually when no longer needed; do not publish any inbox files.

## Event adapter and power

The C adapter blocks on wildcard LIPC events from powerd, wifid, volumd and hal. Startup reads authoritative state, establishes subscriptions, then reads again. Only complete, recognized power/connectivity/USB state-event lines are processed; signal-strength, scan, battery and other unrelated events are ignored. Unsafe transitions apply immediately; safe transitions read only the corresponding properties. Repeated same-state events do not fork getters. There is no timer loop, RTC wakeup, Wi-Fi enabling or sleep inhibition. It emits conservative states if queries fail, and stops if a listener fails. Four small firmware LIPC listener processes are part of its resource footprint; measure the whole process tree on PW2. Reader active/idle comes from the collector's native log parsing, not a guessed idle timer.

This still requires real-device tests: event subscriptions, USB transitions, full sleep/resume, reboot, log rotation, throughput and battery drain. Software cannot upload while the CPU is suspended. Upload is opportunistic on natural awake/idle/connected periods.

The adapter also owns private `adapter-control.fifo` (0600, root-owned, symlinks rejected). Every collector startup requests `refresh\n` after it opens its own control FIFO and publishes runtime state. This forces a full authoritative snapshot and replays unchanged power/network/USB state after collector respawn; it does not rely on Upstart restarting the adapter and does not add timer polling. The root probe also checks the firmware `gzip` utility, used only for idle catch-up of rotated native logs.

## Root mount verification

The installer and compatibility probe share the same kernel mount-state check. The firmware native `mntroot` helper may return zero even when its remount failed, so its return code alone never proves success. After requesting `rw` or `ro`, installation requires every non-`rootfs` `/` entry in `/proc/mounts` to have the requested state. Mixed, malformed or missing real-root entries are rejected. A native nonzero exit is reported but does not fail an operation whose requested mount state is independently verified. The always-writable `rootfs` pseudo entry is not the on-disk system partition.

## Upgrade, rollback and removal

Install/upgrade stages the complete new binaries, installation scripts, manifest and jobs before replacing active components. `upgrade.previous` keeps one complete old set: binaries, installation scripts, manifest, optional CA, device-acceptance evidence, jobs and their previous running states. A further upgrade refuses to overwrite an unreviewed recovery set or transaction residue. Copy/remount/reload/start failures restore the old set and previous service state, then restore root read-only; incomplete recovery retains evidence and refuses guessed cleanup. An already-running adapter started by Upstart is accepted, not treated as a failed second start.

Successful upgrades remove active device-acceptance evidence while preserving its archived copy; new binaries require new device/performance acceptance. The complete backup is synchronized first; active acceptance is removed and synchronized before any binary cutover. This prevents a new binary inheriting old performance acceptance across that transition. It is **not** a promise of automatic power-loss rollback: interruption markers/recovery copies must be reviewed before retrying. Auth/config/ledger are never reset. Installation also refuses a pre-existing explicit `disabled` flag rather than silently re-enabling collection.

Uninstall stops only this project's services and removes project executables, scripts, backups and staging, while **keeping private ledger/auth/config/data and device evidence**. It removes the obsolete `disabled` runtime flag after successful removal, so an explicit future Install is possible without a missing control-script dependency. It does not claim to reclaim all disk space. Inspect and separately erase private data only when no upload is unresolved. Do not delete a ledger merely to retry a cloud request.

## PW2 private-partition budget

The single shell budget source is `install/common.sh` (`pw2-private-v1`):

| Allowance | Size |
| --- | --- |
| Raw event queue | 4 MiB |
| SQLite main ledger | 4 MiB |
| SQLite rollback journal | 4 MiB + 64 KiB header/page-record overhead |
| Space retained for Kindle system use | 20 MiB |
| Filesystem/metadata margin | 1 MiB |

The runtime requirement adds only the **unused** raw/ledger allowances to the journal, system and metadata reserves; current occupied files are already reflected in `df`. The installer additionally measures the complete new staging tree and complete old recovery copy, including old jobs, to cover the transaction peak. `upgrade.previous` consumes real partition space reflected in free space and is never ignored or overwritten to make an upgrade fit. Probe exposes `required_runtime_free_kib`, `required_install_free_kib`, `retained_backup_kib` and the fixed storage profile. `supported` describes hardware plus runtime space; `installation_supported` separately checks the installation peak. Thus an already-installed device may safely run but lack space for another transactional upgrade.

An existing ledger whose logical file length exceeds 4 MiB blocks installation explicitly: there is no automatic downgrade/size reduction of a previously larger ledger. An oversized raw queue is retained and reported as blocked; no unsent event is deleted to fit the profile. A full queue or low reserve must stop collection safely. If an upgrade cannot retain its full recovery set and reserves, it is refused; the installer never repartitions or deletes Amazon data. Separately uninstalling while preserving data and then installing another version requires explicit user actions and is not an automatic bypass of the transaction budget.

Root probe output and worker status can be explicitly exported to `.kindle-native-sync-export/`; neither raw logs, auth nor ledger is exported by the control script.

## Experimental compiler fallback

`KSYNC_C_PROFILE=experimental-static-musl` permits an explicit soft-float ARM musl compiler wrapper through `KSYNC_CC`; the default remains NiLuJe kindlepw2. This fallback is labeled in the package manifest and is only a static ARM candidate, not a compatibility claim. `KSYNC_MUSL_CC` independently supplies the hard-float ARM musl compiler for SQLite in the Rust worker; optional `KSYNC_MUSL_AR` supplies a matching archiver. Do not mix hard-float SQLite objects into soft-float dynamically linked C helpers.

For the reproducible Zig 0.13.0 candidate path, obtain the official release archive separately and verify its published checksum. The ARM macOS `zig-macos-aarch64-0.13.0.tar.xz` archive SHA-256 is `46fae219656545dfaf4dce12fb4e8685cec5b51d721beee9389ab4194d43394c` (archive checksum, not the extracted executable). Then set `KSYNC_ZIG` to the extracted executable and run:

```sh
KSYNC_ZIG=/absolute/path/to/verified/zig ./device/build/build.sh --experimental-static-musl
```

The checked-in wrappers choose distinct soft-ABI collector/adapter and hard-ABI musl SQLite targets; the hard-ABI wrapper removes only cc-rs's known incompatible `--target=arm-unknown-linux-musleabihf` argument. No wrapper downloads tools, executes on Kindle, or enables validation flags. The build requires the executable's reported version to be exactly 0.13.0. For other host architectures, use the corresponding official archive and its own published checksum.

## Replacing a stopped, unconfigured preview on a small private partition

`preview-upgrade.sh PACKAGE` is a separate, explicit route for the early collection-only preview. It is **not** an override for an account-enabled installation. It requires all of the following before any replacement:

- A real `disabled` flag, both project Upstart jobs stopped, and no config/auth/device-acceptance file.
- The existing worker can read its private ledger and reports zero attempts and an unconfigured state.
- No unresolved transaction marker; the new package passes checksum and ARM32 checks.
- The runtime/system budget still fits after exchanging old and new components.

The complete old executable/script/manifest/job set is copied to `.kindle-native-sync-recovery/preview-…/` on USB and verified against hashes of the old files before private components are removed. Credentials, ledger, counters, checkpoints and captured reading logs are never moved to USB or deleted. A small private `.preview-upgrade` marker records the recovery location before cutover. New private files and jobs are read back and hashed; root is checked read-only. Collection remains disabled until the later configuration stage explicitly enables it.

Controlled failures restore the complete stopped old component set from the verified USB copy. Recovery failure or interruption keeps the marker; ordinary install, enable and uninstall refuse to bypass it. After a power interruption, keep the same verified USB recovery directory and choose the explicit preview recovery action; do not delete the marker or build a fresh ledger. This is not an automatic power-loss rollback guarantee. The ordinary upgrade of configured installations retains its original full private staging/recovery budget and refuses insufficient space.

KUAL `Setup stopped preview + import + reboot` then performs independent stages: replacement, local self-test, USB configuration import, OTA guard, collection enable, and an explicitly requested reboot. Once replacement succeeds its marker is cleared. If configuration import or OTA protection later fails, **new components and all mutable account/ledger state remain**, with `setup-result.json` describing the failed stage. `Resume imported setup + protect + reboot` resumes those stages after the cause is fixed, without pretending an imported account is an unconfigured preview. Setup never enables Wi-Fi and never submits reading time.
