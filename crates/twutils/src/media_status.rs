//! 媒体播放状态（WinRT GlobalSystemMediaTransportControls）。
//!
//! equiv: windows_media_status.cc（153 行）。原版 100ms 超时语义逐字保留。

use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

/// getPlaybackStatus 等价：返回 "closed"/"opened"/"changing"/"stopped"/"playing"/"paused"。
/// 100ms 未完成即返回 "closed"（原版 wait_for(100ms) 语义）。
pub fn get_playback_status() -> &'static str {
    let status_str = "closed";

    {
        let request = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
            Ok(r) => r,
            Err(_) => return status_str,
        };
        // 等价 wait_for(100ms)：超时返回 closed
        if !crate::wait_with_timeout(&request, std::time::Duration::from_millis(100)) {
            return status_str;
        }
        let manager = match request.GetResults() {
            Ok(m) => m,
            Err(_) => return status_str,
        };

        let session = match manager.GetCurrentSession() {
            Ok(s) => s,
            Err(_) => return status_str,
        };

        let info = match session.GetPlaybackInfo() {
            Ok(i) => i,
            Err(_) => return status_str,
        };
        let status = match info.PlaybackStatus() {
            Ok(s) => s,
            Err(_) => return status_str,
        };

        match status {
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Opened => "opened",
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Changing => "changing",
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped => "stopped",
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing => "playing",
            GlobalSystemMediaTransportControlsSessionPlaybackStatus::Paused => "paused",
            _ => "closed",
        }
    }
}

/// getPlaybackInfo 等价的元数据结构。
#[derive(Debug, Clone, Default)]
pub struct PlaybackInfo {
    pub title: String,
    pub subtitle: String,
    pub artist: String,
    pub album: String,
    pub albumartist: String,
    pub source: String,
    pub tracks: u32,
    pub tracknumber: u32,
    /// "unknown"/"music"/"video"/"image"
    pub media_type: String,
}

/// getPlaybackInfo 等价。
pub fn get_playback_info() -> Option<PlaybackInfo> {
    {
        let request = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok()?;
        if !crate::wait_with_timeout(&request, std::time::Duration::from_millis(100)) {
            return None;
        }
        let manager = request.GetResults().ok()?;
        let session = manager.GetCurrentSession().ok()?;

        let properties_request = session.TryGetMediaPropertiesAsync().ok()?;
        if !crate::wait_with_timeout(&properties_request, std::time::Duration::from_millis(100)) {
            return None;
        }
        let playback = properties_request.GetResults().ok()?;

        let type_str = match playback.PlaybackType() {
            Ok(t) => match t.Value() {
                Ok(windows::Media::MediaPlaybackType::Music) => "music",
                Ok(windows::Media::MediaPlaybackType::Video) => "video",
                Ok(windows::Media::MediaPlaybackType::Image) => "image",
                _ => "unknown",
            },
            Err(_) => "unknown",
        };

        Some(PlaybackInfo {
            title: playback.Title().map(|s| s.to_string()).unwrap_or_default(),
            subtitle: playback.Subtitle().map(|s| s.to_string()).unwrap_or_default(),
            artist: playback.Artist().map(|s| s.to_string()).unwrap_or_default(),
            album: playback.AlbumTitle().map(|s| s.to_string()).unwrap_or_default(),
            albumartist: playback.AlbumArtist().map(|s| s.to_string()).unwrap_or_default(),
            source: session.SourceAppUserModelId().map(|s| s.to_string()).unwrap_or_default(),
            tracks: playback.AlbumTrackCount().unwrap_or(0).max(0) as u32,
            tracknumber: playback.TrackNumber().unwrap_or(0).max(0) as u32,
            media_type: type_str.to_string(),
        })
    }
}
