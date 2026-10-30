# Offline process lifecycle model

This finite TLA+ model checks the authority held by one Nemo daemon family:
controller, model-download helper, one curl subprocess, and one inference runtime.
It is not a proof of the whole Rust program, native runtime, macOS, or network stack.

## Run

Requires Python 3.11+ and Java 17+; Java 21 was exercised. Set `JAVA` to the Java
executable if needed (on this Mac, `/opt/homebrew/opt/openjdk@21/bin/java`).

```sh
python3 scripts/verify-offline-model.py --fetch-tools --self-test \
  --output artifacts/offline-model-NEW
```

The first `--fetch-tools` downloads only the developer tool
[TLC 1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), checking SHA-256
`936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`.
Subsequent runs can omit that option and run offline. Nothing is installed in Nemo
or added to its runtime dependencies. An existing matching JAR can be supplied with
`--jar /absolute/path/tla2tools.jar`.

**The full command currently exits 2: two stronger properties have counterexamples.**
That is a finding, not a successful proof of those properties. Each new output
directory retains the exact model/configuration, TLC log/trace and `report.json`.
Exit 1 is a runner/tool/source-drift failure, not evidence about the model.

The smaller confinement check, excluding the two diagnostic properties, passes:

```sh
python3 scripts/verify-offline-model.py --self-test --privacy-only \
  --output artifacts/offline-privacy-NEW
```

There is no fixed transition-depth cutoff. TLC exhausts the reachable graph for
the finite actor/state abstraction, including cycles representing retries and
repeated recordings. This is not an unbounded-process or Rust execution proof.
The examined baseline has 226 distinct states; the count is a regression reference,
not a coverage percentage or assurance score. Each case uses one TLC worker, a
fixed seed/fingerprint choice, a 512 MiB heap cap and a 60-second timeout. A timeout,
parser error or incomplete search is rejected rather than treated as a pass.

## Checked properties

| Invariant | Meaning and user harm it guards against |
| --- | --- |
| `OperationalConfined` | Normal work starts only after successful controller confinement; a sandbox error must not silently permit external connections. |
| `AudioConfined` | Every running audio holder is confined; audio must not share a process with direct external-network authority. |
| `OfflineNoDownloadAuthority` | A sealed offline session has no executing helper/curl; successful handoff must revoke download authority. |
| `HelperHasNoAudio` | The model helper cannot obtain user audio; a network-capable process must not receive recording data. |
| `InferenceConfined` | The modeled inference launch remains confined; restarting native inference must not grant external authority. |
| `CleanupRevokesDownloadAuthority` | Once a live helper's parent watchdog has completed group termination, neither helper nor curl can execute; cleanup must cover an unregistered child. |

`TypeOK` additionally checks state types. `sealed` is a model observation, not a
Rust field: cached startup after confinement, or successful helper retirement and
wait. It is deliberately different from “weights exist” and from UI `Idle`.

## Findings retained by the full check

### Helper termination can orphan curl

`Ownership.cfg` checks `DownloadHasLiveOwner`. TLC's counterexample is:

1. Start without models; helper becomes ready; controller installs confinement.
2. Begin download; helper spawns curl before registering its handle.
3. The helper is abruptly killed; curl remains executing without its owner.

The registered-child case has the same underlying issue. The helper watchdog in
`src/download.rs::worker` observes **controller** death, but cannot run after the
helper itself receives SIGKILL. `Downloader::download` reports EOF without dropping
the `Downloader`; the main worker retains it after an error. Process-group cleanup
in `Downloader::drop` therefore need not happen immediately. This is a source-backed
failure-handling gap, not a reproduced audio leak or remote exploit. No live model
download was force-killed to validate it. The model also permits helper death after
controller death, before its watchdog executes.

### Cancellation can retain authority after publication

`Retirement.cfg` checks `UsableModelHasNoDownloadAuthority`. Its counterexample is:

1. Download completes; curl exits; the helper verifies/publishes usable weights.
2. Cancellation arrives before the controller accepts the helper's successful reply.
3. `Downloader::download` returns `Canceled`; `Job::Download` retains the helper
   because its result is an error. Recording/inference can subsequently use the model.

The model includes this path as `CancelPublishedResult`. The helper is still
capable of model networking; the trace does **not** establish that it sends a new
request or can read audio. Thus “a usable model always means download authority is
gone” is stronger than the current implementation. The privacy invariants still pass.
Neither finding is fixed or hidden by changing its expected property in this PR.

## Source correspondence and assumptions

The reviewed mapping and full-file hashes are in `source-map.json`. The runner
rejects changed source until a reviewer revisits the mapping. Hash matching is a
drift alarm, **not automatic Rust-to-TLA translation or a refinement proof**.

| Model action/state | Rust boundary being abstracted |
| --- | --- |
| `ChooseStartup`, `LockSuccess`, `LockFailure` | `src/main.rs::run`: cached/custom models skip the helper, then `network::local_only()?` precedes UI/recording. Custom models are represented by the local-model startup branch. |
| `HelperReady` | `src/download.rs::worker`: expected parent, process group, handler/watchdog, download policy, then `ready` reply. |
| `SpawnCurl`, `RegisterCurl` | `src/backend.rs::Process::run`: actual spawn precedes mutex-protected registration. These are separate transitions. |
| `Cancel`, `CancelCompletes` | Parent cancellation after the `started` handshake; the helper kills a registered child, or checks cancellation when registering a just-spawned child. The completion transition coalesces internal steps with no additional network authority. |
| `CurlCompletes`, `VerifySuccess/Failure` | Sequential HEAD/body subprocesses and both manifest assets abstract to one representative curl slot, followed by integrity checking/publication in `src/model.rs`. Cached `Store::ready` checks size/presence, not a fresh full hash. |
| `DropDownloadHelper`, `ReapDownloadHelper` | `Job::Download` drops the helper on success; group kill and wait are separate actions. |
| `Record`, `Infer`, `InferenceExits` | Main event loop owns capture; backend launches the native runtime using the confined wrapper and inherited controller restriction. Inference has no authority beyond that OS boundary. |
| `ControllerExits`, `ParentWatchdog`, `HelperKilled` | Parent death, watchdog group kill, and abrupt helper death are distinct events. |

The model assumes macOS correctly enforces the specified sandbox, process-group
membership/inheritance, and successful termination operations. The inference
launch abstraction assumes fail-closed execution of that boundary; it does not
verify `sandbox_init`, `sandbox-exec`, SBPL semantics or the C++ runtime. Likewise,
helper audio isolation abstracts the filesystem/microphone policy and fixed pipe;
pre-sandbox bootstrap code is trusted, not verified by this model.

`running` means able to execute, not an unreaped PID. A successful modeled group
kill removes execution authority atomically; `ReapDownloadHelper` separately models
the wait before declaring the handoff complete. Kill failures, detailed kernel
signal timing, PID reuse, multiple concurrent CLI/daemon families, model deletion
between runs, and automatic child reparenting behavior are outside this abstraction.
No fairness assumptions or liveness checks establish eventual cleanup/cancellation;
`CHECK_DEADLOCK FALSE` permits idle/terminal states intentionally.

Loopback proxies, local IPC forwarding, clipboard consumers, receiving apps, URL/TLS
validation, filesystem confidentiality beyond the modeled helper restriction,
secure erasure, and source installation networking are excluded. Keep the real
[offline runtime checks](../../.agents/skills/verify-offline/SKILL.md) alongside this model.

## Negative controls and maintenance

`--self-test` requires counterexamples for four deliberately broken model policies:
ignoring sandbox failure, retaining a successful helper, permitting helper audio
access, and killing only a registered curl handle on parent loss. It also changes
the real Rust `run` confinement call to ignore errors in a disposable source copy,
requires the source-drift gate to reject it, removes that copy, then rechecks the
unmodified tree. The Rust edit is detected by hashes, not inferred automatically
by TLC. The model mutations exercise the mapped failure behavior separately.

When modeled source changes, inspect the diff, revise the actions/assumptions and
trace mapping first, then update only the reviewed hashes:

```sh
python3 - <<'PY'
import hashlib, json
from pathlib import Path
p = Path('verification/offline/source-map.json')
bindings = json.loads(p.read_text())
for name, entry in bindings.items():
    entry['sha256'] = hashlib.sha256(Path(name).read_bytes()).hexdigest()
p.write_text(json.dumps(bindings, indent=2) + '\n')
PY
```

Run both the full diagnostic and privacy/self-test commands afterward. Preserve
the failing property when the code violates it; do not make the model pass by
assuming away a reachable ordering. Future fixes should make the corresponding
diagnostic pass because the modeled implementation changes.
