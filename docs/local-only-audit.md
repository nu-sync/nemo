# Local-only operation and audit

Nemo performs inference on this Mac. Normal controller and native inference
processes deny external networking at the OS boundary. Loopback TCP and local
Unix sockets remain available. Sandbox setup errors stop the operation; there is
no opt-out or cloud fallback.

## Network authority

`src/network.rs` applies the embedded `scripts/local-only.sb` policy before normal
CLI work or daemon UI/recording starts. Inference additionally starts through
`sandbox-exec` with that policy and an empty inherited environment. The HTTP client
disables proxies and redirects and addresses only `127.0.0.1`. A random bearer key
authenticates transcription requests to the local native runtime.

A daemon with cached models creates no network-capable download helper. When
managed weights are missing, it starts a dedicated helper before restricting the
controller. The same scoped grant is used by `nemo backend setup mlx-audio` only
when that profile is not already usable: the helper is started before the command
applies `local_only()`, then receives the pinned profile name (never a URL or
arbitrary destination) and downloads into that profile's managed revision directory.
The command verifies every pinned digest after acquisition. The helper is retired
and reaped on successful acquisition; failures and cancellation also retire it.
A subsequent setup with usable weights starts no helper and performs local hash
verification only. The private pipe accepts only the `download` operation, with no
URL, file contents, audio or transcript parameter. The helper cannot read ordinary
user files, cannot use the microphone, can write only its model workspace, and may
connect only over TCP port 443. It exits after successful acquisition or when its
controller dies.
The helper performs HTTP transfers in-process and its sandbox denies subprocess
execution, so killing it also ends its download activity.

The helper constructs model URLs from the embedded pinned manifest. It checks
every HEAD redirect against an explicit HTTPS host list before requesting it;
the body request does not follow redirects. The HTTP client disables proxies and automatic redirects; its requests contain
no user credentials. Size and SHA-256 must match before publication.
The OS port restriction and the application host restriction are distinct controls:
the sandbox does not itself authenticate DNS names. TLS authenticates the approved
host. Observed downloads redirect from Hugging Face to `us.aws.cdn.hf.co`.

The menu verifies already-cached weights locally. If files disappear or are
damaged after startup, use `nemo model download` to repair them, then reopen Nemo;
the restricted controller cannot grant itself a new external-network capability.

The [lifecycle model](../verification/offline/README.md) checks both previously
failing cleanup properties: transfer activity cannot outlive its helper, and
publication followed by cancellation must retire the helper before returning to
idle/recording. Verified success and broken helper protocols terminate and reap
the helper; ordinary incomplete/canceled transfers retain it for retry. Once the
helper has died, repair missing models with the explicit CLI and reopen Nemo.
The full diagnostic is expected to pass under its documented OS assumptions.

Building and installing from source is a separate network-capable workflow:
Cargo, Homebrew, GitHub/submodules and signing timestamps may contact their servers.
An unbundled bare `nemo` can bootstrap installation. These development/install
operations are not covered by the model-only runtime allowance.

## Local data

Normal recording and WAV encoding use memory, and native transcription handles
uploaded bytes in memory. There is no controller recording or transcript-history
file. CLI transcription deliberately prints its output. The system clipboard
retains the last delivered transcript; clipboard synchronization, clipboard
managers and the application receiving pasted text have their own behavior.

Daemon and inference logs have mode 0600. Timing events omit audio/transcript, and
HTTP failure bodies are not included in errors. Clearing the native environment
also removes inherited debug tracing and optional backend/model settings.
This is not a secure-erasure guarantee for RAM, swap, OS diagnostics or clipboard.

## Licenses and dependencies

Nemo is MIT; NeMo-Speech.cpp is Apache-2.0 with its upstream third-party notices;
the model is OpenMDW-1.1; Handy-derived sounds are MIT. These licenses permit
free-of-charge use subject to their terms and retained notices. There is no Nemo
account, subscription or inference API requirement.

The macOS Rust graph is checked against a permissive-license policy and approved
registry sources using `deny.toml`. This check does not generate or collect license
texts. Distribution notice completeness is outside this lifecycle experiment.

## Verification and limits

Use [.agents/skills/verify-offline](../.agents/skills/verify-offline/SKILL.md) for exact
journeys and evidence requirements. Real CLI inference under external-network
denial, local-loopback success, denied IPv4/IPv6 connections, and denied descendant
curl were exercised on macOS 26.5.2 / Apple Silicon. The production model hosts were
checked using HEAD requests inside the constrained helper. Full model transfer was
not repeated; transfer/resume/integrity regressions use a loopback fixture.

The source audit reviewed the tracked textual repository and traced the native
local-model serving, authentication, input decoding, network and logging paths.
No substantiated attacker-to-impact vulnerability was established. Runtime network
authority was a hardening gap addressed by this change.
This is not an exhaustive audit of llama.cpp, ggml, audio parsers, macOS/GPU drivers,
or every dependency's implementation. Authentication-key confidentiality across
different local accounts remains untested; the current native CLI receives it in
argv. Same-account malicious code and a compromised OS are outside this boundary.

Evidence is kept under ignored `artifacts/local-only-audit/`, including failed
attempts. The first externally sandboxed daemon attempt exposed macOS's rejection
of a different nested helper policy; cached startup was changed to require no
helper. The final report must distinguish subsequent passing runs from that failure.

Boundary Discipline drove the separation of download authority from audio handling.
Foundational Thinking drove the fixed pipe protocol and process ownership.
Build the Lever drove the repeatable verifier. Prove It Works
requires real model output and actual denied connections before claiming success.
