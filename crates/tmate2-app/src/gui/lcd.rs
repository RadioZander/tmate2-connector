//! On-screen mirror of the TMATE-2 LCD, drawn from the same output report
//! that is sent to the device. Layout approximates the real display.

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};
use tmate2_hid::{OutputReport, SegmentMap};

const W: f32 = 640.0;
const H: f32 = 212.0;

/// Text indicators: (segment name, text, x, y, size). Positions measured from
/// a photo of an HW1.1 / FW1.5 unit with every segment lit.
const LABELS: &[(&str, &str, f32, f32, f32)] = &[
    ("lp", "LP", 34.0, 21.0, 15.0),
    ("att", "ATT", 75.0, 21.0, 15.0),
    ("att.1", "1", 110.0, 21.0, 15.0),
    ("att.2", "2", 127.0, 21.0, 15.0),
    ("pre", "PRE", 165.0, 21.0, 15.0),
    ("pre.1", "1", 198.0, 21.0, 15.0),
    ("pre.2", "2", 215.0, 21.0, 15.0),
    ("rx", "RX", 37.0, 44.0, 15.0),
    ("tx", "TX", 37.0, 67.0, 15.0),
    ("unit.dbm", "dBm", 497.0, 68.0, 14.0),
    ("stereo", "STEREO", 584.0, 33.0, 15.0),
    ("scale.s", "S", 31.0, 88.0, 15.0),
    ("scale.1", "1", 57.0, 88.0, 15.0),
    ("scale.3", "3", 95.0, 88.0, 15.0),
    ("scale.5", "5", 133.0, 88.0, 15.0),
    ("scale.7", "7", 171.0, 88.0, 15.0),
    ("scale.9", "9", 210.0, 88.0, 15.0),
    ("scale.10", "10", 241.0, 88.0, 15.0),
    ("scale.20.plus", "+", 268.0, 88.0, 15.0),
    ("scale.20", "20", 283.0, 88.0, 15.0),
    ("scale.40.plus", "+", 345.0, 88.0, 15.0),
    ("scale.40", "40", 361.0, 88.0, 15.0),
    ("scale.60.plus", "+", 422.0, 88.0, 15.0),
    ("scale.60", "60", 438.0, 88.0, 15.0),
    ("scale.w", "W", 464.0, 88.0, 15.0),
    ("vfo", "VFO", 44.0, 116.0, 15.0),
    ("vfo.a", "A", 80.0, 116.0, 15.0),
    ("vfo.b", "B", 102.0, 116.0, 15.0),
    ("left.nr", "NR", 39.0, 147.0, 15.0),
    ("left.nb", "NB", 69.0, 147.0, 15.0),
    ("left.ant", "AN", 99.0, 147.0, 15.0),
    ("unit.m", "m", 468.0, 116.0, 17.0),
    ("unit.w", "W", 489.0, 116.0, 17.0),
    ("unit.k", "K", 467.0, 145.0, 19.0),
    ("unit.hz", "Hz", 487.0, 147.0, 17.0),
    ("mode.am", "AM", 541.0, 55.0, 15.0),
    ("mode.sam", "SAM", 599.0, 55.0, 15.0),
    ("mode.lsb", "LSB", 541.0, 78.0, 15.0),
    ("mode.usb", "USB", 598.0, 78.0, 15.0),
    ("mode.cw", "CW", 538.0, 102.0, 15.0),
    ("mode.cw.plus", "+", 559.0, 98.0, 11.0),
    ("mode.cw.minus", "-", 559.0, 106.0, 11.0),
    ("mode.fm", "FM", 590.0, 102.0, 15.0),
    ("mode.fm.w", "W", 613.0, 102.0, 15.0),
    ("mode.drm", "DRM", 546.0, 126.0, 15.0),
    ("mode.dsb", "DSB", 598.0, 126.0, 15.0),
    ("mode.dig", "DIG", 540.0, 149.0, 15.0),
    ("mode.dig.plus", "+", 562.0, 145.0, 11.0),
    ("mode.dig.minus", "-", 562.0, 153.0, 11.0),
    ("err", "ERR", 598.0, 149.0, 15.0),
    ("e1.vol", "VOL", 87.0, 190.0, 15.0),
    ("e1.rfg", "RFG", 126.0, 190.0, 15.0),
    ("e1.sql", "SQL", 169.0, 190.0, 15.0),
    ("e1.drv", "DRV", 209.0, 190.0, 15.0),
    ("e1.nr", "NR", 252.0, 190.0, 15.0),
    ("e1.nb", "NB", 288.0, 190.0, 15.0),
    ("e1.an", "AN", 326.0, 190.0, 15.0),
    ("e2.high", "HIGH", 415.0, 190.0, 15.0),
    ("e2.low", "LOW", 460.0, 190.0, 15.0),
    ("e2.shift", "SHIFT", 508.0, 190.0, 15.0),
    ("e2.rit", "RIT", 558.0, 190.0, 15.0),
    ("e2.xit", "XIT", 595.0, 190.0, 15.0),
];

/// Boxed markers: (segment name, text, x, y).
const BOXES: &[(&str, &str, f32, f32)] = &[("e1", "E1", 43.0, 190.0), ("e2", "E2", 368.0, 190.0)];

/// S-meter bars: (x centre, top, width). Nine thin bars, then six tall blocks;
/// all sit on the same baseline.
const BARS: [(f32, f32, f32); 15] = [
    (57.0, 43.0, 7.0),
    (76.0, 43.0, 7.0),
    (95.0, 43.0, 7.0),
    (114.0, 43.0, 7.0),
    (133.0, 43.0, 7.0),
    (152.0, 43.0, 7.0),
    (171.0, 43.0, 7.0),
    (190.0, 43.0, 7.0),
    (210.0, 43.0, 7.0),
    (240.0, 37.0, 28.0),
    (279.0, 35.0, 28.0),
    (318.0, 32.0, 28.0),
    (357.0, 30.0, 28.0),
    (395.0, 28.0, 28.0),
    (432.0, 25.0, 28.0),
];
const BAR_BOTTOM: f32 = 67.0;

/// Main digit left edges, digit 9 (leftmost) first.
const MAIN_X: [f32; 9] = [121.0, 157.0, 193.0, 231.0, 267.0, 303.0, 342.0, 378.0, 414.0];

struct Painter<'a> {
    p: egui::Painter,
    origin: Pos2,
    scale: f32,
    report: &'a OutputReport,
    map: &'a SegmentMap,
    on: Color32,
    off: Color32,
}

impl Painter<'_> {
    fn pt(&self, x: f32, y: f32) -> Pos2 {
        self.origin + Vec2::new(x, y) * self.scale
    }

    fn lit(&self, name: &str) -> bool {
        self.map.get(name).is_some_and(|a| self.report.segment(a))
    }

    fn colour(&self, name: &str) -> Color32 {
        if self.lit(name) {
            self.on
        } else {
            self.off
        }
    }

    fn rect(&self, name: &str, x: f32, y: f32, w: f32, h: f32) {
        let r = Rect::from_min_max(self.pt(x, y), self.pt(x + w, y + h));
        self.p.rect_filled(r, 1.0, self.colour(name));
    }

    fn text(&self, name: &str, text: &str, x: f32, y: f32, size: f32) {
        self.p.text(
            self.pt(x, y),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(size * self.scale),
            self.colour(name),
        );
    }

    /// A 7-segment digit with its top-left at (x, y).
    fn digit(&self, group: &str, n: usize, x: f32, y: f32, w: f32, h: f32) {
        let t = (w * 0.16).max(2.0);
        let half = h / 2.0;
        // (segment, horizontal?, x, y, length)
        let segs = [
            ('a', true, x + t * 0.5, y, w - t),
            ('b', false, x + w - t, y + t * 0.5, half - t * 0.5),
            ('c', false, x + w - t, y + half, half - t * 0.5),
            ('d', true, x + t * 0.5, y + h - t, w - t),
            ('e', false, x, y + half, half - t * 0.5),
            ('f', false, x, y + t * 0.5, half - t * 0.5),
            ('g', true, x + t * 0.5, y + half - t * 0.5, w - t),
        ];
        for (seg, horizontal, sx, sy, len) in segs {
            let name = format!("{group}.{n}.{seg}");
            let c = t / 2.0;
            let pts: Vec<Pos2> = if horizontal {
                vec![
                    self.pt(sx, sy + c),
                    self.pt(sx + c, sy),
                    self.pt(sx + len - c, sy),
                    self.pt(sx + len, sy + c),
                    self.pt(sx + len - c, sy + t),
                    self.pt(sx + c, sy + t),
                ]
            } else {
                vec![
                    self.pt(sx + c, sy),
                    self.pt(sx + t, sy + c),
                    self.pt(sx + t, sy + len - c),
                    self.pt(sx + c, sy + len),
                    self.pt(sx, sy + len - c),
                    self.pt(sx, sy + c),
                ]
            };
            self.p
                .add(Shape::convex_polygon(pts, self.colour(&name), Stroke::NONE));
        }
    }
}

/// LCD background from the backlight colour (a dark grey when it is off).
fn background(rgb: (u8, u8, u8)) -> Color32 {
    let (r, g, b) = rgb;
    let lift = |c: u8| (40.0 + c as f32 * 0.75) as u8;
    if (r, g, b) == (0, 0, 0) {
        Color32::from_rgb(45, 48, 45)
    } else {
        Color32::from_rgb(lift(r), lift(g), lift(b))
    }
}

pub fn show(ui: &mut egui::Ui, report: &OutputReport, map: &SegmentMap) {
    let width = ui.available_width().min(900.0);
    let scale = width / W;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(W, H) * scale, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 8.0 * scale, background(report.rgb()));

    let lcd = Painter {
        p: painter,
        origin: rect.min,
        scale,
        report,
        map,
        on: Color32::from_rgb(20, 22, 24),
        off: Color32::from_black_alpha(16),
    };

    for (name, text, x, y, size) in LABELS {
        lcd.text(name, text, *x, *y, *size);
    }
    for (name, text, x, y) in BOXES {
        lcd.text(name, text, *x, *y, 15.0);
        let r = Rect::from_center_size(lcd.pt(*x, *y), Vec2::new(27.0, 21.0) * scale);
        lcd.p.rect_stroke(
            r,
            1.0,
            Stroke::new(1.5 * scale, lcd.colour(name)),
            StrokeKind::Inside,
        );
    }

    for (i, (x, top, w)) in BARS.iter().enumerate() {
        lcd.rect(&format!("bar.{}", i + 1), x - w / 2.0, *top, *w, BAR_BOTTOM - top);
    }
    lcd.rect("bar.line", 51.0, 73.0, 395.0, 2.0);

    // Small readout next to the bars: minus sign and three digits.
    lcd.rect("smeter.minus", 456.0, 38.0, 8.0, 2.5);
    for n in 1..=3 {
        let x = 467.0 + (3 - n) as f32 * 19.0;
        lcd.digit("smeter", n, x, 26.0, 14.0, 27.0);
    }

    // Main display: 9 digits (1 = rightmost), dots and step underlines.
    for n in 1..=9 {
        let x = MAIN_X[9 - n];
        lcd.digit("main", n, x, 106.0, 32.0, 47.0);
        lcd.rect(&format!("underline.{n}"), x, 157.0, 32.0, 2.5);
    }
    lcd.rect("dot.mhz", 223.0, 150.0, 4.5, 4.5);
    lcd.rect("dot.khz", 334.0, 150.0, 4.5, 4.5);
}
