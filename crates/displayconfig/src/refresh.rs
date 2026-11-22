//! 垂直刷新率上下文。
//!
//! equiv: index.js:L620-L757 VerticalRefreshRateContext。
//! 维护当前所有 inUse 显示器的几何区域 + 刷新率快照；按显示点求最小刷新率。

use std::sync::Mutex;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy)]
pub struct DisplayGeometry {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
    pub v_refresh_rate: f64,
}

pub struct VerticalRefreshRateContext {
    inner: OnceLock<Mutex<Vec<DisplayGeometry>>>,
}

impl Default for VerticalRefreshRateContext {
    fn default() -> Self {
        Self::new()
    }
}

impl VerticalRefreshRateContext {
    pub fn new() -> Self {
        let ctx = Self {
            inner: OnceLock::new(),
        };
        // 初始快照（等价原版构造时的首次 addDisplayChangeListener 回调）
        ctx.update_geometry();
        ctx
    }

    fn geometry_lock(&self) -> &Mutex<Vec<DisplayGeometry>> {
        self.inner.get_or_init(|| Mutex::new(Vec::new()))
    }

    /// equiv index.js computeDisplayGeometryFromConfig。
    /// 30Hz 是 vSyncFreq 输出损坏时的安全猜测（原注释），逐字保留。
    fn update_geometry(&self) {
        let mut geometry = Vec::new();
        match crate::extract::extract_display_config() {
            Ok(config) => {
                for entry in &config {
                    if !entry.in_use {
                        continue;
                    }
                    let width = entry.source_mode.width;
                    let height = entry.source_mode.height;
                    let tvsi = match &entry.target_video_signal_info {
                        Some(v) => v,
                        None => continue,
                    };
                    let (num, den) = (
                        tvsi.v_sync_freq.numerator,
                        tvsi.v_sync_freq.denominator,
                    );
                    let v_refresh_rate = if num == 0 || den == 0 {
                        30.0
                    } else {
                        num as f64 / den as f64
                    };
                    let top = entry.source_mode.position_y;
                    let bottom = entry.source_mode.position_y + height as i32;
                    let left = entry.source_mode.position_x;
                    let right = entry.source_mode.position_x + width as i32;
                    geometry.push(DisplayGeometry {
                        top,
                        bottom,
                        left,
                        right,
                        v_refresh_rate,
                    });
                }
            }
            Err(_) => {
                geometry = Vec::new();
            }
        }
        let mut guard = self.geometry_lock().lock().unwrap();
        *guard = geometry;
    }

    /// equiv index.js findVerticalRefreshRateForDisplayPoint：
    /// 重叠区域取最小刷新率；越界返回 None。
    pub fn find_vertical_refresh_rate_for_display_point(&self, x: i32, y: i32) -> Option<f64> {
        self.update_geometry();
        let guard = self.geometry_lock().lock().unwrap();
        let mut ret: Option<f64> = None;
        for g in guard.iter() {
            if g.left <= x && x < g.right && g.top <= y && y < g.bottom {
                ret = Some(match ret {
                    None => g.v_refresh_rate,
                    Some(prev) => prev.min(g.v_refresh_rate),
                });
            }
        }
        ret
    }

    /// 等价 close()：清空几何缓存。
    pub fn close(&self) {
        let mut guard = self.geometry_lock().lock().unwrap();
        guard.clear();
    }
}
