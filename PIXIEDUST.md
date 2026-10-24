# Pixiedust cursor effects

Pixiedust uses small translucent sprites beneath text while the native cursor
marks the immediate logical position. Effects run only in Normal-mode document
navigation. Modes, overlays, terminal focus, view changes, edits and scrolling
cancel particles. Returning to Normal seeds a new position.

## Configuration

```toml
[visual]
cursor_effect = "auto" # auto | pixiedust | smear | off
# fps = 60            # existing overall/preset cap also applies

[visual.pixiedust]
density = 0.7         # particles per cursor-height traveled; 0..4
lifetime_ms = 500     # 100..1500
fps = 60             # 10..120; capped by visual.fps/preset
```

Use `:Settings`, `:Set visual.cursor_effect 'smear'`, or
`:Set visual.pixiedust.density 1.0`. Existing `cursor_animation=false`, Minimal
preset and native cursor quality still disable effects. Density zero skips pixel
negotiation and uses smear; use `off` to disable effects completely. Smear settings
and appearance are unchanged. Color comes from `visual.smear.color` or theme Iris.

## Compatibility and detection

`auto` attempts the local Unix Kitty profile, then falls back to smear. Explicit
`pixiedust` uses the same checks and reports unavailable/suspended fallback.
`smear` sends no graphics queries; `off` performs no particle work or negotiation.
Pixels require all of:

1. A Kitty hint without SSH/tmux/screen/WezTerm markers.
2. Measured integral cell pixel dimensions from terminal window size.
3. A live XTVERSION response identifying Kitty 0.20.0 or newer.
4. A successful raw RGBA query, atlas upload acknowledgement, and transparent
   crop-placement acknowledgement using placement identity, `C=1`, and negative z.

Environment alone never enables pixels. Ghostty, WezTerm, Windows Terminal,
Windows/ConPTY, remote/multiplexed sessions and unknown implementations use smear.
Their graphics/input semantics have not been validated for this overlay. This
does not mean they lack graphics features. Missing/nonintegral metrics also
cause fallback.

**Validation:** automated protocol-contract and headless integration coverage.
No live Kitty/Ghostty/WezTerm rendering was available on the Windows host. Visual
compatibility, GPU latency, flicker and actual terminal display latency remain
unverified. Live Kitty validation is required before broadening support.

## Architecture and bounds

The renderer-independent simulation (`src/pixiedust.rs`) adapts Neovide's
distance-based emission, downward drift, curl, varied lifetime and linear fade.
It adds stratified sampling, exact curl integration, a 96-particle cap, at most
32 births/update and a 128 births/second token budget. Jumps over 25 cursor-heights
do not emit; stalls over 250 ms discard particles. Upstream MIT attribution is
retained in `LICENSES/` and `THIRD_PARTY_NOTICES.md`.

The adapter uploads one RGBA atlas (16 alpha levels, three sizes) in bounded
4 KiB base64 chunks. At 10×20 cell size, the payload is 25,600 base64 bytes per
setup/resize/configuration change. Animation sends only small source-crop
placements, never full-screen images. Reusing placement IDs avoids deletion
flicker; distinct negative z-indices avoid undefined overlap ordering. Glyphs
remain above sprites, which are clipped to the active text viewport.

Graphics follow Ratatui's diff/flush, save/restore the cursor, and never modify
its cells. Normal animation is quiet (`q=2`), producing no input replies.
Negotiation isolates matching APC/version replies in the existing reader; it
adds no permanent worker. Unrelated input is replayed, with a bounded wait for
an ambiguous Alt escape prefix during negotiation.

Particle-only frames skip Ratatui construction, scene snapshots and cell diffs;
text/input frames retain Ratatui's normal rendering. Smear keeps its existing
cached scene path. The event-driven clock stops after expiration. A phase-locked graphics
cap prevents typing-triggered frames from flooding output. Three consecutive
frames exceeding twice their budget suspend pixels and restore smear until a
configuration change or resize. This monitors application write/frame time,
not GPU or physical display completion.

Only Filavim's atlas is deleted. Cancellation removes placements; resize,
configuration changes, shutdown and panic restoration free image data. Cleanup
can retry after an output failure. A hard kill cannot run cleanup. Quiet animation
cannot observe later atlas eviction; resize/change settings to upload again.
Terminal backpressure can still block shared output: traffic is bounded rather
than independently queued, which could reorder text and graphics.

`ratatui-image` was evaluated. Its stdio query must precede the event reader and
its cell-owned widgets do not fit this persistent under-text overlay. Filavim
implements the small required Kitty subset with existing base64 support. No
image decoder, GUI, Skia or graphics dependency was added.

## Measurements and manual checks

```text
cargo test --release --lib --features perf pixiedust -- --include-ignored --nocapture --test-threads=1
```

The budget benchmark records 10,000 individual simulation/placement samples and
test-only per-thread allocations after warm-up. The replay runs three 202-event
navigation/typing streams per mode at 120 events/s through TestBackend and a mock
Kitty profile. Graphics bytes are counted, not sent to a terminal. Quantiles use
individual samples; settled effects must not add periodic background polls.

Final Windows release measurements (Rust 1.93.1, isolated headless replay):

| Mode | Input → action p50/p95/p99 (µs) | Input → frame p50/p95/p99 (µs) |
| --- | --- | --- |
| Off | 50.0 / 115.6 / 276.4 | 373.6 / 684.8 / 875.8 |
| Smear | 41.7 / 85.2 / 198.6 | 297.8 / 636.5 / 802.6 |
| Pixiedust | 41.8 / 89.7 / 183.1 | 299.5 / 634.1 / 807.6 |

There are 606 input samples per mode. Maximum sampled input queue depth was one;
settled effects added no periodic background polls. Pixiedust emitted 304,331
protocol bytes across its three streams. The isolated 10,000-sample
simulation/placement benchmark measured p50/p95/p99 **2.3/3.2/3.7 µs**, zero
hot-path allocations, and 788.3 average / 819 maximum bytes/update at default
density (11 peak live particles in this workload, rather than the hard cap).
The warmed command/atlas buffers retained 16/32 KiB respectively.

An earlier run before bypassing Ratatui for particle-only frames measured
Pixiedust input-to-frame p99 1,314.9 µs versus 807.6 µs in the final run. Off and
smear also became faster between runs, so this is inconclusive as a causal speedup.
The deterministic regression instead verifies that isolated particle updates no
longer construct/diff text scenes. No significant latency improvement over smear
is claimed; the goal is bounded additional visual work.

These results include occasional host scheduling outliers (the off replay's
maximum input-to-action was 6.04 ms) and do not establish physical terminal
latency. Native CPU utilization, GPU cost, display latency and visual fidelity
remain unmeasured. The replay covers navigation/typing; it does not establish
tails under every possible syntax/LSP/terminal workload.

In local Kitty, test slow/held WASD, word moves and long jumps over light/dark
text. Check glyph readability. Change modes/open overlays during animation;
scroll, resize, switch splits/tabs and quit while particles exist. Check remnants,
cursor displacement, flicker and altered text. Compare `smear` and `off`. Confirm
unsupported terminals fall back without escape text. Do not infer Ghostty or
WezTerm compatibility from Kitty support.

References: [Neovide](https://github.com/neovide/neovide/blob/4caa65a968ce03624978a45c3605424a56f26d90/src/renderer/cursor_renderer/cursor_vfx.rs),
[Kitty graphics](https://sw.kovidgoyal.net/kitty/graphics-protocol/),
[ratatui-image](https://github.com/ratatui/ratatui-image).
