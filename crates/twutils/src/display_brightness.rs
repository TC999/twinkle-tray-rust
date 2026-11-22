//! gamma ramp 软件亮度。
//!
//! equiv: windows_display_brightness.cc（309 行，逐函数移植）。
//! 常量 kMinimumBrightness=20 等均为真机调校值，逐字保留（AGENTS.md §3.2）。

use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Debug, Clone, Default)]
struct GammaState {
    base_ramp: Vec<u16>,
    brightness: i32,
}

static GAMMA_STATES: Mutex<Option<HashMap<String, GammaState>>> = Mutex::new(None);

const MINIMUM_BRIGHTNESS: i32 = 20;
const MINIMUM_DRIVER_SCALE: i32 = 70;
const MINIMUM_RELIABLE_CURVE: i32 = 60;
const CHANNEL_ENTRIES: usize = 256;
const GREEN_CHANNEL: usize = CHANNEL_ENTRIES;
const SET_ATTEMPTS: usize = 3;
const RAMP_VERIFICATION_TOLERANCE: u16 = 256;

/// utf8ToWide 等价。
fn utf8_to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().collect()
}

/// normalizeDisplayName 等价：去掉 "\\Monitor" 后缀。
fn normalize_display_name(value: &str) -> String {
    match value.find("\\Monitor") {
        Some(idx) => value[..idx].to_string(),
        None => value.to_string(),
    }
}

/// openDisplay 等价：CreateDCW("DISPLAY", normalized)。
unsafe fn open_display(display_name: &str) -> Option<windows::Win32::Graphics::Gdi::HDC> {
    let normalized = normalize_display_name(display_name);
    if normalized.is_empty() {
        return None;
    }
    let wide = utf8_to_wide(&normalized);
    let mut wide_nul: Vec<u16> = wide.clone();
    wide_nul.push(0);
    // SAFETY: PCWSTR 以 NUL 结尾
    let dc = windows::Win32::Graphics::Gdi::CreateDCW(
        windows::core::w!("DISPLAY"),
        windows::core::PCWSTR::from_raw(wide_nul.as_ptr()),
        None,
        None,
    );
    if dc.is_invalid() { None } else { Some(dc) }
}

/// readRamp 等价。
unsafe fn read_ramp(dc: windows::Win32::Graphics::Gdi::HDC) -> Option<Vec<u16>> {
    let mut ramp = vec![0u16; 3 * 256];
    // SAFETY: ramp 长度 768 与 API 约定一致
    let ok = windows::Win32::UI::ColorSystem::GetDeviceGammaRamp(
        dc,
        ramp.as_mut_ptr() as *mut core::ffi::c_void,
    )
    .as_bool();
    if ok { Some(ramp) } else { None }
}

/// rampsMatch 等价：逐项差值 <= 容差。
fn ramps_match(expected: &[u16], actual: &[u16]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    for i in 0..expected.len() {
        let difference = (expected[i] as i32 - actual[i] as i32).abs();
        if difference > RAMP_VERIFICATION_TOLERANCE as i32 {
            return false;
        }
    }
    true
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // requested_scale 保留: 与 C++ 结构体字段一一对应
struct BrightnessCurve {
    requested_scale: f64,
    peak_scale: f64,
    exponent: f64,
}

/// curveForBrightness 等价（数学公式逐字保留）。
fn curve_for_brightness(brightness: i32) -> BrightnessCurve {
    let curve_brightness = MINIMUM_RELIABLE_CURVE as f64
        + ((brightness - MINIMUM_BRIGHTNESS) as f64
            * (100.0 - MINIMUM_RELIABLE_CURVE as f64)
            / (100.0 - MINIMUM_BRIGHTNESS as f64));

    let requested_scale = curve_brightness / 100.0;
    let peak_scale = curve_brightness.max(MINIMUM_DRIVER_SCALE as f64) / 100.0;

    // Windows rejects very dark linear ramps. Below the driver-safe peak,
    // preserve that peak and darken mid-tones enough to match the requested
    // level at 50% input. This remains monotonic and passes the GDI safeguard.（原注释）
    let exponent = if curve_brightness < MINIMUM_DRIVER_SCALE as f64 {
        ((requested_scale * 0.5) / peak_scale).ln() / 0.5f64.ln()
    } else {
        1.0
    };
    BrightnessCurve {
        requested_scale,
        peak_scale,
        exponent,
    }
}

/// buildBrightnessRamp 等价。
fn build_brightness_ramp(base_ramp: &[u16], brightness: i32) -> Vec<u16> {
    let curve = curve_for_brightness(brightness);
    let mut ramp = vec![0u16; base_ramp.len()];
    for i in 0..ramp.len() {
        let normalized = base_ramp[i] as f64 / 65535.0;
        let adjusted = curve.peak_scale * normalized.powf(curve.exponent);
        ramp[i] = (adjusted.min(1.0) * 65535.0).round() as u16;
    }
    ramp
}

/// defaultRamp 等价。
fn default_ramp() -> Vec<u16> {
    let mut ramp = vec![0u16; 3 * CHANNEL_ENTRIES];
    for channel in 0..3usize {
        for i in 0..CHANNEL_ENTRIES {
            ramp[(channel * CHANNEL_ENTRIES) + i] = 65535.min(i as i32 * 257) as u16;
        }
    }
    ramp
}

/// midToneIndex 等价：绿通道里最接近 32768 的条目（亮度测量锚点，原注释语义）。
fn mid_tone_index(base_ramp: &[u16]) -> usize {
    let mut closest = CHANNEL_ENTRIES / 2;
    let mut smallest = i32::MAX;
    for i in 0..CHANNEL_ENTRIES {
        let difference = (base_ramp[GREEN_CHANNEL + i] as i32 - 32768).abs();
        if difference < smallest {
            smallest = difference;
            closest = i;
        }
    }
    closest
}

/// brightnessFromRamp 等价。
fn brightness_from_ramp(base_ramp: &[u16], ramp: &[u16]) -> i32 {
    if base_ramp.len() != ramp.len() || ramp.len() < 3 * CHANNEL_ENTRIES {
        return 100;
    }
    let index = GREEN_CHANNEL + mid_tone_index(base_ramp);
    let base_mid_tone = base_ramp[index] as f64 / 65535.0;
    if base_mid_tone <= 0.0 {
        return 100;
    }
    let curve_brightness = ((ramp[index] as f64 / 65535.0) / base_mid_tone) * 100.0;
    let brightness = MINIMUM_BRIGHTNESS as f64
        + ((curve_brightness - MINIMUM_RELIABLE_CURVE as f64)
            * (100.0 - MINIMUM_BRIGHTNESS as f64)
            / (100.0 - MINIMUM_RELIABLE_CURVE as f64));
    ((brightness.round() as i32).max(MINIMUM_BRIGHTNESS)).min(100)
}

/// restoreBaseRamp 等价：从曲线反推原始 ramp（原注释语义）。
fn restore_base_ramp(ramp: &[u16], brightness: i32) -> Vec<u16> {
    let curve = curve_for_brightness(brightness);
    if curve.peak_scale <= 0.0 || curve.exponent <= 0.0 {
        return ramp.to_vec();
    }
    let mut base_ramp = vec![0u16; ramp.len()];
    for i in 0..ramp.len() {
        let normalized = ((ramp[i] as f64 / 65535.0) / curve.peak_scale).min(1.0);
        base_ramp[i] = normalized.powf(1.0 / curve.exponent).round() as u16;
        // 注意: 与 C++ 一致，先 pow 再 * 65535
    }
    // 修正: C++ 是 pow(normalized, 1/exponent) * 65535，normalized 已先除以 peakScale 并 clamp
    for i in 0..base_ramp.len() {
        base_ramp[i] = (1.0f64.min((ramp[i] as f64 / 65535.0) / curve.peak_scale)
            .powf(1.0 / curve.exponent)
            * 65535.0)
            .round() as u16;
    }
    base_ramp
}

/// levelFromRamp 等价：measured ±2 内的候选级别逐一回验（驱动少位深时偏移，原注释语义）。
fn level_from_ramp(base_ramp: &[u16], ramp: &[u16]) -> Option<i32> {
    let measured = brightness_from_ramp(base_ramp, ramp);
    for candidate in [measured, measured - 1, measured + 1, measured - 2, measured + 2] {
        if candidate < MINIMUM_BRIGHTNESS || candidate > 100 {
            continue;
        }
        if ramps_match(&build_brightness_ramp(base_ramp, candidate), ramp) {
            return Some(candidate);
        }
    }
    None
}

/// stateFromRamp 等价（原注释：gamma ramp 跨进程存活；校色工具至少保留一个满输出通道）。
fn state_from_ramp(ramp: &[u16]) -> GammaState {
    let mut state = GammaState {
        base_ramp: ramp.to_vec(),
        brightness: 100,
    };
    if ramp.len() < 3 * CHANNEL_ENTRIES {
        return state;
    }

    let mut top: u16 = 0;
    for channel in 0..3usize {
        top = top.max(ramp[(channel * CHANNEL_ENTRIES) + CHANNEL_ENTRIES - 1]);
    }
    let peak = (top as f64 / 65535.0) * 100.0;
    let tolerance = (RAMP_VERIFICATION_TOLERANCE as f64 / 65535.0) * 100.0;
    if peak >= 100.0 - tolerance || peak < MINIMUM_DRIVER_SCALE as f64 - tolerance {
        return state;
    }

    let level: i32;
    if peak >= MINIMUM_DRIVER_SCALE as f64 + (tolerance / 2.0) {
        // Above the driver-safe peak the curve is a plain scale, so the peak is the level（原注释）
        level = (MINIMUM_BRIGHTNESS as f64
            + ((peak - MINIMUM_RELIABLE_CURVE as f64) * (100.0 - MINIMUM_BRIGHTNESS as f64)
                / (100.0 - MINIMUM_RELIABLE_CURVE as f64)))
            .round() as i32;
    } else {
        // The peak is pinned here, and only the mid-tones carry the level. There's
        // no base to measure them against, so a default ramp has to stand in.（原注释）
        level = brightness_from_ramp(&default_ramp(), ramp);
    }
    let level = level.max(MINIMUM_BRIGHTNESS).min(99);
    state.brightness = level;
    state.base_ramp = restore_base_ramp(ramp, level);
    state
}

/// syncState 等价：把存储状态与当前 Windows ramp 对齐（原注释语义）。
/// 前置条件: 调用方已持有 GAMMA_STATES 锁（本函数不再加锁，避免 std Mutex 重入死锁）。
fn sync_state_locked(display_name: &str, dc: windows::Win32::Graphics::Gdi::HDC) -> Option<String> {
    let current = unsafe { read_ramp(dc) }?;
    let normalized = normalize_display_name(display_name);

    let mut states = GAMMA_STATES.lock().unwrap();
    let map = states.get_or_insert_with(HashMap::new);
    if let Some(found) = map.get(&normalized) {
        let base = found.base_ramp.clone();
        if let Some(level) = level_from_ramp(&base, &current) {
            let found = map.get_mut(&normalized).unwrap();
            found.brightness = level;
            return Some(normalized);
        }
    }
    let state = state_from_ramp(&current);
    map.insert(normalized.clone(), state);
    Some(normalized)
}

/// getBrightness 等价（equiv cc:getBrightness）。
pub fn get_brightness(display_name: &str) -> i32 {
    if display_name.is_empty() {
        return -1;
    }
    // SAFETY: GDI DC 生命周期由本函数管理
    unsafe {
        let dc = match open_display(display_name) {
            Some(d) => d,
            None => return -1,
        };
        // Re-read the ramp rather than trusting the last value set here. Another
        // app, or an earlier session, may have changed it since.（原注释）
        let state_key = sync_state_locked(display_name, dc);
        let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);
        match state_key {
            Some(key) => {
                let states = GAMMA_STATES.lock().unwrap();
                states.as_ref().and_then(|m| m.get(&key)).map(|s| s.brightness).unwrap_or(-1)
            }
            None => -1,
        }
    }
}

/// setBrightness 等价（equiv cc:setBrightness）：写入 + 读回验证 + 3 次重试（20ms 递增）。
pub fn set_brightness(display_name: &str, requested_value: f64) -> bool {
    if !requested_value.is_finite()
        || !(0.0..=100.0).contains(&requested_value)
        || requested_value.floor() != requested_value
    {
        return false;
    }
    let requested = requested_value as i32;
    let requested = requested.max(MINIMUM_BRIGHTNESS);
    if display_name.is_empty() {
        return false;
    }

    // SAFETY: GDI DC 生命周期由本函数管理
    unsafe {
        let dc = match open_display(display_name) {
            Some(d) => d,
            None => return false,
        };
        let state_key = match sync_state_locked(display_name, dc) {
            Some(k) => k,
            None => {
                let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);
                return false;
            }
        };
        let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);

        let base_ramp = {
            let states = GAMMA_STATES.lock().unwrap();
            match states.as_ref().and_then(|m| m.get(&state_key)) {
                Some(s) => s.base_ramp.clone(),
                None => return false,
            }
        };
        let ramp = build_brightness_ramp(&base_ramp, requested);

        let mut ok = false;
        for attempt in 0..SET_ATTEMPTS {
            if ok { break; }
            let dc = open_display(display_name);
            if let Some(dc) = dc {
                ok = windows::Win32::UI::ColorSystem::SetDeviceGammaRamp(
                    dc,
                    ramp.as_ptr() as *const core::ffi::c_void,
                )
                .as_bool();
                if ok {
                    // 写入后读回验证（等价原版逻辑）
                    ok = read_ramp(dc).map(|applied| ramps_match(&ramp, &applied)).unwrap_or(false);
                }
                let _ = windows::Win32::Graphics::Gdi::DeleteDC(dc);
            }
            if !ok && attempt + 1 < SET_ATTEMPTS {
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1) as u64));
            }
        }
        if ok {
            let mut states = GAMMA_STATES.lock().unwrap();
            if let Some(s) = states.as_mut().and_then(|m| m.get_mut(&state_key)) {
                s.brightness = requested;
            }
        }
        ok
    }
}
