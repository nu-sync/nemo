# Offline process lifecycle model

This finite TLA+ model checks one Nemo daemon family: controller, model-download
helper, and native inference. Transfer activity is a boolean owned by the helper,
not a separate process. HTTP runs inside the helper; its sandbox denies subprocess
execution. This is not a proof of Rust, the native runtime, macOS or the network stack.

## Run

Requires Python 3.11+, Java 17+ (Java 21 exercised), and checksum-pinned TLC 1.7.4.

```sh
python3 scripts/verify-offline-model.py --fetch-tools --self-test \
  --output artifacts/offline-model-NEW
```

The expected result is exit **0**, `status: passed`, including both ownership and
retirement diagnostics. Exit 2 reports a property counterexample; exit 1 means
incomplete verification, tool failure or source drift. Inspect the actual
`report.json` and TLC logs. Output folders must be new.

`--fetch-tools` downloads only the developer tool
[TLC 1.7.4](https://github.com/tlaplus/tlaplus/releases/tag/v1.7.4), checking SHA-256
`936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`.
Omit that flag for offline reruns, or supply `--jar /absolute/path/tla2tools.jar`.
Nothing is added to Nemo's runtime dependencies. `--privacy-only` omits the two
stronger diagnostics and remains available for focused investigation; CI runs the
full suite.

TLC exhausts the reachable finite graph, including retry/recording cycles, without
a transition-depth cutoff. The current graph has **95 reachable states**. This is
not a coverage percentage or an unbounded-process proof. Every case uses one
worker, fixed seed/fingerprint, 512 MiB heap, and a 60-second timeout; incomplete
searches are rejected.

## Checked properties

| Invariant | Meaning |
| --- | --- |
| `TypeOK` | State values have their declared types. |
| `OperationalConfined` | Normal work starts only after successful controller confinement. |
| `AudioConfined` | Every running audio holder is confined. |
| `OfflineNoDownloadAuthority` | A sealed offline session has neither helper nor active transfer. |
| `HelperHasNoAudio` | The download helper cannot obtain user audio. |
| `InferenceConfined` | Native inference starts inside the confinement boundary. |
| `CleanupRevokesDownloadAuthority` | Completed parent-watchdog cleanup leaves no helper or transfer. |
| `DownloadHasLiveOwner` | Active transfer always has a live helper owner. |
| `UsableModelHasNoDownloadAuthority` | Usable models in idle/recording coexist with no download authority. |

`sealed` is a model observation, not a Rust field: cached startup after confinement,
or successful helper retirement and wait. It differs from weights existing and UI idle.

## Fixed lifecycle findings

Previously, abrupt helper death could orphan its curl subprocess, including before
registration. HTTP transfers now run inside the helper and `download_only` denies
`process-exec`. There is no independent download child to orphan. `HelperKilled`
therefore ends its transfer activity, even if the controller has also died before
the helper watchdog runs. The dead helper is reaped on protocol EOF/failure.

Previously, cancellation between publication and accepted success returned
`Canceled` and retained the helper. `Downloader::download` now retires and reaps
on successful publication **before** checking late cancellation. `Job::Download`
removes a retired helper independently of its UI result. Incomplete/canceled
transfers that report an ordinary error can still retry through the existing
helper. A dead helper cannot be recreated by the already-confined controller;
repair with `nemo model download` and reopen Nemo.

The checked-in protocol regressions use a deterministic fixture peer and the real
`Downloader` to check late cancellation, abrupt death, retry and malformed replies.
The real transfer/resume tests use a loopback fixture and pinned-host HEAD checks
exercise the actual HTTPS implementation inside the helper sandbox. No full model
body transfer is implied by these tests.

## Source correspondence and assumptions

Reviewed full-file hashes and action mapping live in `source-map.json`. The drift
gate is **not** Rust-to-TLA translation or a refinement proof.

| Model action/state | Rust boundary |
| --- | --- |
| `ChooseStartup`, `LockSuccess/Failure` | `run`: cached/custom models skip helper; confinement precedes UI/audio. |
| `HelperReady` | `worker`: parent/group/handler/watchdog, restrictive policy, then ready reply. |
| `StartTransfer`, `TransferCompletes` | In-process `model::transfer`; sequential HEAD/body requests and both assets coalesce into one owned activity. No subprocess registration step remains. |
| `Cancel`, `CancelCompletes` | SIGINT sets the helper's cancellation flag; transfer/hash checks stop the attempt before its error reply. |
| `VerifySuccess/Failure` | Hash-checked publication; cached `Store::ready` checks size/presence rather than a fresh full hash. |
| `CancelPublishedResult` | Late cancellation marks the request canceled but stays in retirement, not idle. |
| `DropDownloadHelper`, `ReapDownloadHelper` | `Downloader::retire` kills the child then waits; these remain separate model actions. Only after wait can cancellation return to idle. |
| `Record`, `Infer`, `InferenceExits` | Capture belongs to controller; native inference uses the confined wrapper. |
| `ControllerExits`, `ParentWatchdog`, `HelperKilled` | Controller death, watchdog termination, and abrupt helper death are separate events. |

The model assumes OS sandbox enforcement and successful process termination. It
does not prove `sandbox_init`, SBPL, native C++, TLS/DNS/URL policy, or pre-sandbox
bootstrap. `running` means able to execute, not an unreaped PID. A successful kill
ends execution atomically in this abstraction; wait remains separate.

Kill failures, kernel signal timing, PID reuse, simultaneous app families, model
replacement/deletion, local proxies, IPC/clipboard forwarding, secure erasure and
source-install networking are excluded. There are no fairness/liveness properties;
`CHECK_DEADLOCK FALSE` permits idle and terminal states. Cancellation during a
blocked HTTP read can take up to the configured 30-second read timeout. Keep the
real [offline runtime checks](../../.agents/skills/verify-offline/SKILL.md) alongside
this model.

## Negative controls and maintenance

`--self-test` catches five deliberate defects: fail-open startup, retaining a
successful helper, helper audio access, reintroducing an independent download
child, and bypassing retirement on late cancellation. The last two restore the
previous findings in the abstract model. A separate scratch Rust edit ignoring
confinement must fail the source-drift gate; that test does not infer Rust semantics.

When watched source changes, review the implementation/action correspondence first,
then refresh reviewed hashes:

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

Run the full suite afterward. Keep publication, cancellation, kill and wait separate;
do not merge non-atomic implementation actions or weaken a failed invariant to pass.

## Statechart

[lifecycle.mmd](lifecycle.mmd) is the Mermaid projection used in the PR;
[chart-view.mjs](chart-view.mjs) contains its labels/filter. It draws 14 states and
24 arrows from the 95-state checked graph. It hides repetitive controller exits,
helper deaths, capture attempts without a model, inference start/exit details and
within-label transitions. Every label remains reachable and has an outgoing arrow
when its underlying states do. Paths composed through merged labels are not
necessarily single concrete TLC traces.

With the optional statechart skill installed, regenerate outside the checkout:

```sh
scratch=$(mktemp -d)
cp verification/offline/Offline.tla "$scratch/Offline.tla"
cp verification/offline/Offline.cfg "$scratch/Offline.cfg"
printf '\nINVARIANTS DownloadHasLiveOwner UsableModelHasNoDownloadAuthority\n' >> "$scratch/Offline.cfg"
sh /Users/wcygan/.agents/skills/statechart/scripts/statechart \
  "$scratch/Offline.tla" --config "$scratch/Offline.cfg" \
  --view "$PWD/verification/offline/chart-view.mjs" --out "$scratch/chart"
# Review the report before replacing the checked-in projection.
cp "$scratch/chart/chart.mmd" verification/offline/lifecycle.mmd
```

The statechart command needs Node and its developer-only Java/TLC tooling. It is
not a Nemo build or runtime dependency. The model runner above is the standalone
verification entry point and does not require the chart skill.
