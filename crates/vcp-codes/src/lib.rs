//! MCCS v2.2a VCP codes 常量表。
//!
//! 等价移植 `origin/src/vcp-codes.js`（170 行，纯常量，无逻辑）。
//! 数值逐字保留，不得"修正"。

pub const CODE_PAGE: u8 = 0x00;
pub const RESTORE_FACTORY_COLOR_DEFAULTS: u8 = 0x08;
pub const RESTORE_FACTORY_DEFAULTS: u8 = 0x04;
pub const RESTORE_FACTORY_GEOMETRY_DEFAULTS: u8 = 0x06;
pub const RESTORE_FACTORY_LUMINANCE_CONTRAST_DEFAULTS: u8 = 0x05;
pub const RESTORE_FACTORY_TV_DEFAULTS: u8 = 0x0a;
pub const SAVE_RESTORE_SETTINGS: u8 = 0xb0;

pub const SIX_AXIS_HUE_CONTROL_BLUE: u8 = 0x9f;
pub const SIX_AXIS_HUE_CONTROL_CYAN: u8 = 0x9e;
pub const SIX_AXIS_HUE_CONTROL_GREEN: u8 = 0x9d;
pub const SIX_AXIS_HUE_CONTROL_MAGENTA: u8 = 0xa0;
pub const SIX_AXIS_HUE_CONTROL_RED: u8 = 0x9b;
pub const SIX_AXIS_HUE_CONTROL_YELLOW: u8 = 0x9c;
pub const SIX_AXIS_SATURATION_CONTROL_BLUE: u8 = 0x5d;
pub const SIX_AXIS_SATURATION_CONTROL_CYAN: u8 = 0x5c;
pub const SIX_AXIS_SATURATION_CONTROL_GREEN: u8 = 0x5b;
pub const SIX_AXIS_SATURATION_CONTROL_MAGENTA: u8 = 0x5e;
pub const SIX_AXIS_SATURATION_CONTROL_RED: u8 = 0x59;
pub const SIX_AXIS_SATURATION_CONTROL_YELLOW: u8 = 0x5a;
pub const ADJUST_ZOOM: u8 = 0x7c;
pub const AUTO_COLOR_SETUP: u8 = 0x1f;
pub const AUTO_SETUP: u8 = 0x1e;
pub const AUTO_SETUP_ON_OFF: u8 = 0xa2;
pub const BACKLIGHT_CONTROL: u8 = 0x13;
pub const BACKLIGHT_LEVEL_BLUE: u8 = 0x71;
pub const BACKLIGHT_LEVEL_GREEN: u8 = 0x6f;
pub const BACKLIGHT_LEVEL_RED: u8 = 0x6d;
pub const BACKLIGHT_LEVEL_WHITE: u8 = 0x6b;
pub const BLOCK_LUT_OPERATION: u8 = 0x75;
pub const CLOCK: u8 = 0x0e;
pub const CLOCK_PHASE: u8 = 0x3e;
pub const COLOR_SATURATION: u8 = 0x8a;
pub const COLOR_TEMPERATURE_INCREMENT: u8 = 0x0b;
pub const COLOR_TEMPERATURE_REQUEST: u8 = 0x0c;
pub const CONTRAST: u8 = 0x12;
pub const DISPLAY_APPLICATION: u8 = 0xdc;
pub const FLESH_TONE_ENHANCEMENT: u8 = 0x11;
pub const FOCUS: u8 = 0x1c;
pub const GAMMA: u8 = 0x72;
pub const GRAY_SCALE_EXPANSION: u8 = 0x2e;
pub const HORIZONTAL_MOIRE: u8 = 0x56;
pub const HUE: u8 = 0x90;
pub const LUMINANCE: u8 = 0x10;
pub const LUT_SIZE: u8 = 0x73;
pub const SCREEN_ORIENTATION: u8 = 0xaa;
pub const SELECT_COLOR_PRESET: u8 = 0x14;
pub const SHARPNESS: u8 = 0x87;
pub const SINGLE_POINT_LUT_OPERATION: u8 = 0x74;
pub const STEREO_VIDEO_MODE: u8 = 0xd4;
pub const TV_BLACK_LEVEL_LUMINANCE: u8 = 0x92;
pub const TV_CONTRAST: u8 = 0x8e;
pub const TV_SHARPNESS: u8 = 0x8c;
pub const USER_COLOR_VISION_COMPENSATION: u8 = 0x17;
pub const VELOCITY_SCAN_MODULATION: u8 = 0x88;
pub const VERTICAL_MOIRE: u8 = 0x58;
pub const VIDEO_BLACK_LEVEL_BLUE: u8 = 0x70;
pub const VIDEO_BLACK_LEVEL_GREEN: u8 = 0x6e;
pub const VIDEO_BLACK_LEVEL_RED: u8 = 0x6c;
pub const VIDEO_GAIN_BLUE: u8 = 0x1a;
pub const VIDEO_GAIN_GREEN: u8 = 0x18;
pub const VIDEO_GAIN_RED: u8 = 0x16;
pub const WINDOW_BACKGROUND: u8 = 0x9a;
pub const WINDOW_CONTROL_ON_OFF: u8 = 0xa4;
pub const WINDOW_SELECT: u8 = 0xa5;
pub const WINDOW_SIZE: u8 = 0xa6;
pub const WINDOW_TRANSPARENCY: u8 = 0xa7;

pub const DISPLAY_CONTROLLER_ID: u8 = 0xc8;
pub const DISPLAY_FIRMWARE_LEVEL: u8 = 0xc9;
pub const DISPLAY_USAGE_TIME: u8 = 0xc6;
pub const HORIZONTAL_FREQUENCY: u8 = 0xac;
pub const IMAGE_MODE: u8 = 0xdb;
pub const OSD_BUTTON_LEVEL_CONTROL: u8 = 0xca;
pub const OSD_LANGUAGE: u8 = 0xcc;
pub const POWER_MODE: u8 = 0xd6;
pub const SOURCE_COLOR_CODING: u8 = 0xb5;
pub const SOURCE_TIMING_MODE: u8 = 0xb4;
pub const VERSION: u8 = 0xdf;
pub const VERTICAL_FREQUENCY: u8 = 0xae;

pub const BOTTOM_CORNER_FLARE: u8 = 0x4a;
pub const BOTTOM_CORNER_HOOK: u8 = 0x4c;
pub const DISPLAY_SCALING: u8 = 0x86;
pub const HORIZONTAL_CONVERGENCE_M_G: u8 = 0x29;
pub const HORIZONTAL_CONVERGENCE_R_B: u8 = 0x28;
pub const HORIZONTAL_KEYSTONE: u8 = 0x42;
pub const HORIZONTAL_LINEARITY: u8 = 0x2a;
pub const HORIZONTAL_LINEARITY_BALANCE: u8 = 0x2c;
pub const HORIZONTAL_MIRROR: u8 = 0x82;
pub const HORIZONTAL_PARALLELOGRAM: u8 = 0x40;
pub const HORIZONTAL_PINCUSHION: u8 = 0x24;
pub const HORIZONTAL_PINCUSHION_BALANCE: u8 = 0x26;
pub const HORIZONTAL_POSITION: u8 = 0x20;
pub const HORIZONTAL_SIZE: u8 = 0x22;
pub const ROTATION: u8 = 0x44;
pub const SCAN_MODE: u8 = 0xda;
pub const TOP_CORNER_FLARE: u8 = 0x46;
pub const TOP_CORNER_HOOK: u8 = 0x48;
pub const VERTICAL_CONVERGENCE_M_G: u8 = 0x39;
pub const VERTICAL_CONVERGENCE_R_B: u8 = 0x38;
pub const VERTICAL_KEYSTONE: u8 = 0x43;
pub const VERTICAL_LINEARITY: u8 = 0x3a;
pub const VERTICAL_LINEARITY_BALANCE: u8 = 0x3c;
pub const VERTICAL_MIRROR: u8 = 0x84;
pub const VERTICAL_PARALLELOGRAM: u8 = 0x41;
pub const VERTICAL_PINCUSHION: u8 = 0x34;
pub const VERTICAL_PINCUSHION_BALANCE: u8 = 0x36;
pub const VERTICAL_POSITION: u8 = 0x30;
pub const VERTICAL_SIZE: u8 = 0x32;
pub const WINDOW_POSITION_BR_X: u8 = 0x97;
pub const WINDOW_POSITION_BR_Y: u8 = 0x98;
pub const WINDOW_POSITION_TL_X: u8 = 0x95;
pub const WINDOW_POSITION_TL_Y: u8 = 0x96;

pub const ACTIVE_CONTROL: u8 = 0x52;
pub const AMBIENT_LIGHT_SENSOR: u8 = 0x66;
pub const APPLICATION_ENABLE_KEY: u8 = 0xc6;
pub const ASSET_TAG: u8 = 0xd2;
pub const AUXILIARY_DISPLAY_DATA: u8 = 0xcf;
pub const AUXILIARY_DISPLAY_SIZE: u8 = 0xce;
pub const AUXILIARY_POWER_OUTPUT: u8 = 0xd7;
pub const DEGAUSS: u8 = 0x01;
pub const DISPLAY_DESCRIPTOR_LENGTH: u8 = 0xc2;
pub const DISPLAY_IDENTIFICATION_DATA_OPERATION: u8 = 0x87;
pub const DISPLAY_TECHNOLOGY_TYPE: u8 = 0xb6;
pub const ENABLE_DISPLAY_OF_DISPLAY_DESCRIPTOR: u8 = 0xc4;
pub const FLAT_PANEL_SUB_PIXEL_LAYOUT: u8 = 0xb2;
pub const INPUT_SOURCE: u8 = 0x60;
pub const NEW_CONTROL_VALUE: u8 = 0x02;
pub const OUTPUT_SELECT: u8 = 0xd0;
pub const PERFORMANCE_PRESERVATION: u8 = 0x54;
pub const REMOTE_PROCEDURE_CALL: u8 = 0x76;
pub const SCRATCH_PAD: u8 = 0xde;
pub const SOFT_CONTROLS: u8 = 0x03;
pub const STATUS_INDICATORS: u8 = 0xcd;
pub const TRANSMIT_DISPLAY_DESCRIPTOR: u8 = 0xc3;
pub const TV_CHANNEL_UP_DOWN: u8 = 0x8b;

pub const AUDIO_BALANCE_L_R: u8 = 0x93;
pub const AUDIO_BASS: u8 = 0x91;
pub const AUDIO_JACK_CONNECTION_STATUS: u8 = 0x65;
pub const AUDIO_MICROPHONE_VOLUME: u8 = 0x64;
pub const AUDIO_MUTE: u8 = 0x8d;
pub const AUDIO_PROCESSOR_MODE: u8 = 0x94;
pub const AUDIO_SPEAKER_SELECT: u8 = 0x63;
pub const AUDIO_SPEAKER_VOLUME: u8 = 0x62;
pub const AUDIO_TREBLE: u8 = 0x8f;

pub const BODY_CRC_ERROR_COUNT: u8 = 0xbc;
pub const CLIENT_ID: u8 = 0xbd;
pub const HEADER_ERROR_COUNT: u8 = 0xbb;
pub const LINK_CONTROL: u8 = 0xbe;
pub const MONITOR_STATUS: u8 = 0xb7;
pub const MONITOR_X_ORIGIN: u8 = 0xb9;
pub const MONITOR_Y_ORIGIN: u8 = 0xba;
pub const PACKET_COUNT: u8 = 0xb8;

#[cfg(test)]
mod tests {
    #[test]
    fn key_codes() {
        // 亮度 0x10 / 对比度 0x12 / 背光 0x13 是 Monitors.js 依赖的核心码
        assert_eq!(super::LUMINANCE, 0x10);
        assert_eq!(super::CONTRAST, 0x12);
        assert_eq!(super::BACKLIGHT_CONTROL, 0x13);
        assert_eq!(super::INPUT_SOURCE, 0x60);
        assert_eq!(super::POWER_MODE, 0xd6);
    }
}
