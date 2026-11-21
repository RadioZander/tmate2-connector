# tmate2-connector

An open, cross-platform connector for the **ELAD / Woodbox TMATE-2** SDR controller.
It runs on Windows, Linux (including Raspberry Pi / aarch64) and macOS.

The TMATE-2 is a USB HID device: no driver is needed. This project talks to it
directly, so ELAD's Windows-only HMI software isn't required. It aims to give
far more control over what each knob and button does, and to connect to
radios and SDR software over:

- **TCI**: Thetis, piHPSDR, ExpertSDR and others. Optional **TCI-X**
  extensions are negotiated automatically when the server supports them.
- **CAT**: PowerSDR/Thetis (ZZ), Kenwood, Elecraft and Icom CI-V, over
  serial, virtual COM or TCP.
- **HMI emulation**: a virtual serial port that behaves like the original
  HMI's Perseus / PowerSDR output, for software that expects it.

## Status

| Part | State |
|---|---|
| `tmate2-hid` driver: input reports, encoders, buttons, LCD, backlight, LEDs | Working. The full LCD map is confirmed on HW1.1 / FW1.5, and the inputs on FW1.3. |
| `tmate2` connector: a window or `--headless`, mapping engine, LCD renderer, commented TOML config that the window edits | Working |
| TCI backend | Working. Tested with Thetis on Windows and piHPSDR on Linux, both with an ANAN-G2. |
| Optional TCI-X extensions | Working. Tried on an Icom IC-7100 through kadrad-hub. Login and control have been tried only against a test server so far. |
| `tmate2-probe` hardware tool | Working |
| CAT backends, HMI emulation, installers | Planned |

See [docs/hardware-bringup.md](docs/hardware-bringup.md) to help confirm the
hardware map on your own TMATE-2.

## What it does

### Knobs

- **The main knob** tunes the active VFO.
  - The tuning step can be changed from a button, and the digit it changes is underlined.
  - The knob speeds up when you spin it fast.
  - A `fast` button multiplies the step.
- **E1** steps through VOL, RFG, SQL, DRV, NR, NB and AN when you push it.
- **E2** steps through HIGH, LOW, WIDTH, SHIFT, RIT and XIT.
- **Holding a knob's push** switches its current function on or off, or puts the mode's default filter back.
- **Either knob can also have four functions with no label on the LCD:** the receiver's own volume, monitor volume, CW speed and tune drive. While a knob rests on one, the small readout shows its name: AF, -non, SPd or tdr.

### Buttons

Each of F1–F6 and each knob push has a **tap** action and a **hold**
action. Holding a stepping action repeats it. The actions are:

- **Bands and VFOs:** band up / down, VFO A / B, A→B, B→A and swap.
- **Split, RIT and XIT:** split, RIT and XIT on or off, and clear one or both.
- **Lock.**
- **Mode:** step through the modes, or go straight to one.
- **Filter:** wider, narrower, or back to the mode's default.
- **AGC:** step through the AGC modes, or go straight to one.
- **Noise and filters:** NR, NB, auto notch, manual notch, APF, binaural and squelch.
- **Audio:** the master mute, RX1 and RX2 mutes, and the transmit monitor.
- **Receivers:** RX2 on or off, and the sub-receiver. The knobs can also be switched to control RX2.
- **Transmit:** MOX, push-to-talk (only while held), tune, and tune while held.
- **Other:** start / stop the radio, and step through the backlight colours.
- **Any TCI command:** a button can send raw TCI text.

The configuration file lists every action with a comment.

### Display

The LCD works like a radio's front panel:

- **Main display:** the frequency to the hertz.
- **Mode and status:** the mode, VFO A / B, RX / TX, NR and NB.
- **Knobs:** the labels of each knob's function.
- **S meter:** the bargraph with its scale, and the signal in -dBm or S units.
- **On transmit:** the S scale switches off. The bargraph shows power or SWR (1.8:1 at S9), and the readout shows watts or SWR (`1_2` means 1.2:1).
- **Backlight:** separate colours for receive, transmit and no connection.

The window shows a live copy of the LCD, and you can change all the settings there.

### Radio connection

- **TCI** (Expert Electronics' standard) works by itself. Only standard TCI commands are used.
- **Pacing** (`pacing = "auto"`) is for a radio that can't keep up with the knob, such as one reached over a network. Once the radio's replies start to fall behind, the connector waits for each one before sending the newest value. While the radio keeps up, nothing is held back.
- **TCI-X** (`tcix = "auto"`), by Mike G0KAD, is used when the server offers it.
  - The connector sends only the commands the server says it accepts, and skips knob functions the radio can't do.
  - It uses the radio's own AF and RF gain, filter presets, band stack and antenna tuner.
  - It logs in if the server asks, and takes part in the server's one-operator control.
  - The ATT, PRE, ANT, LP and ERR lamps follow the radio.

## Download

Ready-built versions are on the
[Releases page](https://github.com/RadioZander/tmate2-connector/releases):

| File | For |
|---|---|
| `…-windows-x64.zip` | Windows 10 / 11 |
| `…-linux-x64.tar.gz` | 64-bit Linux PCs |
| `…-linux-arm64-raspberry-pi.tar.gz` | Raspberry Pi (64-bit Raspberry Pi OS), e.g. the ANAN-G2's internal Pi |
| `…-macos-arm64.tar.gz` | Apple-silicon Macs (unsigned, untested) |

Each archive contains `tmate2` (the connector), `tmate2-probe` (hardware
diagnostics), an example configuration and, on Linux, the udev rule.
`SHA256SUMS.txt` lets you verify the downloads.

Run `tmate2` to open the window, or `tmate2 --headless` on a machine with no
screen. The first run writes a commented configuration file and logs where.

## Roadmap

- CAT backends: PowerSDR / Thetis ZZ, Kenwood, Elecraft, Icom CI-V
- piHPSDR on the ANAN-G2's internal Raspberry Pi (headless service)
- Emulation of the original ELAD HMI serial output (Perseus / PowerSDR)
- Installers / packages (Windows installer, .deb for the Pi)

## Building

Install Rust from <https://rustup.rs>.

- **Windows:** needs the Visual Studio C++ Build Tools.
- **Linux:** `sudo apt install libudev-dev pkg-config`. For the window you also
  need the X11, Wayland and OpenGL development packages: on Debian / Ubuntu,
  `libxkbcommon-dev libgl1-mesa-dev libwayland-dev libx11-dev libxcursor-dev
  libxrandr-dev libxi-dev`. A headless-only build
  (`cargo build --release -p tmate2-app --no-default-features`) doesn't need
  them. Install
  [`packaging/linux/50-tmate2.rules`](packaging/linux/50-tmate2.rules) so you
  can open the device without root.
- **macOS:** builds with no extra packages, but it hasn't been tried on a TMATE-2.

```sh
cargo build --release
cargo run -p tmate2-probe -- list     # is the TMATE-2 visible?
cargo run -p tmate2-probe -- dump     # watch knobs / buttons
cargo run -p tmate2-probe -- test     # LCD, backlight and click test
cargo run -p tmate2-probe -- freq 14074000
cargo run -p tmate2-probe -- walk     # map every LCD segment interactively

cargo run -p tmate2-app -- -v         # run the connector (writes a commented
                                      # config on first run and says where)
```

The configuration is plain, commented TOML. It covers:

- the radio connection
- the display
- tuning steps and acceleration
- the E1 / E2 knob functions
- the default filter for each mode
- a tap and a hold action for every button and knob push

Saving from the window keeps your own comments.

## Licence

MIT; see [LICENSE](LICENSE).

## Credits and references

- Hardware notes in [microenh/Tmate2_C](https://github.com/microenh/Tmate2_C)
  helped locate the USB IDs and report fields. That repository has no licence,
  so **none of its code, tables or text is used here**. This project
  re-implements everything independently, and confirms the hardware facts by
  probing.
- **TCI** (Transceiver Control Interface) by Expert Electronics:
  <https://github.com/ExpertSDR3/TCI> (MIT).
- **TCI-X**, an extended superset of TCI by Mike G0KAD:
  <https://github.com/g0kad/TCI-X> (MIT).

## Disclaimer

This is an independent project. It is not affiliated with or endorsed by ELAD,
Woodbox Radio, Expert Electronics, Apache Labs, or the Thetis, PowerSDR or
piHPSDR projects. Product names are used only to identify compatible hardware
and software. Their trademarks belong to their owners.
