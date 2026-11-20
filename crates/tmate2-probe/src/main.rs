//! `tmate2-probe`: bring-up and diagnostics for the TMATE-2.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tmate2_hid::hidapi::HidApi;
use tmate2_hid::output::{SEGMENT_END, SEGMENT_FIRST};
use tmate2_hid::{list_devices, Button, InputTracker, RawInput, SegAddr, SegmentMap, Tmate2};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Segment map overriding entries of the built-in one (e.g. from `walk`).
    #[arg(long, global = true)]
    segmap: Option<PathBuf>,
    /// For display commands: keep re-sending for this many seconds before exiting.
    #[arg(long, global = true, default_value_t = 3.0)]
    hold: f32,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List TMATE-2 HID interfaces (or every HID device with --all).
    List {
        #[arg(long)]
        all: bool,
    },
    /// Print input reports as they change: raw bytes, decoded values, events.
    Dump,
    /// Light each LCD bit in turn and record what it is into a segment map file.
    Walk {
        /// Where to save the map (re-read on start, so a walk can be resumed).
        #[arg(long, default_value = "segmap.walked.toml")]
        out: PathBuf,
        /// Start position as byte.bit.
        #[arg(long, default_value = "1.0")]
        start: String,
        /// Also visit positions already in the built-in map (digits, dots).
        #[arg(long)]
        all: bool,
        /// Visit only these positions (comma-separated byte.bit list).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        /// Light only the bit being asked about, instead of lighting the whole
        /// display and blinking that bit.
        #[arg(long)]
        no_context: bool,
    },
    /// Display test: all segments, backlight colours, click.
    Test,
    /// Light all segments and step through contrast values; each step has its
    /// own backlight colour so you can say which ones showed segments.
    Sweep {
        /// Contrast values to try.
        #[arg(long, value_delimiter = ',', default_value = "0,64,128,192,255")]
        values: Vec<u8>,
        /// Seconds per step.
        #[arg(long, default_value_t = 3.0)]
        secs: f32,
    },
    /// Show a realistic receive screen using the segment map.
    Demo,
    /// Clear all segments.
    Clear,
    /// Set the backlight colour (0-255 each).
    Rgb { r: u8, g: u8, b: u8 },
    /// Set LCD contrast (0-255).
    Contrast { value: u8 },
    /// Show text on the main display, and optionally on the S-meter digits.
    Text {
        text: String,
        #[arg(long)]
        smeter: Option<String>,
        /// Also light these named segments (comma-separated).
        #[arg(long, value_delimiter = ',')]
        on: Vec<String>,
    },
    /// Show a frequency in Hz on the main display.
    Freq { hz: u64 },
    /// Light one named segment, or a raw byte.bit position.
    Seg { name: String },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let api = HidApi::new().context("initialising hidapi")?;

    let mut map = SegmentMap::builtin();
    if let Some(p) = &cli.segmap {
        let text = std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
        map.merge(&SegmentMap::from_toml(&text)?);
    }

    match cli.cmd {
        Cmd::List { all } => list(&api, all),
        Cmd::Dump => dump(&mut open(&api)?),
        Cmd::Walk {
            out,
            start,
            all,
            only,
            no_context,
        } => {
            let only: Vec<SegAddr> = only
                .iter()
                .map(|s| s.parse().map_err(anyhow::Error::msg))
                .collect::<Result<_>>()?;
            let skip = if all || !only.is_empty() {
                SegmentMap::default()
            } else {
                map.clone()
            };
            let context = !no_context;
            walk(&mut open(&api)?, &out, &start, &skip, &only, context)
        }
        Cmd::Test => test(&mut open(&api)?, &map),
        Cmd::Sweep { values, secs } => sweep(&mut open(&api)?, &values, secs),
        Cmd::Demo => {
            let mut dev = open(&api)?;
            demo(&mut dev, &map);
            hold(&mut dev, cli.hold)
        }
        cmd => {
            let mut dev = open(&api)?;
            let o = dev.output_mut();
            match cmd {
                Cmd::Clear => o.clear_segments(),
                Cmd::Rgb { r, g, b } => o.set_rgb(r, g, b),
                Cmd::Contrast { value } => o.set_contrast(value),
                Cmd::Text { text, smeter, on } => {
                    o.clear_segments();
                    map.draw_text(o, "main", &text);
                    if let Some(s) = smeter {
                        map.draw_text(o, "smeter", &s);
                    }
                    for name in &on {
                        if !map.set(o, name, true) {
                            anyhow::bail!("unknown segment {name}");
                        }
                    }
                }
                Cmd::Freq { hz } => {
                    o.clear_segments();
                    map.draw_frequency(o, hz);
                }
                Cmd::Seg { name } => {
                    o.clear_segments();
                    let a = map
                        .get(&name)
                        .map(Ok)
                        .unwrap_or_else(|| name.parse::<SegAddr>().map_err(anyhow::Error::msg))?;
                    o.set_segment(a, true);
                    println!("lit {name} at {a}");
                }
                _ => unreachable!(),
            }
            hold(&mut dev, cli.hold)
        }
    }
}

fn open(api: &HidApi) -> Result<Tmate2> {
    let dev = Tmate2::open(api)
        .context("opening TMATE-2 (is it plugged in? on Linux, is the udev rule installed?)")?;
    Ok(dev)
}

/// Send the output report repeatedly for `secs` (the device may need refreshing).
fn hold(dev: &mut Tmate2, secs: f32) -> Result<()> {
    let end = Instant::now() + Duration::from_secs_f32(secs.max(0.0));
    loop {
        dev.flush().context("writing output report")?;
        if Instant::now() >= end {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("output report accepted with write length {:?}", dev.write_len());
    Ok(())
}

fn list(api: &HidApi, all: bool) -> Result<()> {
    let devs = list_devices(api, all);
    if devs.is_empty() {
        println!("no matching HID devices found");
    }
    for d in devs {
        println!(
            "{:04x}:{:04x}  if={} usage_page=0x{:04x} usage=0x{:04x}  {} / {}  serial={}\n    {}",
            d.vendor_id,
            d.product_id,
            d.interface,
            d.usage_page,
            d.usage,
            d.manufacturer.as_deref().unwrap_or("?"),
            d.product.as_deref().unwrap_or("?"),
            d.serial.as_deref().unwrap_or("-"),
            d.path
        );
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn dump(dev: &mut Tmate2) -> Result<()> {
    // Some devices only report after the host has written once.
    dev.flush().context("initial write")?;
    println!("write length accepted: {:?}", dev.write_len());
    println!("turn the knobs and press the buttons; Ctrl-C to stop\n");
    let mut tracker = InputTracker::new();
    let mut last: Vec<u8> = Vec::new();
    let mut events = Vec::new();
    let mut last_refresh = Instant::now();
    loop {
        if last_refresh.elapsed() > Duration::from_millis(500) {
            dev.flush()?;
            last_refresh = Instant::now();
        }
        let (n, buf) = dev.read_raw(100)?;
        // Only the first 9 bytes are meaningful; byte 0 toggles on its own,
        // so changes are detected on bytes 1.. only.
        let sig = RawInput::significant(&buf[..n]);
        if n == 0 || sig.get(1..) == Some(&last[..]) {
            continue;
        }
        last = sig.get(1..).unwrap_or_default().to_vec();
        match RawInput::parse(&buf[..n]) {
            Ok(r) => {
                let pressed: Vec<String> = Button::ALL
                    .iter()
                    .filter(|b| r.is_pressed(**b))
                    .map(|b| format!("{b:?}"))
                    .collect();
                println!(
                    "len={n:2} [{}]  main={:5} e1={:5} e2={:5} buttons=0x{:04x} pressed={pressed:?}",
                    hex(sig),
                    r.main,
                    r.e1,
                    r.e2,
                    r.buttons_raw
                );
                events.clear();
                tracker.feed(r, &mut events);
                for e in &events {
                    println!("    -> {e:?}");
                }
            }
            Err(e) => println!("len={n:2} [{}]  (not decoded: {e})", hex(sig)),
        }
    }
}

fn walk(
    dev: &mut Tmate2,
    out: &PathBuf,
    start: &str,
    skip: &SegmentMap,
    only: &[SegAddr],
    context: bool,
) -> Result<()> {
    let mut found = match std::fs::read_to_string(out) {
        Ok(t) => SegmentMap::from_toml(&t)?,
        Err(_) => SegmentMap::default(),
    };
    let start: SegAddr = start.parse().map_err(anyhow::Error::msg)?;
    let positions: Vec<SegAddr> = (SEGMENT_FIRST..SEGMENT_END)
        .flat_map(|byte| {
            (0..8).map(move |bit| SegAddr {
                byte: byte as u8,
                bit,
            })
        })
        .filter(|p| !skip.iter().any(|(_, a)| a == *p))
        .filter(|p| only.is_empty() || only.contains(p))
        .collect();
    println!("{} positions to check", positions.len());
    let mut i = positions
        .iter()
        .position(|p| *p >= start)
        .unwrap_or(positions.len());

    println!(
        "Segment walk: the whole display is lit and one bit BLINKS. Type what is\n\
         blinking and press Enter. Names: main.<1-9>.<a-g> (1 = rightmost digit),\n\
         smeter.<1-3>.<a-g>, or short names like mode.usb, att.1, e1.label.vol.\n\
         Enter alone = nothing blinks / keep the old name.  b = back.  q = save and quit.\n\
         Saved after every entry to {}\n",
        out.display()
    );

    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            if tx.send(line.unwrap_or_default()).is_err() {
                break;
            }
        }
    });

    'outer: while i < positions.len() {
        let p = positions[i];
        let o = dev.output_mut();
        o.clear_segments();
        if context {
            o.all_segments();
        }
        o.set_segment(p, true);
        let mut blink_on = true;
        let mut last_blink = Instant::now();
        let known: Vec<&str> = found.iter().filter(|(_, a)| *a == p).map(|(n, _)| n).collect();
        print!(
            "[{p:>5}] {}> ",
            if known.is_empty() {
                String::new()
            } else {
                format!("(was {known:?}) ")
            }
        );
        std::io::stdout().flush()?;
        let line = loop {
            if context && last_blink.elapsed() >= Duration::from_millis(300) {
                blink_on = !blink_on;
                last_blink = Instant::now();
                dev.output_mut().set_segment(p, blink_on);
            }
            dev.flush()?;
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(l) => break l,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break 'outer,
            }
        };
        match line.trim() {
            "" => i += 1,
            "b" => i = i.saturating_sub(1),
            "q" => break,
            name if name.contains(char::is_whitespace) => println!("  names can't contain spaces"),
            name => {
                let old = found.remove_addr(p);
                if !old.is_empty() {
                    println!("  replaced {old:?}");
                }
                found.insert(name, p);
                std::fs::write(out, header() + &found.to_toml())?;
                i += 1;
            }
        }
    }
    std::fs::write(out, header() + &found.to_toml())?;
    println!("\nsaved {} segments to {}", found.iter().count(), out.display());
    Ok(())
}

fn header() -> String {
    "# TMATE-2 segment map recorded with `tmate2-probe walk`.\n\
     # \"name\" = \"byte.bit\" in the output report.\n\n"
        .to_string()
}

fn test(dev: &mut Tmate2, map: &SegmentMap) -> Result<()> {
    let step = |dev: &mut Tmate2, label: &str, secs: f32| -> Result<()> {
        println!("{label}");
        hold(dev, secs)
    };
    dev.output_mut().all_segments();
    step(dev, "all segments on, white", 4.0)?;
    for (name, (r, g, b)) in [
        ("red", (255, 0, 0)),
        ("green", (0, 255, 0)),
        ("blue", (0, 0, 255)),
    ] {
        dev.output_mut().set_rgb(r, g, b);
        step(dev, name, 2.0)?;
    }
    let o = dev.output_mut();
    o.set_rgb(255, 255, 255);
    o.clear_segments();
    o.set_usb_led(true);
    o.set_lock_led(true);
    map.draw_frequency(o, 14_074_000);
    map.draw_text(o, "smeter", "S9");
    step(dev, "LEDs on, 14.074.000 on main, S9 on the small readout", 3.0)?;
    dev.click()?;
    println!("click sent");
    Ok(())
}

fn sweep(dev: &mut Tmate2, values: &[u8], secs: f32) -> Result<()> {
    const COLOURS: [(&str, (u8, u8, u8)); 6] = [
        ("red", (255, 0, 0)),
        ("green", (0, 255, 0)),
        ("blue", (0, 0, 255)),
        ("yellow", (255, 255, 0)),
        ("magenta", (255, 0, 255)),
        ("cyan", (0, 255, 255)),
    ];
    dev.output_mut().all_segments();
    for (i, &c) in values.iter().enumerate() {
        let (name, (r, g, b)) = COLOURS[i % COLOURS.len()];
        let o = dev.output_mut();
        o.set_contrast(c);
        o.set_rgb(r, g, b);
        println!("step {}: backlight {name}, contrast {c}", i + 1);
        hold(dev, secs)?;
    }
    Ok(())
}

fn demo(dev: &mut Tmate2, map: &SegmentMap) {
    let o = dev.output_mut();
    o.clear_segments();
    o.set_rgb(255, 255, 255);
    map.draw_frequency(o, 14_074_000);
    map.draw_text(o, "smeter", "S9");
    let mut lit: Vec<String> = [
        "underline.3",
        "unit.hz",
        "mode.usb",
        "vfo",
        "vfo.a",
        "rx",
        "bar.line",
        "e1",
        "e1.vol",
        "e2",
        "e2.shift",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    lit.extend((1..=9).map(|n| format!("bar.{n}")));
    lit.extend(
        [
            "s", "1", "3", "5", "7", "9", "20.plus", "20", "40.plus", "40", "60.plus", "60",
        ]
        .map(|s| format!("scale.{s}")),
    );
    for name in &lit {
        if !map.set(o, name, true) {
            println!("unknown segment {name}");
        }
    }
    println!("demo: 14.074.000 Hz, 100 Hz step underline, USB, VFO A, RX, S9 on bars and readout, E1=VOL, E2=SHIFT");
}
