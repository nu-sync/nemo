---
name: research-handy
description: Find Handy features in its official docs and trace their implementation in cjpais/Handy on GitHub. Use for Handy feature research, implementation comparisons, or adapting Handy ideas to Nemo.
---

# Research Handy

Nemo is heavily inspired by [Handy](https://github.com/cjpais/Handy). Use its
[official documentation](https://handy.computer/docs) to understand the experience
and its source to explain the mechanism. This skill produces research; implement
Nemo changes only when the user's request includes them.

## Find the feature

Use the [feature map](references/feature-map.md) to choose a docs page and initial
code locations. Fetch only the relevant page and files. The map is a navigation
aid: verify paths and behavior against the revision being researched.

Prefer connected GitHub tools for repository browsing and code search; use web
tools for the docs. Search with a visible label, then the setting key or command
name it reveals. If GitHub search is unavailable, browse directories or use an
isolated shallow clone outside Nemo; do not vendor Handy or install its dependencies.

For local source searches, set `handy_checkout` to that checkout's absolute path:

```sh
git -C "$handy_checkout" rev-parse HEAD
rg -n -i 'clipboard_handling|ClipboardHandling' "$handy_checkout/src" "$handy_checkout/src-tauri/src"
rg --files "$handy_checkout/src" "$handy_checkout/src-tauri/src" | rg -i 'clipboard|paste'
```

Replace the example terms with the feature's label, translation key, or symbol.

## Trace the implementation

1. Read the docs for the trigger, options, defaults, and platform restrictions.
2. Resolve a source commit SHA (or the requested release/tag) and use it consistently.
   Live docs may describe a different version; call out disagreements.
3. Follow the settings component through `src/hooks/useSettings.ts` and
   `src/stores/settingsStore.ts` to the Rust command, persisted setting/default in
   `src-tauri/src/settings.rs`, and runtime consumer. For non-settings features,
   start at the CLI, shortcut, or tray handler and follow its callees.
4. Read the actual branches that perform the work, including cancellation,
   cleanup, platform gates, and relevant tests. Separate documented behavior,
   source-confirmed behavior, and inference. A matching filename is only a lead.
5. When adaptation is requested, inspect Nemo's corresponding modules and explain
   the smallest useful change. Account for Handy's React/Tauri architecture versus
   Nemo's native menu event loop and managed inference subprocess. Preserve license
   notices and attribution if reusing code or assets.

## Report the useful result

Give the feature's user-visible behavior, docs link, and a short call chain with
the responsible files/symbols. Cite source with immutable links such as
`https://github.com/cjpais/Handy/blob/<sha>/<path>#L10-L25`, using inspected line
numbers. State the commit/tag and platform, relevant tests or evidence gaps, and
any Nemo adaptation recommendation. Reading source does not establish runtime
verification. Keep the answer focused on the requested feature.
