//! Amateur band table (IARU Region 1 edges; generous enough for other regions)
//! used for band up / down and the per-band frequency memory.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    pub name: &'static str,
    pub low: u64,
    pub high: u64,
    /// Where to land the first time the band is selected.
    pub default_hz: u64,
}

pub const BANDS: &[Band] = &[
    Band {
        name: "160m",
        low: 1_810_000,
        high: 2_000_000,
        default_hz: 1_840_000,
    },
    Band {
        name: "80m",
        low: 3_500_000,
        high: 4_000_000,
        default_hz: 3_573_000,
    },
    Band {
        name: "60m",
        low: 5_250_000,
        high: 5_450_000,
        default_hz: 5_357_000,
    },
    Band {
        name: "40m",
        low: 7_000_000,
        high: 7_300_000,
        default_hz: 7_074_000,
    },
    Band {
        name: "30m",
        low: 10_100_000,
        high: 10_150_000,
        default_hz: 10_136_000,
    },
    Band {
        name: "20m",
        low: 14_000_000,
        high: 14_350_000,
        default_hz: 14_074_000,
    },
    Band {
        name: "17m",
        low: 18_068_000,
        high: 18_168_000,
        default_hz: 18_100_000,
    },
    Band {
        name: "15m",
        low: 21_000_000,
        high: 21_450_000,
        default_hz: 21_074_000,
    },
    Band {
        name: "12m",
        low: 24_890_000,
        high: 24_990_000,
        default_hz: 24_915_000,
    },
    Band {
        name: "10m",
        low: 28_000_000,
        high: 29_700_000,
        default_hz: 28_074_000,
    },
    Band {
        name: "6m",
        low: 50_000_000,
        high: 54_000_000,
        default_hz: 50_313_000,
    },
];

/// Index of the band containing `hz`, if any.
pub fn band_of(hz: u64) -> Option<usize> {
    BANDS.iter().position(|b| (b.low..=b.high).contains(&hz))
}

/// Next band up (`dir = 1`) or down (`dir = -1`) from `hz`, wrapping round.
/// Outside any band, moves to the nearest band in that direction.
pub fn step_band(hz: u64, dir: i32) -> usize {
    let n = BANDS.len();
    match band_of(hz) {
        Some(i) => (i as i32 + dir).rem_euclid(n as i32) as usize,
        None if dir > 0 => BANDS.iter().position(|b| b.low > hz).unwrap_or(0),
        None => BANDS.iter().rposition(|b| b.high < hz).unwrap_or(n - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_stepping_wraps_and_handles_gaps() {
        assert_eq!(BANDS[step_band(14_074_000, 1)].name, "17m");
        assert_eq!(BANDS[step_band(14_074_000, -1)].name, "30m");
        assert_eq!(BANDS[step_band(50_100_000, 1)].name, "160m");
        assert_eq!(BANDS[step_band(1_850_000, -1)].name, "6m");
        // 9 MHz is between 40m and 30m.
        assert_eq!(BANDS[step_band(9_000_000, 1)].name, "30m");
        assert_eq!(BANDS[step_band(9_000_000, -1)].name, "40m");
    }
}
