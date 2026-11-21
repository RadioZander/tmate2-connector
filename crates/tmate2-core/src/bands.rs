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

/// Bands above 6 m, used only to place a TCI-X server's own band list
/// (`cap:band`); the plain band up / down above stays HF and 6 m.
const VHF_UHF: &[(&str, u64, u64)] = &[
    ("4m", 69_900_000, 70_500_000),
    ("2m", 144_000_000, 148_000_000),
    ("1.25m", 222_000_000, 225_000_000),
    ("70cm", 420_000_000, 450_000_000),
    ("23cm", 1_240_000_000, 1_300_000_000),
];

/// Edges of a band by name (case-insensitive), from either table.
pub fn band_range(name: &str) -> Option<(u64, u64)> {
    BANDS
        .iter()
        .find(|b| b.name.eq_ignore_ascii_case(name))
        .map(|b| (b.low, b.high))
        .or_else(|| {
            VHF_UHF
                .iter()
                .find(|(n, ..)| n.eq_ignore_ascii_case(name))
                .map(|&(_, lo, hi)| (lo, hi))
        })
}

/// Step through a server's band list (in its order, which is low to high)
/// from `hz`: the next (`dir = 1`) or previous (`dir = -1`) entry, wrapping
/// round. Outside every listed band, the nearest listed band that way.
/// `None` for an empty list.
pub fn step_in_list(list: &[String], hz: u64, dir: i32) -> Option<usize> {
    let n = list.len();
    if n == 0 {
        return None;
    }
    let range = |i: usize| band_range(&list[i]);
    let here = (0..n).find(|&i| range(i).is_some_and(|(lo, hi)| (lo..=hi).contains(&hz)));
    Some(match here {
        Some(i) => (i as i32 + dir).rem_euclid(n as i32) as usize,
        None if dir > 0 => (0..n)
            .find(|&i| range(i).is_some_and(|(lo, _)| lo > hz))
            .unwrap_or(0),
        None => (0..n)
            .rev()
            .find(|&i| range(i).is_some_and(|(_, hi)| hi < hz))
            .unwrap_or(n - 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_through_a_server_band_list() {
        // The IC-7100's list from kadrad-hub: no 60m, but 2m and 70cm.
        let list: Vec<String> = "160m,80m,40m,30m,20m,17m,15m,12m,10m,6m,2m,70cm"
            .split(',')
            .map(str::to_string)
            .collect();
        let at = |hz, dir| list[step_in_list(&list, hz, dir).unwrap()].as_str();
        assert_eq!(at(14_074_000, 1), "17m");
        assert_eq!(at(50_313_000, 1), "2m");
        assert_eq!(at(145_500_000, 1), "70cm");
        assert_eq!(at(433_500_000, 1), "160m");
        assert_eq!(at(1_840_000, -1), "70cm");
        // 60m isn't listed: up goes to 40m, down to 80m.
        assert_eq!(at(5_357_000, 1), "40m");
        assert_eq!(at(5_357_000, -1), "80m");
        assert_eq!(step_in_list(&[], 14_000_000, 1), None);
    }

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
