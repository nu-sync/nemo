---
name: verify-offline-model
description: Check Nemo's TLA+ offline process-capability lifecycle, source correspondence, and download ownership and retirement guarantees. Use when startup confinement, download helper lifecycle, subprocess ownership, recording transitions, or sandbox profiles change.
---
# Verify offline process authority

Read [the model's scope, source mapping and findings](../../../verification/offline/README.md)
before interpreting a pass. Work from the repository root. This is finite lifecycle
model checking, not a proof of arbitrary Rust/macOS execution or all information flow.

Prerequisites: Python 3.11+, Java 17+ (Java 21 exercised), and the checksum-pinned
TLC 1.7.4 JAR. The first command may fetch the developer-only JAR; omit
`--fetch-tools` for offline reruns. It never launches Nemo, accesses the microphone,
changes the installed app, or downloads model weights.

```sh
python3 scripts/verify-offline-model.py --fetch-tools --self-test \
  --output artifacts/offline-model-NEW
```

Read the actual new `report.json` and per-case `tlc.log` traces. Expected result
is exit 0 (`passed`), including both `ownership` and `retirement`. Exit 2 means a
property counterexample; exit 1 means incomplete verification/tool/source drift.
Require both diagnostics to pass before claiming the lifecycle fixes verified.

To rerun only the established narrower confinement properties and negative controls:

```sh
python3 scripts/verify-offline-model.py --self-test --privacy-only \
  --output artifacts/offline-privacy-NEW
```

Require a complete TLC search with no privacy invariant violation, five deliberately
broken policies rejected by their intended invariants, and the scratch Rust
fail-open edit caught by the source-drift gate. That gate protects model freshness;
it is not a Rust refinement proof. Temporary source edits are isolated and removed;
logs, copied models/configurations and counterexamples remain in the output directory.

The watched source is enumerated in `verification/offline/source-map.json`:
`src/main.rs`, `src/download.rs`, `src/backend.rs`, `src/network.rs`, `src/model.rs`,
and `scripts/local-only.sb`. On drift, review the actual changed semantics against
the action mapping before refreshing hashes using the README's command. HTTP
transfers run in the helper; its policy denies subprocess execution. Separate
publication from accepted success, and termination from wait. Permit helper death
before its parent watchdog can run. Do not combine
non-atomic implementation steps merely to eliminate a counterexample.

For a new failure, narrate the TLC trace using Nemo's processes and Rust operations,
then check each step against source. A mismapped step is a modeling error; a real
ordering that violates the property remains a finding. Do not weaken its invariant
or fix product code without authorization. If source is changed intentionally,
update the model and retain meaningful negative controls before claiming it verified.

Use verify-offline for real OS denial/transcription evidence. This model assumes OS
enforcement, successful process termination, and the sandboxed native launch;
it excludes IPC forwarding, clipboard consumers and multi-instance/PID reuse cases.
It proves no liveness/eventual-cleanup property. The three-process state graph can
cycle through repeated sessions/jobs but does not represent arbitrarily many processes.
