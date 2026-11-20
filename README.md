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

Early development. What exists today:

| Part | State |
|---|---|
| `tmate2-hid` driver (input reports, encoders, buttons, LCD, backlight, LEDs) | working; input, output and the full LCD map confirmed on HW1.1 / FW1.5 |
| `tmate2-probe` bring-up tool | working |
| `tmate2` connector: mapping engine, commented TOML config, LCD renderer | working (headless) |
| TCI backend with optional TCI-X (`auto`) | working; tested with Thetis |
| CAT backends, HMI emulation, GUI, packaging | planned |

See [docs/hardware-bringup.md](docs/hardware-bringup.md) to help confirm the
hardware map on your own TMATE-2.

## Building

Install Rust from <https://rustup.rs>.

- **Windows:** needs the Visual Studio C++ Build Tools.
- **Linux:** `sudo apt install libudev-dev pkg-config`, then install
  [`packaging/linux/50-tmate2.rules`](packaging/linux/50-tmate2.rules) so you
  can open the device without root.
- **macOS:** works out of the box.

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

The configuration is plain, commented TOML: radio connection, display,
tuning steps and acceleration, the E1 / E2 knob functions, and a tap and a
hold action for every button and knob push.

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
