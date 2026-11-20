# Third-party notices

tmate2-connector is MIT-licensed (see [LICENSE](LICENSE)). The program is
built from Rust crates that are almost all MIT and/or Apache-2.0 licensed;
`cargo deny check licenses` (configured in [deny.toml](deny.toml)) verifies
every dependency on each build.

## Fonts bundled in the desktop window

The window uses the default fonts of the [egui](https://github.com/emilk/egui)
toolkit (crate `epaint_default_fonts`), which are compiled into `tmate2`:

| Font | Licence | Text |
|---|---|---|
| Ubuntu Light | Ubuntu Font Licence 1.0 | [licenses/fonts/UFL.txt](licenses/fonts/UFL.txt) |
| Noto Emoji | SIL Open Font Licence 1.1 | [licenses/fonts/OFL.txt](licenses/fonts/OFL.txt) |
| Hack | MIT / Bitstream Vera | [licenses/fonts/Hack-Regular.txt](licenses/fonts/Hack-Regular.txt) |
| emoji-icon-font | MIT | [licenses/fonts/emoji-icon-font-mit-license.txt](licenses/fonts/emoji-icon-font-mit-license.txt) |

A headless-only build (`cargo build -p tmate2-app --no-default-features`)
contains no fonts.

## Protocols

- TCI, by Expert Electronics: <https://github.com/ExpertSDR3/TCI> (MIT).
- TCI-X, by Mike G0KAD: <https://github.com/g0kad/TCI-X> (MIT).

These specifications were implemented independently; no specification text
is reproduced beyond command names.
