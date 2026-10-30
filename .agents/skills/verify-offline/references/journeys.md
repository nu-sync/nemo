# Local-only verification journeys

| Journey | Prerequisites and entry point | Actions and required result |
| --- | --- | --- |
| Real offline file inference | Local runtime/model; `scripts/verify-offline.py` | Launch actual CLI under external-network denial; prove loopback works, IPv4/IPv6 and descendant curl are denied, exact synthetic words transcribe, private log permissions, isolated data cleanup. |
| Offline menu daemon | Graphical macOS session; verifier with `--daemon` | Launch isolated daemon with cached model references; identity PID matches, idle has no download helper, warm produces one inference child, quit reaps owned processes. No microphone or clipboard operation. |
| Constrained model acquisition | Rust tests; optional network access for HEAD check | Run policy, resumable-transfer, and fixed helper protocol tests. Validate actual pinned-host redirects without transferring weights. Explicit cached `nemo model download` verifies existing weights without replacing them. |
| Dependencies and packaged notices | Cached crate sources, cargo-deny, candidate bundle | Check advisories, licenses and sources, reproduce Rust notices offline, run release checks and relocated bundle verification; inspect actual native notices. |
| Acoustic recording and paste | Installed candidate with existing macOS grants and a scratch target | Follow verify-nemo for real speakers/microphone, inference and observed paste. The controller's built-in network-deny policy remains active. Keep the scratch app foreground and never submit fixture text. |

## Downloader checks

```sh
cargo test --locked -p nemo-dictate network::tests -- --nocapture
cargo test --locked -p nemo-dictate model::tests -- --nocapture
cargo test --locked --test cli download_helper_rejects_urls_and_arbitrary_commands
```

The network policy test runs in a child so the irreversible sandbox cannot affect
other tests. Require private audio reads outside the model workspace to fail with
`EPERM`, while model reads succeed. HTTP transfer/resume tests use a **simulated
loopback boundary**, not Hugging Face. They do not establish a production transfer.

The separately ignored test makes real HEAD requests to the two pinned model URLs
inside the download-helper policy. It follows only approved HTTPS destinations,
prints only hostnames, and does not transfer model bodies:

```sh
cargo test --locked -p nemo-dictate \
  pinned_model_redirects_work_inside_download_sandbox -- --ignored --nocapture
```

Expect the approved final host printed by the test (observed `us.aws.cdn.hf.co`).
A new CDN hostname requires review; do not permit all hosts or skip TLS verification.
Downloading 21.35 GB again is a separate optional journey when full hosted-transfer
coverage is required. First launch without model files cannot transcribe offline.

## Dependency and package checks

```sh
python3 scripts/dependency-notices.py --check
cargo deny --workspace --locked check advisories licenses sources --show-stats
cargo xtask release-check
python3 scripts/verify-package.py --app artifacts/offline-candidate/Nemo.app
```

Cargo-deny may refresh its public advisory database; it does not upload recordings
or source. Record its version, database revision, target, output and date. No
advisory finding is not a complete source-level audit of every dependency.

When Cargo.lock changes, regenerate notices with
`python3 scripts/dependency-notices.py`, inspect the result and run the license
gate again. The generator reads local crate sources only. Missing upstream license
statements are checked into `assets/licenses/upstream/` with pinned provenance;
review new missing statements rather than silently substituting a generic license.
`cargo xtask check` rejects a notice inventory bound to a different lockfile.

The bundle must include Rust dependency/standard-library notices, Nemo/Handy/model
licenses, native upstream notices, and actual Homebrew library notices. Inspect
`Contents/Resources/licenses`. The relocation helper checks every shipped Mach-O
dependency and signature, including absence of external Homebrew loader paths.
