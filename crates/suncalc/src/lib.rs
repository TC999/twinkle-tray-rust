//! suncalc 等价移植(太阳时间部分)。
//!
//! equiv: origin/node_modules/suncalc/suncalc.js(v1.9.0,纯数学库)。
//! AGENTS.md §1.2:suncalc 为 ~200 行纯数学,按原 JS 逐函数移植。
//!
//! 移植范围论证:Twinkle Tray 仅通过 getSunCalcTime/getSunCalcTimes 使用
//! SunCalc.getTimes(electron.js:L5733-L5747)。月亮(getMoonPosition/
//! getMoonIllumination*)与太阳方位(getPosition/azimuth/altitude/
//! astroRefraction)以及 addTime 在本仓库调用链中从未出现,不移植;
//! 若后续需要,按同法补充即可。

#![allow(non_snake_case)]

/// rad = PI / 180(equiv suncalc.js:L16)
const RAD: f64 = core::f64::consts::PI / 180.0;
const PI: f64 = core::f64::consts::PI;

// date/time constants and conversions(equiv L25-L27)
const DAY_MS: f64 = 1000.0 * 60.0 * 60.0 * 24.0;
const J1970: f64 = 2440588.0;
const J2000: f64 = 2451545.0;

/// toJulian(date)(equiv L29):输入 Unix 毫秒,返回儒略日。
pub fn to_julian(date_ms: f64) -> f64 {
    date_ms / DAY_MS - 0.5 + J1970
}

/// fromJulian(j)(equiv L30):返回 Unix 毫秒。
pub fn from_julian(j: f64) -> f64 {
    (j + 0.5 - J1970) * DAY_MS
}

/// toDays(date)(equiv L31)。
pub fn to_days(date_ms: f64) -> f64 {
    to_julian(date_ms) - J2000
}

/// e = rad * 23.4397:地球轨道倾角(equiv L36)。
const E: f64 = RAD * 23.4397;

/// rightAscension(l, b)(equiv L38)。
pub fn right_ascension(l: f64, b: f64) -> f64 {
    (l.sin() * E.cos() - b.tan() * E.sin()).atan2(l.cos())
}

/// declination(l, b)(equiv L39)。
pub fn declination(l: f64, b: f64) -> f64 {
    (b.sin() * E.cos() + b.cos() * E.sin() * l.sin()).asin()
}

/// siderealTime(d, lw)(equiv L44)。
pub fn sidereal_time(d: f64, lw: f64) -> f64 {
    RAD * (280.16 + 360.9856235 * d) - lw
}

/// solarMeanAnomaly(d)(equiv L57)。
pub fn solar_mean_anomaly(d: f64) -> f64 {
    RAD * (357.5291 + 0.98560028 * d)
}

/// eclipticLongitude(M)(equiv L59-L65)。
pub fn ecliptic_longitude(m: f64) -> f64 {
    // equation of center + perihelion of the Earth
    let c = RAD * (1.9148 * m.sin() + 0.02 * (2.0 * m).sin() + 0.0003 * (3.0 * m).sin());
    let p = RAD * 102.9372;
    m + c + p + PI
}

/// sunCoords(d) 的 (dec, ra)(equiv L67-L76)。
pub fn sun_coords(d: f64) -> (f64, f64) {
    let m = solar_mean_anomaly(d);
    let l = ecliptic_longitude(m);
    (declination(l, 0.0), right_ascension(l, 0.0))
}

/// sun times configuration(angle, morning name, evening name)(equiv L102-L109)。
/// Rust 侧以静态表保序;名称与 JS 的 result 键一一对应。
pub const TIMES: [(f64, &str, &str); 6] = [
    (-0.833, "sunrise", "sunset"),
    (-0.3, "sunriseEnd", "sunsetStart"),
    (-6.0, "dawn", "dusk"),
    (-12.0, "nauticalDawn", "nauticalDusk"),
    (-18.0, "nightEnd", "night"),
    (6.0, "goldenHourEnd", "goldenHour"),
];

const J0: f64 = 0.0009;

/// julianCycle(d, lw)(equiv L122)。
pub fn julian_cycle(d: f64, lw: f64) -> f64 {
    (d - J0 - lw / (2.0 * PI)).round()
}

/// approxTransit(Ht, lw, n)(equiv L124)。
pub fn approx_transit(ht: f64, lw: f64, n: f64) -> f64 {
    J0 + (ht + lw) / (2.0 * PI) + n
}

/// solarTransitJ(ds, M, L)(equiv L125)。
pub fn solar_transit_j(ds: f64, m: f64, l: f64) -> f64 {
    J2000 + ds + 0.0053 * m.sin() - 0.0069 * (2.0 * l).sin()
}

/// hourAngle(h, phi, d)(equiv L127)。
pub fn hour_angle(h: f64, phi: f64, d: f64) -> f64 {
    ((h.sin() - phi.sin() * d.sin()) / (phi.cos() * d.cos())).acos()
}

/// observerAngle(height)(equiv L128)。
pub fn observer_angle(height: f64) -> f64 {
    -2.076 * height.sqrt() / 60.0
}

/// getSetJ(h, lw, phi, dec, n, M, L):给定太阳高度角的落下时刻(equiv L131-L136)。
pub fn get_set_j(h: f64, lw: f64, phi: f64, dec: f64, n: f64, m: f64, l: f64) -> f64 {
    let w = hour_angle(h, phi, dec);
    let a = approx_transit(w, lw, n);
    solar_transit_j(a, m, l)
}

/// getTimes 的结果:时间名 -> Unix 毫秒(UTC)。
/// JS 版返回 Date 对象,这里等价为毫秒数(调用方再按本地时区取 H:M)。
pub type SunTimes = std::collections::BTreeMap<&'static str, f64>;

/// SunCalc.getTimes(date, lat, lng, height)(equiv L142-L181)。
/// date 为 Unix 毫秒;height 为观察者高度(米,默认 0)。
pub fn get_times(date_ms: f64, lat: f64, lng: f64, height: f64) -> SunTimes {
    let height = if height == 0.0 { 0.0 } else { height };

    let lw = RAD * -lng;
    let phi = RAD * lat;

    let dh = observer_angle(height);

    let d = to_days(date_ms);
    let n = julian_cycle(d, lw);
    let ds = approx_transit(0.0, lw, n);

    let m = solar_mean_anomaly(ds);
    let l = ecliptic_longitude(m);
    let dec = declination(l, 0.0);

    let jnoon = solar_transit_j(ds, m, l);

    let mut result: SunTimes = std::collections::BTreeMap::new();
    result.insert("solarNoon", from_julian(jnoon));
    result.insert("nadir", from_julian(jnoon - 0.5));

    for time in TIMES.iter() {
        let h0 = (time.0 + dh) * RAD;

        let jset = get_set_j(h0, lw, phi, dec, n, m, l);
        let jrise = jnoon - (jset - jnoon);

        result.insert(time.1, from_julian(jrise));
        result.insert(time.2, from_julian(jset));
    }

    result
}

/// getSunCalcTime 的本地时间换算(equiv electron.js:L5733-L5738):
/// 把 UTC 毫秒加上时区偏移后取当天的 (小时, 分钟)。
/// JS 侧 Date.getHours() 基于本地时区;此处用 offset_minutes 显式表达,
/// 运行时偏移由 system_events 侧经 Win32 取得。
pub fn ms_to_local_hm(ms_utc: f64, offset_minutes: i64) -> (i64, i64) {
    let local_ms = ms_utc + (offset_minutes as f64) * 60_000.0;
    // Unix epoch 毫秒对 86400000 取模即当天毫秒数(时区偏移已并入)
    let day_ms = local_ms.rem_euclid(86_400_000.0);
    let hours = (day_ms / 3_600_000.0).floor() as i64;
    let minutes = ((day_ms / 60_000.0).floor() as i64) % 60;
    (hours, minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// JS 对拍基线(node target/suncalc-reference.js 同输入输出)。
    /// 输入:2026-11-23T04:00:00Z(= 北京时间 12:00),lat 39.9042, lng 116.4074。
    const T0: f64 = 1_794_412_800_000.0; // 2026-11-23T04:00:00Z

    fn hm(t: &SunTimes, name: &str, offset: i64) -> String {
        let (h, m) = ms_to_local_hm(*t.get(name).unwrap(), offset);
        format!("{}:{}", h, m)
    }

    #[test]
    fn julian_roundtrip() {
        let j = to_julian(T0);
        assert!((from_julian(j) - T0).abs() < 1.0);
    }

    #[test]
    fn beijing_times_match_node_baseline() {
        // node 基线(东八区 +480,2026-11-23T04:00:00Z):
        // dawn 6:26 / sunrise 6:55 / solarNoon 11:59 / goldenHour 16:24 /
        // sunsetStart 17:1 / sunset 17:4 / dusk 17:32 / night 18:36
        let times = get_times(T0, 39.9042, 116.4074, 0.0);
        assert_eq!(hm(&times, "dawn", 480), "6:26");
        assert_eq!(hm(&times, "sunrise", 480), "6:55");
        assert_eq!(hm(&times, "solarNoon", 480), "11:59");
        assert_eq!(hm(&times, "goldenHour", 480), "16:24");
        assert_eq!(hm(&times, "sunsetStart", 480), "17:1");
        assert_eq!(hm(&times, "sunset", 480), "17:4");
        assert_eq!(hm(&times, "dusk", 480), "17:32");
        assert_eq!(hm(&times, "night", 480), "18:36");
    }

    #[test]
    fn negative_longitude_wraps_to_next_day() {
        // 西半球(纽约 40.7128, -74.0060):日落在当地 16:29,
        // 但 T0 当天纽约仍在日落前的白天,sunset 落在 UTC 次日,本地时钟回绕
        // 到前一日 16:29 —— node 基线:sunrise 19:39 / sunset 5:43(本机 +480)。
        // 该回绕正是 electron.js getSunCalcTime 用 Date.getHours() 时同样存在的
        // 原始行为(跨日 sunCalc 时间会被 parseTime 按字面 H:M 解析),逐字保留。
        let times = get_times(T0, 40.7128, -74.0060, 0.0);
        assert_eq!(hm(&times, "sunrise", 480), "19:39");
        assert_eq!(hm(&times, "sunset", 480), "5:43");
    }
}
