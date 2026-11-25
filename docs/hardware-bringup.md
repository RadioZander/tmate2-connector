# Hardware bring-up

Confirmed on HW1.1 / FW1.5:

- the input report layout
- all button bits (E2 push is bit 7, E1 push is bit 8)
- encoder directions (E1/E2 give 2 counts per detent)
- the 45-byte write
- the backlight, LEDs and click
- all main and S-meter digit segments

Other findings:

- **Contrast:** lower is darker. 0–128 is visible and 192 or more blanks the
  LCD. The default is 0.
- **Timeout:** when the host stops writing, the TMATE-2 clears its display
  after a short time and shows `--- --- ---`. So a connector that exits, or
  is ended by another program, leaves the device in a clean state. The
  connector writes continuously while it runs.

- **Knob function labels:** the bottom row of the LCD shows what each knob
  currently controls. The `E1` and `E2` markers split the row:
  - E1 (left knob): VOL, RFG, SQL, DRV, NR, NB, AN
  - E2 (right knob): HIGH, LOW, SHIFT, RIT, XIT
- **Lamps left of the frequency:** NR, NB and AN, matching the E1 row. AN is
  the automatic notch.

The full LCD map (176 segments, including icons, modes, S-meter bars and
scale, and knob labels) is in `crates/tmate2-hid/segmap.default.toml`. Every
entry is confirmed, checked with `tmate2-probe demo`. The display differs slightly from the drawing in ELAD's
manual, so the map follows the hardware rather than the drawing.

The steps below let anyone re-check the map on other hardware or firmware.

## 1. Is the device visible?

```sh
cargo run -p tmate2-probe -- list
```

Expect at least one `1721:0614` entry. On Linux, if `dump` fails with a
permission error, install the udev rule from `packaging/linux/`.

## 2. Input report

```sh
cargo run -p tmate2-probe -- dump
```

Check and note:

- [ ] The report length (`len=`), and whether the first byte is constant (a report ID).
- [ ] Turning the **main knob clockwise** gives `Turned { encoder: Main, delta: +n }`.
- [ ] Turning **E1** and **E2** moves the right counter, and clockwise is positive.
- [ ] Each of **F1–F6** gives `Pressed(F1)` … `Pressed(F6)`.
- [ ] Pushing the **main knob** gives `MainPush`.
- [ ] Pushing **E1** gives `E1Push` and pushing **E2** gives `E2Push`. Public
      notes disagree on which of the two is bit 7, so this one matters.

If anything is wrong, fix the constants in `crates/tmate2-hid/src/input.rs`:
`LAYOUT`, `Button::bit`, and `InputTracker::new`'s `invert`.

## 3. Outputs

```sh
cargo run -p tmate2-probe -- test
```

- [ ] All segments light, and the backlight cycles white → red → green → blue.
- [ ] The USB and LOCK LEDs light, and a click is heard.
- [ ] Note the `write length accepted` line (45 or 44).
- [ ] `freq 14074000` shows **14.074.000** (if digits are scrambled on other hardware, step 4 fixes the map).

## 4. Segment walk

```sh
cargo run -p tmate2-probe -- walk
```

The whole display lights and one bit blinks. Type what is blinking, then Enter:

| What lights | Type |
|---|---|
| a segment of a main-display digit | `main.<digit>.<seg>`: digit 1 is the rightmost; seg is `a`–`g` (a top, b top-right, c bottom-right, d bottom, e bottom-left, f top-left, g middle) |
| a segment of an S-meter digit | `smeter.<digit>.<seg>` |
| a decimal point | `dot.mhz` / `dot.khz` (or `dot.<n>`) |
| an S-meter bar | `bar.<n>`, counting from the left |
| an icon or word | its name in `segmap.default.toml`, e.g. `mode.usb`, `mode.cw`, `att`, `rx`, `tx`, `vfo.a`, `left.nr`, `e1.vol` |
| nothing | just press Enter |

Type `b` to go back and `q` to stop. Progress is saved after every entry to
`segmap.walked.toml`, and running `walk` again resumes it. Use
`--start 12.0` to jump ahead.

Then check it:

```sh
cargo run -p tmate2-probe -- --segmap segmap.walked.toml freq 14074000
cargo run -p tmate2-probe -- --segmap segmap.walked.toml text HELLO
```

## 5. Capturing the original HMI's serial output (for HMI emulation)

The HMI emulation is built from recordings of what ELAD's HMI software sends.
It is not built from the HMI program itself.

1. Install [com0com](https://sourceforge.net/projects/com0com/) and create a
   pair, e.g. `COM20 <-> COM21`.
2. Point the HMI at `COM20`. Pick the **Perseus** profile first, then **PowerSDR**.
3. Open `COM21` with a logging terminal at the HMI's baud rate. Any terminal
   that logs raw bytes with timestamps works.
4. For each profile, record a short session:
   - tune slowly and quickly both ways on each knob
   - press each button
   - change mode and band in the HMI
5. Save the logs under `captures/` (git-ignored). Before anything goes into
   `crates/*/tests/fixtures/`, check that nothing personal (such as callsign
   or locator) is in it.
