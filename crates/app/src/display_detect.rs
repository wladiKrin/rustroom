//! Which colours the monitor in front of the user shows (2026-11-18) — so the preview is converted into them on Windows
//! and Linux, as macOS does by itself (`display_colorspace`).
//!
//! ⚠ Two halves, on purpose: the platform layer only GATHERS facts ([`facts`]) and ONE pure function decides
//! ([`decide`]), so every rule is a unit test and none hides in FFI. The app re-gathers every two seconds and decides
//! again only when the facts changed (`display_flow`).

use color::edid::{self, Primaries};
use color::DisplayProfile;

/// What the operating system says about the monitor holding the window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Facts {
    /// The OS converts the window's colours itself — Windows' Auto Color Management or HDR.
    pub os_managed: bool,
    /// The profile the OS assigned to the monitor (its file name and bytes) — never the generic sRGB one.
    pub icc: Option<(String, Vec<u8>)>,
    /// EDIDs: the window's monitor's on Windows; every connected monitor's on Linux.
    pub edids: Vec<Vec<u8>>,
}

/// What the preview is converted into under Automatic, and why.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Detected {
    /// The OS matches the window to the monitor itself: converting as well would be twice.
    OsManaged,
    /// The monitor's own colours.
    Profile(&'static DisplayProfile, Source),
    /// No conversion, for this reason.
    Srgb(Why),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The profile the OS assigned.
    Icc,
    /// The primaries the monitor reports.
    Edid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// No profile and no EDID (macOS, which manages colour itself, lands here too).
    NoData,
    /// The monitor says it shows sRGB, or its primaries are sRGB's.
    SrgbMonitor,
    /// Linux: several monitors with different primaries, and which one holds the window is not known.
    SeveralMonitors,
    /// The profile or EDID could not be read.
    Unreadable,
}

/// How near sRGB's primaries (in x and y) an EDID may be and still count as sRGB: a few of EDID's own 1/1024 steps,
/// so a near-identity conversion never costs the GPU's fast paths. ⚠ The spec's 0.005.
pub const SRGB_TOLERANCE: f64 = 0.005;

/// The decision, in the spec's order: the OS manages colour → nothing; an assigned profile → it; one EDID's
/// primaries (or several that agree) unless sRGB → them; else sRGB.
pub fn decide(f: &Facts) -> Detected {
    if f.os_managed {
        return Detected::OsManaged;
    }
    if let Some((name, bytes)) = &f.icc {
        if let Some(p) = DisplayProfile::from_icc(name, bytes) {
            return Detected::Profile(p, Source::Icc);
        }
    }
    let parsed: Vec<edid::Edid> = f.edids.iter().filter_map(|b| edid::parse(b)).collect();
    let Some(first) = parsed.first() else {
        return Detected::Srgb(if f.edids.is_empty() && f.icc.is_none() {
            Why::NoData
        } else {
            Why::Unreadable
        });
    };
    // Two of one model report the same primaries; two that differ cannot both be right for one window.
    if parsed
        .iter()
        .any(|e| !e.primaries.is_close_to(&first.primaries, 0.002))
    {
        return Detected::Srgb(Why::SeveralMonitors);
    }
    if first.srgb_default
        || first
            .primaries
            .is_close_to(&Primaries::SRGB, SRGB_TOLERANCE)
    {
        return Detected::Srgb(Why::SrgbMonitor);
    }
    let name = first
        .name
        .clone()
        .unwrap_or_else(|| "This monitor".to_string());
    match DisplayProfile::from_primaries(&name, &first.primaries) {
        Some(p) => Detected::Profile(p, Source::Edid),
        None => Detected::Srgb(Why::Unreadable),
    }
}

impl Detected {
    /// The display it means — before the manual setting has its say (`Settings::effective_display`).
    pub fn display(&self) -> color::Display {
        match self {
            Detected::Profile(p, _) => color::Display::Profile(p),
            Detected::OsManaged | Detected::Srgb(_) => color::Display::SRGB,
        }
    }

    /// What Settings → Display shows under Automatic.
    pub fn caption(&self) -> String {
        match self {
            Detected::OsManaged => "The system matches the colours".to_string(),
            Detected::Profile(p, Source::Edid) => format!("{}'s own colours", p.name),
            Detected::Profile(p, Source::Icc) => format!("Profile {}", p.name),
            Detected::Srgb(Why::NoData) => "sRGB, no monitor data".to_string(),
            Detected::Srgb(Why::SrgbMonitor) => "sRGB, the monitor's own".to_string(),
            Detected::Srgb(Why::SeveralMonitors) => "sRGB, several different monitors".to_string(),
            Detected::Srgb(Why::Unreadable) => "sRGB, the monitor's data is unreadable".to_string(),
        }
    }
}

/// The facts for the monitor holding `window` (its Win32 handle) on Windows, for every connected monitor on Linux,
/// nothing elsewhere.
pub fn facts(window: Option<isize>) -> Facts {
    platform_facts(window)
}

#[cfg(windows)]
fn platform_facts(window: Option<isize>) -> Facts {
    windows::facts(window)
}

#[cfg(target_os = "linux")]
fn platform_facts(_window: Option<isize>) -> Facts {
    Facts {
        edids: drm_edids(std::path::Path::new("/sys/class/drm")),
        ..Facts::default()
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn platform_facts(_window: Option<isize>) -> Facts {
    Facts::default()
}

/// The EDIDs of the connected outputs under a `/sys/class/drm`-shaped `root` (`card0-DP-1/{status,edid}`), sorted by
/// connector. ⚠ Plain files, so it is tested on every system against a directory the test builds.
pub fn drm_edids(root: &std::path::Path) -> Vec<Vec<u8>> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut outs: Vec<_> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains('-'))
        })
        .filter(|p| {
            std::fs::read_to_string(p.join("status")).is_ok_and(|s| s.trim() == "connected")
        })
        .collect();
    outs.sort();
    outs.into_iter()
        .filter_map(|p| std::fs::read(p.join("edid")).ok())
        .filter(|b| !b.is_empty())
        .collect()
}

/// The registry key under `HKLM\SYSTEM\CurrentControlSet\Enum` holding a monitor's EDID, from its device interface
/// name (`\\?\DISPLAY#GBT270D#5&3031ade7&0&UID4357#{…}` → `DISPLAY\GBT270D\5&3031ade7&0&UID4357`). ⚠ Pure, so tested
/// everywhere.
pub fn edid_key(interface: &str) -> Option<String> {
    let parts: Vec<&str> = interface.split('#').collect();
    match parts.as_slice() {
        [head, model, instance, _guid] if head.ends_with("DISPLAY") => Some(format!(
            "SYSTEM\\CurrentControlSet\\Enum\\DISPLAY\\{model}\\{instance}\\Device Parameters"
        )),
        _ => None,
    }
}

#[cfg(windows)]
mod windows {
    //! The Windows half: the window's monitor (`MonitorFromWindow` → `GetMonitorInfoW` → its device), its EDID from
    //! the registry, its Advanced Color state (`DisplayConfigGetDeviceInfo`) and its assigned profile
    //! (`GetICMProfileW`). ⚠ Declared by hand, like `console.rs`, with the layouts the 2026-11-17 probe read the M27Q
    //! through; every call that fails leaves its fact empty — never a panic.

    use super::Facts;
    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Luid {
        low: u32,
        high: i32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct PathSource {
        adapter: Luid,
        id: u32,
        mode_idx: u32,
        status: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct PathTarget {
        adapter: Luid,
        id: u32,
        mode_idx: u32,
        output_technology: u32,
        rotation: u32,
        scaling: u32,
        refresh: [u32; 2],
        scan_line_ordering: u32,
        available: i32,
        status: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct PathInfo {
        source: PathSource,
        target: PathTarget,
        flags: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ModeInfo([u8; 64]);
    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Header {
        kind: u32,
        size: u32,
        adapter: Luid,
        id: u32,
    }
    #[repr(C)]
    struct SourceName {
        header: Header,
        gdi_name: [u16; 32],
    }
    #[repr(C)]
    #[derive(Default)]
    struct AdvancedColor {
        header: Header,
        value: u32,
        encoding: u32,
        bits: u32,
    }
    #[repr(C)]
    #[derive(Default)]
    struct AdvancedColor2 {
        header: Header,
        value: u32,
        encoding: u32,
        bits: u32,
        active_mode: u32,
    }
    #[repr(C)]
    struct MonitorInfoEx {
        size: u32,
        monitor: [i32; 4],
        work: [i32; 4],
        flags: u32,
        device: [u16; 32],
    }
    #[repr(C)]
    struct DisplayDevice {
        size: u32,
        name: [u16; 32],
        string: [u16; 128],
        state: u32,
        id: [u16; 128],
        key: [u16; 128],
    }

    const QDC_ONLY_ACTIVE_PATHS: u32 = 2;
    const GET_SOURCE_NAME: u32 = 1;
    const GET_ADVANCED_COLOR_INFO: u32 = 9;
    const GET_ADVANCED_COLOR_INFO_2: u32 = 15;
    const MONITOR_DEFAULTTONEAREST: u32 = 2;
    const MONITOR_DEFAULTTOPRIMARY: u32 = 1;
    const EDD_GET_DEVICE_INTERFACE_NAME: u32 = 1;
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const RRF_RT_REG_BINARY: u32 = 0x8;

    #[link(name = "user32")]
    extern "system" {
        fn MonitorFromWindow(hwnd: isize, flags: u32) -> isize;
        fn MonitorFromPoint(x: i32, y: i32, flags: u32) -> isize;
        fn GetMonitorInfoW(monitor: isize, info: *mut MonitorInfoEx) -> i32;
        fn EnumDisplayDevicesW(
            device: *const u16,
            index: u32,
            dd: *mut DisplayDevice,
            flags: u32,
        ) -> i32;
        fn GetDisplayConfigBufferSizes(flags: u32, paths: *mut u32, modes: *mut u32) -> i32;
        fn QueryDisplayConfig(
            flags: u32,
            npaths: *mut u32,
            paths: *mut PathInfo,
            nmodes: *mut u32,
            modes: *mut ModeInfo,
            topology: *mut u32,
        ) -> i32;
        fn DisplayConfigGetDeviceInfo(packet: *mut Header) -> i32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn CreateDCW(
            driver: *const u16,
            device: *const u16,
            port: *const u16,
            mode: *const c_void,
        ) -> isize;
        fn DeleteDC(dc: isize) -> i32;
        fn GetICMProfileW(dc: isize, size: *mut u32, name: *mut u16) -> i32;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn RegGetValueW(
            key: isize,
            subkey: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            size: *mut u32,
        ) -> i32;
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    fn text(w: &[u16]) -> String {
        String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
    }

    pub(super) fn facts(window: Option<isize>) -> Facts {
        // SAFETY (this whole function): every call takes a handle from the call before it or a struct sized here;
        // a failing call returns 0 and leaves the fact empty.
        let monitor = unsafe {
            match window {
                Some(hwnd) => MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                None => MonitorFromPoint(0, 0, MONITOR_DEFAULTTOPRIMARY),
            }
        };
        let mut mi = MonitorInfoEx {
            size: std::mem::size_of::<MonitorInfoEx>() as u32,
            monitor: [0; 4],
            work: [0; 4],
            flags: 0,
            device: [0; 32],
        };
        if monitor == 0 || unsafe { GetMonitorInfoW(monitor, &mut mi) } == 0 {
            return Facts::default();
        }
        let device = text(&mi.device);
        Facts {
            os_managed: os_managed(&device),
            icc: icc(&mi.device),
            edids: edid(&mi.device).into_iter().collect(),
        }
    }

    /// Advanced Color on this monitor's path: an active WCG (Auto Color Management) or HDR mode, or — on a Windows
    /// without the newer query — HDR switched on.
    fn os_managed(device: &str) -> bool {
        let (mut np, mut nm) = (0u32, 0u32);
        if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) } != 0 {
            return false;
        }
        let mut paths = vec![PathInfo::default(); np as usize];
        let mut modes = vec![ModeInfo([0; 64]); nm as usize];
        let rc = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut np,
                paths.as_mut_ptr(),
                &mut nm,
                modes.as_mut_ptr(),
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            return false;
        }
        for p in paths.iter().take(np as usize) {
            let mut name = SourceName {
                header: Header {
                    kind: GET_SOURCE_NAME,
                    size: std::mem::size_of::<SourceName>() as u32,
                    adapter: p.source.adapter,
                    id: p.source.id,
                },
                gdi_name: [0; 32],
            };
            if unsafe { DisplayConfigGetDeviceInfo(&mut name.header) } != 0
                || text(&name.gdi_name) != device
            {
                continue;
            }
            let header = |kind, size| Header {
                kind,
                size,
                adapter: p.target.adapter,
                id: p.target.id,
            };
            let mut two = AdvancedColor2 {
                header: header(
                    GET_ADVANCED_COLOR_INFO_2,
                    std::mem::size_of::<AdvancedColor2>() as u32,
                ),
                ..Default::default()
            };
            if unsafe { DisplayConfigGetDeviceInfo(&mut two.header) } == 0 {
                return two.active_mode != 0; // 1 WCG (Auto Color Management), 2 HDR
            }
            let mut one = AdvancedColor {
                header: header(
                    GET_ADVANCED_COLOR_INFO,
                    std::mem::size_of::<AdvancedColor>() as u32,
                ),
                ..Default::default()
            };
            return unsafe { DisplayConfigGetDeviceInfo(&mut one.header) } == 0
                && one.value & 0b10 != 0;
        }
        false
    }

    /// The profile assigned to the monitor, unless it is Windows' generic sRGB one.
    fn icc(device: &[u16; 32]) -> Option<(String, Vec<u8>)> {
        let dc = unsafe {
            CreateDCW(
                std::ptr::null(),
                device.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
            )
        };
        if dc == 0 {
            return None;
        }
        let mut buf = [0u16; 520];
        let mut size = buf.len() as u32;
        let ok = unsafe { GetICMProfileW(dc, &mut size, buf.as_mut_ptr()) } != 0;
        unsafe { DeleteDC(dc) };
        let path = std::path::PathBuf::from(text(&buf));
        let file = path.file_name()?.to_string_lossy().into_owned();
        if !ok || file.eq_ignore_ascii_case("sRGB Color Space Profile.icm") {
            return None;
        }
        let bytes = std::fs::read(&path).ok()?;
        let name = path
            .file_stem()
            .map_or(file, |s| s.to_string_lossy().into_owned());
        Some((name, bytes))
    }

    /// The monitor's EDID: its device interface name → its key in the registry → the `EDID` value.
    fn edid(device: &[u16; 32]) -> Option<Vec<u8>> {
        let mut dd = DisplayDevice {
            size: std::mem::size_of::<DisplayDevice>() as u32,
            name: [0; 32],
            string: [0; 128],
            state: 0,
            id: [0; 128],
            key: [0; 128],
        };
        if unsafe {
            EnumDisplayDevicesW(device.as_ptr(), 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME)
        } == 0
        {
            return None;
        }
        let key = wide(&super::edid_key(&text(&dd.id))?);
        let value = wide("EDID");
        let mut data = vec![0u8; 1024];
        let mut size = data.len() as u32;
        let rc = unsafe {
            RegGetValueW(
                HKEY_LOCAL_MACHINE,
                key.as_ptr(),
                value.as_ptr(),
                RRF_RT_REG_BINARY,
                std::ptr::null_mut(),
                data.as_mut_ptr().cast(),
                &mut size,
            )
        };
        (rc == 0).then(|| {
            data.truncate(size as usize);
            data
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M27Q: &[u8] = include_bytes!("../../color/testdata/m27q.edid");

    fn facts(edids: &[&[u8]]) -> Facts {
        Facts {
            edids: edids.iter().map(|b| b.to_vec()).collect(),
            ..Facts::default()
        }
    }

    /// An EDID made sRGB by writing sRGB's chromaticities into the M27Q's block (the parser's own round trip is
    /// `color::edid`'s test).
    fn srgb_edid(flag: bool) -> Vec<u8> {
        let mut b = M27Q.to_vec();
        let enc = |v: f64| (v * 1024.0).round() as u16;
        let p = Primaries::SRGB;
        let xs: Vec<u16> = [p.red, p.green, p.blue, p.white]
            .iter()
            .flat_map(|&(x, y)| [enc(x), enc(y)])
            .collect();
        for (i, v) in xs.iter().enumerate() {
            b[27 + i] = (v >> 2) as u8;
        }
        b[25] = ((xs[0] & 3) << 6 | (xs[1] & 3) << 4 | (xs[2] & 3) << 2 | (xs[3] & 3)) as u8;
        b[26] = ((xs[4] & 3) << 6 | (xs[5] & 3) << 4 | (xs[6] & 3) << 2 | (xs[7] & 3)) as u8;
        if flag {
            b[24] |= 0x04;
        }
        b[127] = 0;
        b[127] = 0u8.wrapping_sub(b.iter().fold(0u8, |s, &x| s.wrapping_add(x)));
        b
    }

    #[test]
    fn a_wide_monitor_is_converted_into_its_own_colours() {
        match decide(&facts(&[M27Q])) {
            Detected::Profile(p, Source::Edid) => assert_eq!(p.name, "M27Q"),
            other => panic!("{other:?}"),
        }
        assert_eq!(decide(&facts(&[M27Q])).caption(), "M27Q's own colours");
    }

    /// ⚠⚠ Never twice: whatever else is known, an OS that manages colour gets sRGB.
    #[test]
    fn the_os_managing_colour_wins_over_everything() {
        let mut f = facts(&[M27Q]);
        f.icc = Some((
            "calibrated".into(),
            export::icc_for(color::OutputSpace::DisplayP3).to_vec(),
        ));
        f.os_managed = true;
        assert_eq!(decide(&f), Detected::OsManaged);
        assert!(decide(&f).display().is_srgb());
    }

    #[test]
    fn an_assigned_profile_wins_over_the_edid() {
        let mut f = facts(&[M27Q]);
        f.icc = Some((
            "calibrated".into(),
            export::icc_for(color::OutputSpace::DisplayP3).to_vec(),
        ));
        assert!(matches!(decide(&f), Detected::Profile(p, Source::Icc) if p.name == "calibrated"));
        // An unreadable profile falls through to the EDID rather than to nothing.
        f.icc = Some(("broken".into(), vec![1, 2, 3]));
        assert!(matches!(decide(&f), Detected::Profile(_, Source::Edid)));
    }

    #[test]
    fn an_srgb_monitor_is_left_alone() {
        assert_eq!(
            decide(&facts(&[&srgb_edid(false)])),
            Detected::Srgb(Why::SrgbMonitor)
        );
        assert_eq!(
            decide(&facts(&[&srgb_edid(true)])),
            Detected::Srgb(Why::SrgbMonitor)
        );
        assert!(
            matches!(decide(&facts(&[M27Q])), Detected::Profile(..)),
            "positive control"
        );
    }

    #[test]
    fn several_different_monitors_fall_back_but_two_of_a_kind_do_not() {
        assert_eq!(
            decide(&facts(&[M27Q, &srgb_edid(false)])),
            Detected::Srgb(Why::SeveralMonitors)
        );
        assert!(matches!(
            decide(&facts(&[M27Q, M27Q])),
            Detected::Profile(..)
        ));
    }

    #[test]
    fn no_data_or_bad_data_is_srgb() {
        assert_eq!(decide(&Facts::default()), Detected::Srgb(Why::NoData));
        assert_eq!(
            decide(&facts(&[&[1, 2, 3]])),
            Detected::Srgb(Why::Unreadable)
        );
    }

    #[test]
    fn the_registry_key_comes_from_the_device_interface() {
        assert_eq!(
            edid_key(
                r"\\?\DISPLAY#GBT270D#5&3031ade7&0&UID4357#{e6f07b5f-ee97-4a90-b076-33f57bf4eaa7}"
            )
            .as_deref(),
            Some(
                r"SYSTEM\CurrentControlSet\Enum\DISPLAY\GBT270D\5&3031ade7&0&UID4357\Device Parameters"
            )
        );
        assert_eq!(edid_key("not an interface"), None);
    }

    /// Linux's `/sys/class/drm`, built by the test: only CONNECTED outputs with an EDID count, connectors in order.
    #[test]
    fn drm_lists_the_connected_monitors_edids() {
        let d = test_dir::TempDir::new("display-drm");
        let out = |name: &str, status: &str, edid: &[u8]| {
            let p = d.join(name);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("status"), status).unwrap();
            std::fs::write(p.join("edid"), edid).unwrap();
        };
        out("card0-DP-1", "connected\n", M27Q);
        out("card0-HDMI-A-1", "disconnected\n", M27Q);
        out("card0-eDP-1", "connected\n", b"");
        std::fs::create_dir_all(d.join("card0")).unwrap();
        let edids = drm_edids(d.path());
        assert_eq!(edids, vec![M27Q.to_vec()]);
        assert!(drm_edids(&d.join("absent")).is_empty());
    }
}
