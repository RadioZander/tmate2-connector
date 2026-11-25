# tmate2-connector: notes for Claude Code

This is an open, cross-platform connector for the ELAD / Woodbox TMATE-2
controller. It is written in Rust, MIT-licensed and public on GitHub.

## Project goals

- **Platforms:** Windows and Linux, including the Raspberry Pi (aarch64). macOS
  builds in CI only: the maintainer has no Mac, so don't spend effort on
  Mac-specific work.
- **Run modes:** the egui window, or `--headless`.
- **Radio control:** standard TCI comes first and must work fully on its own.
  TCI-X (by Mike G0KAD) is optional, set by `tcix = off|auto|on` (default
  `auto`). Use only standard TCI commands. Don't use Thetis-only `*_ex`
  commands or CAT passthrough (`run_cat_ex`) without asking: radio-specific
  extensions are a possible future feature, not something to add quietly.
- **Targets:** Thetis on Windows (done, tested with an ANAN-G2), then
  piHPSDR (Linux / the G2's internal Pi), and later KADRAD (G0KAD) via TCI-X.
- **Roadmap:**
  - CAT backends: PowerSDR / Thetis ZZ, Kenwood, Elecraft, Icom CI-V
  - Emulation of the ELAD HMI serial output, built only from captures
  - Packaging
  - Radio profiles for server quirks (see "piHPSDR TCI behaviour": CWL)
- **Scope:** keep it focused. Advanced settings belong in the radio
  software, not here.

## Licence rules

- **microenh/Tmate2_C has no licence.** Use only hardware facts from it
  (USB IDs, which byte holds what). Never copy its code, tables or text.
- **New dependencies:** they must pass `cargo deny check licenses`
  (`deny.toml`), which fails the build on any GPL dependency.
- **Fonts:** egui's bundled fonts (OFL, Ubuntu Font Licence) have a scoped
  exception in `deny.toml`. Their licence texts are in `licenses/fonts/` and
  listed in `THIRD-PARTY-NOTICES.md`.
- **Personal data:** keep it out of commits and test fixtures. `local/`,
  `config.toml` and `captures/` are gitignored.

## Layout

| Crate | Contents |
|---|---|
| `tmate2-hid` | HID reports, encoder tracking, LCD segment map (`segmap.default.toml`), 7-segment font |
| `tmate2-core` | config (`default_config.toml` holds the commented defaults and is the single source of defaults), radio state, `Command`, mapping `engine`, LCD `render` |
| `tmate2-backends` | TCI parser/encoder (`tci.rs`, tests against `tests/fixtures/thetis-init.tci`), WebSocket client with reconnect and TCI-X negotiation and login, `pacing.rs` (dial / knob pacing: `[radio] pacing = off|auto|on`; `auto` paces a target only once its echoes take over 80 ms; the tested numbers are built in (`PacingTuning::default`), not in `default_config.toml`; a user on a slow link can add a `[radio.pacing_tuning]` table to override them, and the window never shows it). At `-v` the client logs lines both ways (`TCI >` / `TCI <`, meter reports left out) and the pacer logs each matched echo's delay and average, and when `auto` switches a target to paced or back |
| `tmate2-app` | the `tmate2` binary: device thread, GUI (`gui/`, saves with `toml_edit` so comments are kept), headless mode |
| `tmate2-probe` | hardware diagnostics: `list`, `dump`, `walk`, `demo`, `rgb`, `contrast`, `text`, ... |

To add a button action:

1. Add it to `Action` and `SIMPLE_ACTIONS` in `config.rs`.
2. Handle it in `Engine::action`.
3. If it talks to the radio, add a `Command` variant in `command.rs` (and
   `RadioState::apply`), then TCI `encode` / `apply` in `tci.rs`.
4. Document it in the `[buttons]` comment in `default_config.toml`.
5. Add tests.

The GUI picker lists everything in `SIMPLE_ACTIONS` automatically.

## Hardware facts (confirmed on HW1.1 / FW1.5)

- **USB:** HID with VID `0x1721` and PID `0x0614`.
- **Input reports:** 64 bytes, of which only the first 9 matter.
  - Bytes 1–6: the main, E2 and E1 encoders, as u16 little-endian.
  - Bytes 7–8: the button word, active-low. Bits 0–5 are F1–F6, bit 6 is the
    main push, bit 7 the E2 push and bit 8 the E1 push.
  - The main knob's counter falls when turned clockwise. E1 and E2 give 2
    counts per detent.
- **Output report:** 45 bytes.
  - Bytes 1–32: LCD segments.
  - Byte 33: USB LED (bit 0), LOCK LED (bit 1), click (bit 2).
  - Bytes 34–36: RGB backlight.
  - Byte 37: contrast. Lower is darker; segments vanish from about 192.
- **Bottom row of the LCD:** split by the E1 / E2 markers. E1 has VOL, RFG,
  SQL, DRV, NR, NB and AN; E2 has HIGH, LOW, SHIFT, RIT and XIT.

More detail is in `docs/hardware-bringup.md`.

## Display conventions

The maintainer wants the LCD to look like a real radio:

- A frequency shown to the hertz lights **Hz**, not k.
- The S-scale labels S 1 3 5 7 9 +20 +40 +60 are always lit on receive, and
  "10" is never lit. On transmit the S scale switches off and the bargraph
  shows power or SWR, with 1.8:1 at the S9 bar.
- The small readout shows -dBm (or S units with no "S" prefix, which would
  read as a 5). On transmit it shows watts, or SWR as `1_2` for 1.2:1.
- Volume is 0–100 like Thetis (TCI sends dB: -60 to 0 dB maps linearly to
  0–100). Thetis truncates when converting back, so aim 0.03 dB above each
  step.

## Radio state

- **Shown:** the LCD and GUI show only what the radio reports. A command
  changes nothing shown until the radio's report arrives.
- **Knob hold (G0KAD):** a knob counts from the value it asked for until
  the radio reports that value, or for 1.5 s (the dial 0.4 s), so late
  echoes can't pull it back. The hold also ends when the radio repeats
  its last report after answering everything sent: the value is at the
  radio's limit, and the knob stops there too.
- **Read, modify, write:** at the start of each turn or press, a value not
  heard in the last 250 ms is read first, in standard TCI's read form
  (`name:trx;`). A server that doesn't answer within 500 ms isn't asked
  again for that command on this connection.
- **Unanswered:** a command with no report gets one read after 0.5 s. If
  that goes unanswered too, the value is taken as unknown (lamp off).
- **Poll:** what the LCD shows is read once a second (frequency, mode,
  NR, NB, AN, LOCK, TX, TUNE), skipping what a knob is setting right now.
- The same rules apply to every server. Don't branch on which program it
  is.

## Thetis TCI behaviour

- **Server:** 127.0.0.1:50001. It reports `protocol:ExpertSDR3,2.0` and
  `device:SunSDR2PRO`.
- **Filter presets:** Thetis overwrites them when the filter is adjusted, so
  the connector keeps its own per-mode defaults in `[filters]`.
- **Echo timing:** on the same PC, `vfo` echoes take up to ~85 ms on a fast
  spin and filter changes ~370 ms, so `pacing = "auto"` does switch on for
  Thetis. It felt as good as `on` (tested 2026-11-21).
- **tune_drive:** works only while Thetis's tune power is a slider; with
  tune power forced to a fixed value Thetis ignores it (no echo).
- **Ignored:** `set_in_focus` (sent, window not raised), so the connector no
  longer has a focus action. `mon_volume` is
  echoed, rounded to 0.1 dB.

## piHPSDR TCI behaviour

Seen with DL1YCF piHPSDR (github.com/dl1ycf/pihpsdr, November 2026) on an
ANAN-G2. The original g0orx/pihpsdr has no TCI, only rigctl on port 19090.

- **Server:** Menu > CAT/TCI, tick TCI. Port 40001, off by default. Don't
  confuse it with the "Server" menu, which is piHPSDR's own client/server.
  It reports `protocol:ExpertSDR3,2.0` and `device:SunSDR2QRP`. TCI has no
  message naming the software, so handle what the server sends and don't
  branch on which program it is.
- **Modes:** `modulations_list` has `CW` and `FMN`. Both CWL and CWU are
  reported as `modulation:…,CW` and FM as `FM`. `Mode::Cw` (sideband unknown)
  shows plain CW. Setting accepts `CW`/`CWL`/`CWU`/`FM`/`FMN` but not `NFM`.
- **CW sideband:** `CW` sets CWU. `CWL` / `CWU` are accepted although not
  listed (seen in the source, not tried on the wire). The connector sends
  the listed name, so CWL can't be chosen from the TMATE-2 (roadmap: radio
  profiles).
- **Volume:** `volume` / `rx_volume` are whole dB from -40 to 0. Use
  `volume_scale = "db"`; the "percent" scale stalls because the rounded echo
  undoes each 0.6 dB step. A value below -40 is answered with -40, and the
  knob stops there. `rx_volume` is the receiver's AF gain.
- **Meters:** `rx_channel_sensors` and `rx_sensors` arrive once a second
  whatever interval is asked for. `tx_sensors:0,mic,rms,peak,swr` is sent only
  while transmitting with forward power.
- **Filters:** CW filter edges are relative to the sidetone pitch (piHPSDR
  reports e.g. `-250,250` in both CWL and CWU), unlike Thetis. Plain CW
  therefore has its own `[filters]` entry, `CW = [-250, 250]`, which is
  centred on the pitch in both sidebands. Tested on the G2, including the
  width, high and low knobs in CWL and CWU. FM ignores filter commands but
  still echoes the edges asked for.
- **Reports:** only frequency, mode, split and MOX changes are announced.
  Volume, NR / NB / ANF, squelch, drive, filter and the rest changed in
  piHPSDR's own window are not, and NR / NB / ANF aren't echoed. Reads are
  answered at once, so read-before-write and the poll cover this.
- **Read bugs (reported upstream):** the `rx_filter_band` read gives the
  mode's preset, not the edges in use, so the connector never reads the
  filter (dl1ycf/pihpsdr#157). `rx_nb_enable` reads back SNB, not NB, so
  the NB lamp follows SNB and turning NB off can take a double press
  (dl1ycf/pihpsdr#158).
- **Echo timing:** on the same machine, 0–6 ms, so `pacing = "auto"`
  never engages. Not measured over a network.
- **Unlabelled functions:** `cw_keyer_speed` works. `tune_drive` works but
  applies at the next tune, and setting it turns piHPSDR's "tune uses
  drive" off. `mon_volume` and `mon_enable` are stubs (always -60 /
  false).
- **Quirks:** `audio_stream_channels` and `audio_stream_samples` have no `;`,
  harmless because each message is its own WebSocket frame. No TCI-X reply.
- **Fixture:** `crates/tmate2-backends/tests/fixtures/pihpsdr-init.tci`.

## Building and checking

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p tmate2-app --no-default-features -- -D warnings   # headless build
cargo test --workspace
cargo deny check licenses
```

- **Rust:** the minimum is 1.95 (eframe 0.37 needs it). CI uses the latest
  stable.
- **GUI renderer:** OpenGL (glow), not wgpu. It is lighter and better on
  the Pi.
- **Linux:** needs `libudev-dev` and `pkg-config`, plus the X11 / Wayland /
  GL development packages for the window. Install
  `packaging/linux/50-tmate2.rules` so the TMATE-2 opens without root.
- **CI:** `.github/workflows/ci.yml` runs on pushes to `main` and on pull
  requests, not on other branches.
- **Releases:** `.github/workflows/release.yml` runs on a `v*` tag and
  builds Windows x64, Linux x64, Linux arm64 (Pi, built on Ubuntu 22.04 for
  glibc compatibility) and macOS arm64.

## Working with the maintainer

- Commit and push only when asked.
- When reporting hardware tests, be exact about what was and wasn't verified.
