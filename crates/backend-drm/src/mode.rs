//! Connector and mode selection, and the arithmetic behind the refresh rate
//! `wl_output` advertises (M2 T1).
//!
//! Governing doc: `docs/scene_graph_v1.md` §13.2.
//!
//! # Why this module is plain data
//!
//! Everything here works on small owned structs ([`ConnectorInfo`],
//! [`ModeInfo`], [`ModeTiming`]) rather than on `drm::control` types. That is
//! deliberate: the *policy* ("which screen, which mode") and the *arithmetic*
//! ("how fast is that mode, exactly") are the two parts of this backend that can
//! be wrong in a way no amount of staring at a TTY would reveal — and they are
//! the two parts CI can run, because they touch no hardware. The DRM-facing code
//! in [`crate::commit`] fills these structs in and does as it is told.

/// A mode's raw timings, in the units DRM reports them.
///
/// This is `drm_mode_modeinfo` reduced to the fields the refresh rate depends
/// on. The DRM-side code copies them out of a `drm::control::Mode`; the
/// arithmetic below never sees a hardware type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeTiming {
    /// Pixel clock in kHz (`drm_mode_modeinfo.clock`).
    pub clock_khz: u32,
    /// Total horizontal pixels per scanline, including blanking.
    pub htotal: u16,
    /// Total scanlines per frame, including blanking.
    pub vtotal: u16,
    /// Vertical scan multiplier; 0 and 1 both mean "no multiplier".
    pub vscan: u16,
    /// The mode is interlaced (`DRM_MODE_FLAG_INTERLACE`).
    pub interlaced: bool,
    /// The mode is double-scanned (`DRM_MODE_FLAG_DBLSCAN`).
    pub doublescan: bool,
}

/// Compute a mode's refresh rate in **millihertz**, or `None` if the timings
/// cannot produce one (a zero clock or a zero total — which a mode line should
/// never have, and which we refuse to divide by rather than guess around).
///
/// # Why not `Mode::vrefresh()`
///
/// `drm_mode_modeinfo` carries a `vrefresh` field in whole hertz. Rounding a
/// panel's 59.951 Hz to 60 and handing that to clients is exactly the kind of
/// plausible-looking lie T7's `wl_output` had to tell for want of hardware, and
/// this task exists to retire it. Clients schedule against the advertised
/// refresh, so a 0.08% error is a frame of drift every twenty minutes.
///
/// The formula is the kernel's own (`drm_mode_vrefresh`), carried out in
/// millihertz and with the divisions ordered to lose nothing:
///
/// ```text
///   refresh_mHz = clock_kHz * 1_000_000 / (htotal * vtotal)
/// ```
///
/// with interlacing doubling the field rate, double-scan halving it, and a
/// vertical-scan multiplier dividing it.
pub fn refresh_mhz(timing: &ModeTiming) -> Option<i32> {
    // u64 throughout: the numerator for a 600 MHz pixel clock is 6e14, which
    // overflows u32 long before it overflows this.
    let mut numerator = (timing.clock_khz as u64).checked_mul(1_000_000)?;
    let mut denominator = (timing.htotal as u64) * (timing.vtotal as u64);

    if timing.interlaced {
        // Interlaced modes scan two fields per frame, so fields arrive at twice
        // the frame rate — and it is the field rate DRM reports as the refresh.
        numerator *= 2;
    }
    if timing.doublescan {
        // Every line is scanned twice, so a frame takes twice as long.
        denominator *= 2;
    }
    if timing.vscan > 1 {
        denominator *= timing.vscan as u64;
    }

    if numerator == 0 || denominator == 0 {
        return None;
    }

    // Round to nearest millihertz rather than truncating: a mode that is exactly
    // 59.9995 Hz should advertise 60000 mHz, not 59999.
    let mhz = (numerator + denominator / 2) / denominator;
    Some(mhz.min(i32::MAX as u64) as i32)
}

/// One mode of one connector, reduced to what the selection policy needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeInfo {
    /// Index into the connector's own mode list — how the caller finds the real
    /// `drm::control::Mode` again after the policy has chosen.
    pub index: usize,
    /// Visible width in pixels.
    pub width: u16,
    /// Visible height in pixels.
    pub height: u16,
    /// The driver flagged this mode `DRM_MODE_TYPE_PREFERRED` — for a fixed
    /// panel this is its native resolution and the only mode that looks right.
    pub preferred: bool,
    /// Refresh in millihertz, per [`refresh_mhz`]; 0 when the timings gave none.
    pub refresh_mhz: i32,
}

/// One connector, reduced to what the selection policy needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorInfo {
    /// Index into the device's connector list, so the caller can find the handle.
    pub index: usize,
    /// Human name, e.g. `eDP-1` — for the log line, and for the diary entry this
    /// task owes about what the dev machine actually reports.
    pub name: String,
    /// The connector reports a display attached.
    pub connected: bool,
    /// Its mode list, in the order the driver gave them.
    pub modes: Vec<ModeInfo>,
}

/// What [`select_output`] chose: indices into the caller's own arrays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// Index into the connector list.
    pub connector: usize,
    /// Index into *that connector's* mode list.
    pub mode: usize,
}

/// Choose the output to drive: **the first connected connector that has any
/// mode**, at its preferred mode.
///
/// This is connector/mode policy v1 and it is deliberately the smallest rule
/// that lights up a laptop or a single monitor. Multi-output, hotplug, and
/// choosing a mode other than preferred are **M9** — the milestone plan says so
/// — and a cleverer rule here would be inventing policy the policy daemon (S1,
/// M4) will eventually own anyway.
///
/// A connected connector with an empty mode list is skipped rather than fatal:
/// that is what a connector reports mid-hotplug or when EDID reading failed, and
/// refusing to look at the next monitor because the first one is confused would
/// be a poor trade.
pub fn select_output(connectors: &[ConnectorInfo]) -> Option<Selection> {
    connectors
        .iter()
        .filter(|c| c.connected && !c.modes.is_empty())
        .find_map(|c| {
            select_mode(&c.modes).map(|mode| Selection {
                connector: c.index,
                mode,
            })
        })
}

/// Choose one connector's mode: the preferred one if the driver flagged any,
/// otherwise the largest by pixel area, breaking ties by the higher refresh and
/// then by the earlier position in the list.
///
/// The total ordering matters more than the ranking does: two runs on the same
/// hardware must pick the same mode, or "it looked right yesterday" stops being
/// evidence of anything.
fn select_mode(modes: &[ModeInfo]) -> Option<usize> {
    if let Some(preferred) = modes.iter().find(|m| m.preferred) {
        return Some(preferred.index);
    }
    modes
        .iter()
        .max_by_key(|m| {
            (
                // Area first: a bigger picture is the better default.
                (m.width as u64) * (m.height as u64),
                // Then refresh, so 1920×1080@144 beats 1920×1080@60.
                m.refresh_mhz as i64,
                // Then earliest-wins: `max_by_key` keeps the *last* maximum, so
                // negating the position makes the first one win instead.
                -(m.index as i64),
            )
        })
        .map(|m| m.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `ModeInfo` for the selection tests. Refresh defaults to 60 Hz;
    /// the tests that care state their own.
    fn mode(index: usize, width: u16, height: u16, preferred: bool) -> ModeInfo {
        ModeInfo {
            index,
            width,
            height,
            preferred,
            refresh_mhz: 60_000,
        }
    }

    /// The canonical 1080p60 mode line (CEA-861 1920×1080p): 148.5 MHz over a
    /// 2200×1125 total. It is exactly 60 Hz, so any arithmetic slip shows up as
    /// a number that is not 60000.
    #[test]
    fn a_1080p60_mode_line_is_exactly_60_000_mhz() {
        let timing = ModeTiming {
            clock_khz: 148_500,
            htotal: 2200,
            vtotal: 1125,
            vscan: 0,
            interlaced: false,
            doublescan: false,
        };
        assert_eq!(refresh_mhz(&timing), Some(60_000));
    }

    /// The case this whole function exists for: a panel whose real rate is not a
    /// round number. `vrefresh` would report 60; the timings say 59.951 Hz, and
    /// that is what a client scheduling against us needs to hear.
    #[test]
    fn a_non_integral_panel_rate_survives_to_millihertz() {
        // A typical eDP panel mode line: 138.7 MHz over 1560×1483.
        let timing = ModeTiming {
            clock_khz: 138_700,
            htotal: 1560,
            vtotal: 1483,
            vscan: 0,
            interlaced: false,
            doublescan: false,
        };
        // 138_700_000_000 / (1560 × 1483 = 2_313_480) = 59_952.97… mHz, which
        // rounds to 59_953 — the rounding is to the *nearest* millihertz, so
        // this also pins that the truncating version is not what we ship.
        assert_eq!(refresh_mhz(&timing), Some(59_953));
        assert_ne!(
            refresh_mhz(&timing),
            Some(60_000),
            "rounding this to 60 Hz is the lie this task retires"
        );
    }

    /// Interlaced modes report the *field* rate: twice the frame rate.
    #[test]
    fn interlacing_doubles_the_reported_rate() {
        let progressive = ModeTiming {
            clock_khz: 74_250,
            htotal: 2200,
            vtotal: 1125,
            vscan: 0,
            interlaced: false,
            doublescan: false,
        };
        let interlaced = ModeTiming {
            interlaced: true,
            ..progressive
        };
        assert_eq!(refresh_mhz(&progressive), Some(30_000));
        assert_eq!(refresh_mhz(&interlaced), Some(60_000));
    }

    /// Double-scan halves it, and a vertical-scan multiplier divides it.
    #[test]
    fn doublescan_and_vscan_divide_the_rate() {
        let base = ModeTiming {
            clock_khz: 148_500,
            htotal: 2200,
            vtotal: 1125,
            vscan: 0,
            interlaced: false,
            doublescan: false,
        };
        assert_eq!(
            refresh_mhz(&ModeTiming {
                doublescan: true,
                ..base
            }),
            Some(30_000)
        );
        assert_eq!(refresh_mhz(&ModeTiming { vscan: 2, ..base }), Some(30_000));
        // vscan 0 and 1 both mean "no multiplier" — a mode line uses either.
        assert_eq!(refresh_mhz(&ModeTiming { vscan: 1, ..base }), Some(60_000));
    }

    /// Degenerate timings produce `None`, not a division by zero and not a
    /// confident wrong number. The caller substitutes the default and says so.
    #[test]
    fn degenerate_timings_refuse_to_answer() {
        let zero_total = ModeTiming {
            clock_khz: 148_500,
            htotal: 0,
            vtotal: 1125,
            vscan: 0,
            interlaced: false,
            doublescan: false,
        };
        assert_eq!(refresh_mhz(&zero_total), None);

        let zero_clock = ModeTiming {
            clock_khz: 0,
            ..zero_total
        };
        assert_eq!(refresh_mhz(&zero_clock), None);
    }

    /// The policy in one sentence: first connected connector, preferred mode.
    /// Here the preferred mode is *smaller* than another on the same connector,
    /// which is the case that catches "pick the biggest" written by accident.
    #[test]
    fn the_first_connected_connector_and_its_preferred_mode_win() {
        let connectors = vec![
            ConnectorInfo {
                index: 0,
                name: "HDMI-A-1".into(),
                connected: false,
                modes: vec![mode(0, 3840, 2160, true)],
            },
            ConnectorInfo {
                index: 1,
                name: "eDP-1".into(),
                connected: true,
                modes: vec![mode(0, 1920, 1200, false), mode(1, 1280, 800, true)],
            },
            ConnectorInfo {
                index: 2,
                name: "DP-1".into(),
                connected: true,
                modes: vec![mode(0, 2560, 1440, true)],
            },
        ];
        assert_eq!(
            select_output(&connectors),
            Some(Selection {
                connector: 1,
                mode: 1
            })
        );
    }

    /// With nothing flagged preferred, the largest mode wins; equal areas are
    /// broken by refresh, and equal refresh by list order. Determinism is the
    /// property under test — a run that picks differently on identical input
    /// makes every "it looked right" observation worthless.
    #[test]
    fn without_a_preferred_mode_the_largest_wins_deterministically() {
        let modes = vec![
            ModeInfo {
                index: 0,
                width: 1920,
                height: 1080,
                preferred: false,
                refresh_mhz: 60_000,
            },
            ModeInfo {
                index: 1,
                width: 1920,
                height: 1080,
                preferred: false,
                refresh_mhz: 144_000,
            },
            ModeInfo {
                index: 2,
                width: 1280,
                height: 1024,
                preferred: false,
                refresh_mhz: 240_000,
            },
            ModeInfo {
                index: 3,
                width: 1920,
                height: 1080,
                preferred: false,
                refresh_mhz: 144_000,
            },
        ];
        // Largest area (1920×1080), highest refresh among those (144 Hz), and of
        // the two 144 Hz entries the earlier one.
        assert_eq!(select_mode(&modes), Some(1));
    }

    /// A connected connector with no modes is skipped, not fatal — the next
    /// connected one is tried. A disconnected connector is never considered.
    #[test]
    fn modeless_and_disconnected_connectors_are_skipped() {
        let connectors = vec![
            ConnectorInfo {
                index: 0,
                name: "DP-1".into(),
                connected: true,
                modes: vec![],
            },
            ConnectorInfo {
                index: 1,
                name: "DP-2".into(),
                connected: false,
                modes: vec![mode(0, 1920, 1080, true)],
            },
            ConnectorInfo {
                index: 2,
                name: "eDP-1".into(),
                connected: true,
                modes: vec![mode(0, 1366, 768, true)],
            },
        ];
        assert_eq!(
            select_output(&connectors),
            Some(Selection {
                connector: 2,
                mode: 0
            })
        );
    }

    /// Nothing connected: `None`, which the caller turns into a diagnostic
    /// naming every connector it saw rather than a panic.
    #[test]
    fn no_connected_connector_selects_nothing() {
        let connectors = vec![ConnectorInfo {
            index: 0,
            name: "HDMI-A-1".into(),
            connected: false,
            modes: vec![mode(0, 1920, 1080, true)],
        }];
        assert_eq!(select_output(&connectors), None);
        assert_eq!(select_output(&[]), None);
    }
}
