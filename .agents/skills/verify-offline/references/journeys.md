# Local-only verification journeys

| Journey | Prerequisites and entry point | Actions and required result |
| --- | --- | --- |
| Real offline file inference | Local runtime/model; `scripts/verify-offline.py` | Launch actual CLI under external-network denial; prove loopback works, IPv4/IPv6 and descendant curl are denied, exact synthetic words transcribe, private log permissions, isolated data cleanup. |
| Offline menu daemon | Graphical macOS session; verifier with `--daemon` | Launch isolated daemon with cached model references; identity PID matches, idle has no download helper, warm produces one inference child, quit reaps owned processes. No microphone or clipboard operation. |
| Constrained model acquisition | Rust tests; optional network access for HEAD check | Run policy, resumable-transfer, and fixed helper protocol tests. Validate actual pinned-host redirects without transferring weights. Explicit cached `nemo model download` verifies existing weights without replacing them. |
| Dependencies and packaged notices | Cached crate sources, cargo-deny, candidate bundle | Check advisories, licenses and sources, run release checks and relocated bundle verification; inspect the bundle dependencies. |
| Acoustic dictation to clipboard | Installed candidate with existing macOS grants; `scripts/verify-clipboard.py` | Follow verify-nemo for real speakers/microphone, inference and expected clipboard text under network denial. No editor or foreground target is required; visible paste is a separate explicit check. |

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
cargo deny --workspace --locked check advisories licenses sources --show-stats
cargo xtask release-check
python3 scripts/verify-package.py --app artifacts/offline-candidate/Nemo.app
```

Cargo-deny may refresh its public advisory database; it does not upload recordings
or source. Record its version, database revision, target, output and date. No
advisory finding is not a complete source-level audit of every dependency.

License metadata checks do not establish complete notice obligations for a
redistributed binary. That release work is separate from the offline lifecycle check.
The relocation helper checks every shipped Mach-O dependency and signature,
including absence of external Homebrew loader paths.
