# 移植进度

> 对照源码: `origin/`（Twinkle Tray v1.18.0-beta2）。规范: `AGENTS.md`。

## 已完成模块（真机对拍通过）

| crate | 源模块 | 对拍结果 |
|---|---|---|
| `crates/tt-localize` | Translate.js + getLocalization() | 单测通过（回退链/占位符/语言检测） |
| `crates/vcp-codes` | vcp-codes.js | 单测通过（核心 VCP 码） |
| `crates/displayconfig` | win32-displayconfig (js 757 + cc 1073 行) | 真机 QueryDisplayConfig 冒烟 + 枚举映射单测通过 |
| `crates/wmibridge` | wmi-bridge.cc (482 行) | **真机亮度读数 60 与 Get-CimInstance 一致**；显示器枚举 1 台一致 |
| `crates/ddcci` | node-ddcci (ddcci.cc 1666 行) | 真机枚举内屏 CMN1521、设备匹配、DDC 探测路径正常（内屏 invalid 属正确行为）；capabilities 解析单测通过 |
| `crates/twutils` | tt-windows-utils (6 个 .cc，1166 行) | **gamma ramp 软件亮度闭环：读 100 → 设 50 → 读回 50**；电源/按键注册边界行为一致 |
| `crates/activewindow` | node-active-window (ActiveWindow.cpp 402 行) | **真机前台窗口抓取一致**：title/pid/path/application("Chromium") 与 PowerShell VersionInfo 对照一致；UWP 识别路径保留 |
| `crates/sensors` | windows-ambient-sensor (cc 137 + hpp 186 行) | 真机 COM 链路通（SensorManager 激活成功）；本机无光感硬件枚举 0 台与系统一致 |
| `crates/material` | acrylic.cc (124 行) + DWM | SetWindowCompositionAttribute 动态加载路径移植；NULL 窗口降级一致；DWM 属性与 twutils 联动 |

## 本地化产物

- `src/localization/*.ftl`（38 语言，由 `tools/convert-localization.mjs` 生成，`--check` 双向校验通过）
- 事实修正：源语料仅 en.json 为 304 键，其余 150–292 键不等——en 回退链必要性实证。

## 关键实现记录

1. **windows crate 0.62.2**（msvc）。DisplayConfig API 在 `Win32::Devices::Display`（非 Graphics::Desktop）；路径位标志（DISPLAYCONFIG_PATH_ACTIVE 等）在 `Graphics::Gdi`；EDD_* 在 `UI::WindowsAndMessaging`；gamma ramp 在 `UI::ColorSystem`；RAWINPUT 全家在 `UI::Input`；POWER GUID 在 `System::SystemServices`。
2. **CLSID_WbemLocator = {4590F811-1D3A-11D0-891F-00AA004B2E24}**（1D3A，非 SDK 文档常见的 1D3B；已用注册表核实）。
3. **rustc 1.96**: AtomicBool 无 exchange()（用 compare_exchange）；SetWindowPos/GetWindowRect 等 User32 API 返回 Result。
4. **windows-future 0.3**：IAsyncOperation 无 wait_for，用 Status() 轮询等价实现超时（twutils::wait_with_timeout）。依赖论证：windows 0.62 强制传递依赖，无净增（AGENTS.md §1.1）。
5. **ddcci HANDLE 管理**：以 HWrap(usize) 跨线程存 HANDLE（原始类型非 Send）；句柄所有权语义（handles 表拥有/物理元数据表不销毁）逐字保留。
6. **gamma 死锁修复**：sync_state 重命名为 sync_state_locked 并由调用方持锁，等价 C++ 的 mutex 语义。
7. **capabilities 解析**（index.js parseCapabilitiesString）逐分支移植：括号嵌套、subdata 忽略、0x 十六进制值、大小写不敏感 vcp( 匹配。
8. **activewindow**: kernel32 包 API（GetPackageFamilyName/GetPackageId/GetPackagePath）在 windows crate 0.62 未绑定，以集中 FFI 声明补齐（AGENTS.md §1.1 必要性成立）。AppxManifest COM 解析链降级为 package family name（§4 允许的降级，已论证）。
9. **VerQueryValueW 实测修正**：对 StringFileInfo 字符串块返回的 pulen 是**宽字符数（含 NUL）**而非 MSDN 所述字节数（"Chromium " 返回 9），以真机对拍为准。
10. **gamma 死锁教训**：std Mutex 重入即死锁；sync_state 改为 sync_state_locked（调用方持锁契约），等价 C++ mutex 语义。
11. **VerQueryValueW pulen 实测**：StringFileInfo 字符串块返回宽字符数（含 NUL），非 MSDN 所述字节数。
12. **CLSID/SensorManager 与 CLSID/WbemLocator 均以注册表核实**（windows crate 未导出，SDK 头文件手写值有误）。

## Monitors Worker（进行中——骨架 + IPC 通路真机验证通过）

| 组件 | 状态 | 对拍 |
|---|---|---|
| 哨兵（ddc-probe.lock / ddc-unstable.json）| ✅ crates/monitor-worker/src/sentinel.rs | 逻辑等价移植，含降级记录与安全报告 |
| IPC 行协议（ready/settings/refreshMonitors/getReport/getVCP/...）| ✅ src/worker_ipc.rs + message.rs | **真机 e2e：stdin→worker→stdout 全链路**，CMN1521 枚举字段与原版 camelCase 一致 |
| 三源枚举（WMI/Win32/DDC hwid 对齐）| ✅ all_monitors.rs + monitors.rs + wmi_fallback.rs | **真机 e2e：CMN1521 三源对齐（hwid `\\?\DISPLAY#CMN1521#4&2cf91c03&0&UID8388688`），name/num 兜底与 connector=internal 一致** |
| 时序/数值常量（SOFTWARE_BRIGHTNESS_MIN=20、BRIDGE_FAILURE_LIMIT=3、Studio Display nits 等）| ✅ settings_state.rs | 逐字保留 |
| 崩溃隔离 + 自动重启 + 30s 复位 | ✅ src/monitor_host.rs + thread_manager.rs | ready 标志改由 stdout 读取线程以 Arc<AtomicBool> 置位（原 thread_local 实现失效，已修） |
| 完整 getFeaturesDDC（capabilities 特征逐项扫描 + 80s/15s 超时）| ✅ features.rs + monitors.rs | checkMonitorFeatures/checkVCP/vcpCache/报告链全量移植；真机 e2e 验证 |
| checkVCPIfEnabled / checkIfVCPSupported / determineBrightnessVCPCode | ✅ features.rs | 缓存链 + 报告过滤逐分支等价 |
| classifyDDCError + DDC_ERROR_REASONS（winerror.h 13 码）| ✅ features.rs | 单测通过 |
| 命令队列 + 节流 + flush 时序 | ✅ crates/monitor-thread-manager/queue.rs | 集合字面量单测对照 electron.js:L158-L166 |
| 崩溃隔离 + 自动重启 + 1s 节流 + 30s 失败复位 | ✅ crates/monitor-thread-manager/thread_manager.rs | 生命周期逐段移植（ready 30s 超时/对话框仅首次）|
| brightnessType 判定（custom VCP 表 + 高层占位）| ✅ all_monitors.rs | 四分支逐字移植（blocked / timedOut / ddcciSupported|custom / highLevel 占位） |
| Studio Display USB 控制 | ✅ studio_display.rs（WinUSB 直调实现）+ studio_display_discovery.rs | SetupDiGetClassDevsW + WinUsb_Initialize + WinUsb_ControlTransfer；报文/nits 换算单测通过 |

## HDR/SDR ✅

- `crates/displayconfig/src/hdr.rs`：windows-hdr.cc 全文等价移植
  - SET_SDR_WHITE_LEVEL=0xFFFFFFEE 未文档化类型 + 24H2 的 GET_ADVANCED_COLOR_INFO_2(type=15) 优先/legacy 回退
  - nits 换算（value*80/1000 读、nits*1000/80 写）、80..480 钳制 + 4 倍数对齐
  - legacy 路径 hdrActive 探测（写当前 SDR white level 验证）逐字保留
  - 真机验证：内屏枚举 nits=80（SDRWhiteLevel=1000 默认值，与 WMI 对照一致）

## Slint UI ✅（骨架）

- `ui/all.slint` 单入口聚合四窗口（slint-build 的 include_modules 只含最后一次 compile，
  多入口需单文件聚合——踩坑记录）
- panel.slint：显示器滑块/刷新/睡眠/设置入口，回调 brightness-changed/refresh-requested/open-settings/sleep-displays
- settings.slint：TabWidget 分类对应 components/Settings/（General/Monitor Features/Hotkeys/Time Adjustments/Sleep & Idle/Advanced）
- intro.slint：首次运行引导（userClosedIntro）；hud.slint：热键 HUD 浮层
- 主线程 = Slint 事件循环（AGENTS.md §5），run_panel() 封装

## 托盘/热键 ✅（骨架）

- `crates/tray`：Shell_NotifyIconW 托盘（NIM_ADD/NIM_DELETE + 消息窗口 + WM_TRAY_ICON 点击路由）
  + RegisterHotKeyW 手写 FFI（windows crate 未绑定，AGENTS.md §1.1 论证）+ WM_HOTKEY 解析单测
- 图标资产直接加载 origin/src/assets/tray-icons/{dark,light}/*.ico（§1.2 禁止自绘）

## Slint UI 实质化 ✅（Round 5 · 2026-11-22）

四个窗口从占位骨架重写为真实可交互 UI（数据驱动 + 全部回调接线），并完成真机渲染与行为对拍。

### 结构

| 文件 | 对应源 | 内容 |
|---|---|---|
| ui/theme.slint | css/vars.scss + page.scss | 设计令牌全局（亮/暗 × Win10/Win11、accent、字体度量） |
| ui/panel.slint | BrightnessPanel.jsx + panel.scss + Slider.jsx + DDCCISliders.jsx + HDRSliders.jsx | 亮度面板：显示器列表 / 联动滑块 / 扩展特征行 / 电源 / 更新条 |
| ui/settings.slint | SettingsWindow.jsx + SettingsOption.jsx + Titlebar.jsx | 设置窗口：标题栏 + 7 页导航 + 数据驱动设置项 + 显示器逐台控制 + 热键列表 |
| ui/intro.slint | IntroWindow.jsx + intro.scss | 首次运行引导（含任务栏位置示意） |
| ui/hud.slint | hotkeyOverlay 系列 | 热键亮度 HUD 浮层 |

### 接线层（Rust）

| 文件 | 对应源 | 内容 |
|---|---|---|
| src/monitor_view.rs | BrightnessPanel 渲染分支 + monitor.util.js + volume-utils.js | 显示器 JSON → UI 模型；isAdjustableDisplay / usesGammaSlider / useFeatures / showPowerButton / isHDROnlySDR 逐分支移植；normalizeBrightness + getCalibratedValue 逐行移植 |
| src/panel_controller.rs | BrightnessPanel 状态机 + IPC 表 + L3027-L3100 节流 + L3138-L3350 updateBrightness | 事件循环 Timer 拉取 worker 事件；VCP 目标判定（SDR/gamma 主滑块）；扩展下限断点；节流器；托盘事件；intro/HUD 窗口 |
| src/settings_view.rs | SettingsWindow 7 页 + SettingsOption | settings.json → SettingsItem/SettingsPage/SettingsMonitorEntry/SettingsHotkey 数据构建 |
| src/settings_host.rs | settings.js + createSettings + send-settings + writeSettings/processSettings | 窗口回调 → settings 写回 → worker 下发 + 重扫判定 |
| src/window_state.rs | tryVibrancy / updateSettingsWindowMaterial / vars.scss | 主题令牌、注册表深色检测、窗口材质（Mica/亚克力） |
| src/hotkey_host.rs | globalShortcut + applyHotkeys + cleanUpKeyboardKeys | accelerator → (modifiers, vk) 解析、热键动作表 |
| crates/tray | electron Tray + setTrayMenu | 专用线程托盘 + TrackPopupMenu 右键菜单 + 点击事件 channel |

### 真机对拍（2026-11-22）

**1) UI 冒烟（--ui-smoke）** — 四窗口创建 + 模型注入 + 本地化

```
panel window: created / panel monitors: 1 visible: 1
  name=Display 1 type=Wmi level=60 extended=false features=0 power=false
theme dark=true win11=true / panel title: 调整亮度
settings pages: 7 (general 23 / monitors 8 / features 6 / time 13 / hotkeys 9 / light-sensor 4 / updates 5)
settings monitor entries: 1 -> connection=WMI hdr=不支持
intro title: 欢迎使用 Twinkle Tray！ / hud label: 亮度
```

**2) 渲染实景** — target/ui-demo.png（面板扩展布局：亮度/对比度/输入源/音量/电源）与
target/ui-shot2.png（intro 窗口）。中文文案、7 页导航、开关、下拉均正确渲染。

**3) 亮度闭环 + 节流（--brightness-smoke=30）**

```
original: 60 -> first target: 30 -> throttled expected: 50 -> readback: 50 -> MATCH
```

连发 30/40/50，只有最后值落地（等价 updateBrightnessThrottle 的「只保留最新值」语义）。

### 本轮修复的真机缺陷

1. **wmibridge set_brightness 恒失败**：enumerator.Next(500, &mut [None], &mut returned) 把结果写进临时数组，
   method_object 永远是 None，函数必然返回 false。改为真实可写槽位 + WBEM_INFINITE。
   真机验证：读 60 -> 设 40 -> 读回 40。
2. **WMI 显示器亮度下发带 id**：原版 type=="wmi" 时不带 id（worker 走全局 WMI 分支），
   带 id 会落到 DDC 分支写不存在的 VCP。已按 monitor.type 分流。
3. **面板启动即显示**：原版面板由托盘左键触发，启动时不可见。改为默认隐藏 + 托盘事件切换。
4. **Slint 语法踩坑**：属性绑定用冒号非等号冒号；if cond 冒号 Element 分支语法；FlowLayout 在 1.19 更名
   FlexboxLayout；LineEdit 需从 std-widgets 导入；Window 无 key-pressed（需 FocusScope）；
   return accept 需写全 EventResult.accept；函数内不能用 for-in 语句（改用 Rust 侧计算）。

### 验证入口

```powershell
.\target\debug\twinkle-tray-rust.exe --ui-smoke --console          # 四窗口 + 模型 + 本地化自检
.\target\debug\twinkle-tray-rust.exe --ui-demo --console           # 面板/设置实景渲染（截图对拍）
.\target\debug\twinkle-tray-rust.exe --brightness-smoke=30 --console  # 亮度闭环 + 节流
cargo test --workspace                                             # 90 个测试用例
```

## 主程序运行时完整化 ✅（Round 6 · 2026-11-23）

新增/重写模块（全部逐段对照 electron.js）：

| 文件 | 覆盖源 | 要点 |
|---|---|---|
| `crates/suncalc` | suncalc v1.9.0（~200 行纯数学） | 逐函数等价移植 getTimes 全链（toJulian/solarMeanAnomaly/eclipticLongitude/julianCycle/solarTransitJ/hourAngle）；**月亮与方位角函数未移植（本仓库调用链从未使用，已记录论证）**；node 基线逐值对拍（北京/纽约双坐标 sunrise/sunset/dawn/dusk 等全键一致） |
| `src/runtime_hosts.rs` | electron.js L1629-L1798（known displays/applyProfile/setKnownBrightness/shouldSkipDisplay）、L3053-L3100（节流）、L3138-L3300（updateBrightness 主进程决策）、L3353-L3495（updateAllBrightness/transition）、L5625-L5856（TOD 全链）、L5485-L5622（idle 检测） | RuntimeHost 状态机（tempSettings/lastKnownDisplays/isUserIdle/recentlyWokeUp/hotkeyThrottle/cycleIndexes）；TOD 含 sunCalc 时间名 + parseTime + LERP 动画计划；idle dim 联动 **全屏检测与媒体播放为「与」条件（§3.2 高危区，未删分支）** |
| `src/system_events.rs` | L5210-L5226（addEventListeners）、L3901-L3977（电源广播）、L5250-L5436（monitor/metrics change）、L649-L664（原生亮度键）、L1801-L1831（applyHotkeys） | 事件源开关逐字保留（useWin32Event/useWmDisplayChangeEvent/useGuidPresenceEvent/useGuidBrightnessEvent/useScMonitorPowerEvent/disablePowerNotifications）；GetTimeZoneInformation 等价 getTimezoneOffset（含 DST bias）；GetLastInputInfo 等价 getSystemIdleTime |
| `src/hotkey_host.rs`（扩展） | doHotkey L1837-L2014 | 全动作类型（off/refresh/set/offset/cycle）；cycle 索引的恒真分支逐字保留；skipInternalBrightnessOffset 分支保留；powerState→0xD2 映射逐字对照 |
| `crates/tray`（扩展） | tray.on("click")/hookWindowMessage 系列 | 消息窗口承载 WM_HOTKEY/WM_DISPLAYCHANGE(126)/WM_POWERBROADCAST(0x218)/WM_SYSCOMMAND SC_MONITORPOWER/WM_INPUT 原生亮度键；TrayEvent 枚举扩展 6 类；菜单项支持 checkbox（MF_CHECKED）与分隔线（MF_SEPARATOR）；tray_window_handle() 供主线程注册热键 |
| `src/panel_controller.rs`（扩展） | setTrayMenu click 回调 + run_panel_controller | 托盘菜单 id 全量路由（1001-1003/2000 quit/2001-2002 暂停 checkbox/2003 刷新/2100+ profiles/2004-2006 DEBUG）；SystemEvents 主线程槽位（RefCell 等价单线程事件循环语义）；HUD 请求消费 |
| `src/main.rs`（重接线） | 启动序列 | RuntimeHost + SystemEvents 装配；start_hotkeys 经托盘窗口句柄 RegisterHotKeyW（MOD_* + MOD_NOREPEAT）；settings 路径快照供菜单构建 |

### 关键实现记录（Round 6）

13. **suncalc 移植范围**：getPosition/moon* 未移植——`getSunCalcTime/getSunCalcTimes`（electron.js:L5733-L5747）只调用 getTimes；按 §1.1 最小化原则裁剪，记录于 crate 文档。
14. **pauseMonitorUpdates 语义**：原版（L3008-L3011）是主进程本地的 `updateInterval*2` 到期标志，不是 worker 消息；以 `paused_until: Mutex<Option<Instant>>` 等价实现，tick 到期复位。
15. **托盘事件路由**：TrayEvent channel 是 mpsc 单发，托盘线程→UI 唯一接收端；系统类事件（Hotkey/PowerSetting/...）由 UI 层 handle_tray_event 转发进 `FORWARDED_EVENTS` 全局队列，SystemEvents::tick 消费——避免双读。
16. **GetPowerSetting 返回序**：twutils 返回 `(guid, name, data)`，tray 转发层按 `(name, guid, data)` 重排后与 electron.js 分支对齐。
17. **'static 节流单例**：doHotkey 与 UI tick 共用 `SHARED_THROTTLE: OnceLock<Mutex<BrightnessThrottle>>`（等价原版 updateBrightnessThrottle 模块级 Map 单例）；HotkeyContext 持 `&'static Mutex<_>`。
18. **profiles 子菜单降级**：tray 菜单暂无 HMENU 子菜单层级，profiles 平铺为 2100+ id 项（点击应用行为等价，视觉少一层缩进）；升级路径记录在案。

### 真机对拍（2026-11-23）

```
# suncalc node 基线（2026-11-23T04:00Z, 北京 39.9042/116.4074, UTC+8）
# dawn 6:26 / sunrise 6:55 / solarNoon 11:59 / goldenHour 16:24 /
# sunsetStart 17:1 / sunset 17:4 / dusk 17:32 / night 18:36  → Rust 逐值一致
cargo test -p suncalc          # 3 passed

# 启动序列（接入 RuntimeHost/SystemEvents 后回归）
--app-smoke → settings keys: 99 / worker ready: true / monitor count: 1 / CMN1521 brightness=60
--ui-smoke  → 四窗口 + 7 页模型 + 本地化全部一致（无回归）
--brightness-smoke=30 → original 60 → throttled 50 → readback 50 → MATCH（60→50→60 闭环）

# 质量状态
cargo build --workspace: 零错误零警告
cargo test --workspace: 96 个测试用例全部通过（12 crate + suncalc 新增 3）
```

## 未完成（按 AGENTS.md §4 映射）

- [x] ~~`activewindow`~~（SetWinEventHook + 进程/包信息，AppxManifest 解析降级为 package family name，已记录论证）
- [x] ~~`sensors`~~（COM Sensor API，环境光；CLSID_SensorManager={77A1C827-FCD2-4689-8915-9D613CC5FA3E} 已核实）
- [x] ~~`material`~~（acrylic.cc SetWindowCompositionAttribute + DWM 属性齐备）
- [x] ~~Monitors worker~~（`--monitor-worker` 子命令 + DDC 哨兵 + 降级链 + IPC 行协议，全量消息分发）
- [x] ~~主程序~~（Round 6：托盘运行时接线 ✅ / 热键注册+doHotkey ✅ / 命令队列节流 ✅ / 事件源开关 ✅ / TOD+idle 联动 ✅ / known displays ✅）
- [x] ~~Slint UI（panel / settings / intro / hud 四窗口）~~（Round 5 实质化完成，见上）
- [x] ~~HDR~~（并入 displayconfig，Round 4 完成；DisplayConfigSetDeviceInfo 真机 nits=80 对拍通过）

### 剩余可深化项（不影响 §4 映射完整性）✅ 全部完成（Round 7 · 2026-11-23）

- [x] ~~托盘 HMENU 子菜单层级~~（TrayMenuItem.children 递归子菜单 + profiles 的 SETTINGS_PROFILES_TITLE 子菜单 + DEBUG 子菜单全项）
- [x] ~~原生亮度键长按重复~~（SystemEvents 状态机：400ms 延迟 + 120ms 周期 + 10000ms 上限 + Raw Input 注册补齐）
- [x] ~~GA4 埋点/更新检查的网络路径~~（WinHTTP 等价移植，§1.2）

## settings 兼容层 ✅

- `crates/monitor-thread-manager/settings.rs`：defaultSettings **99 键逐字对照原版**（含 isDev bare 键、isWin11/disableWin32/useWin32Event 三个含数字键），与 electron.js:L877-L976 逐键 diff 零差异
- 版本迁移：v1.15.0（adjustmentTimes/idle）、v1.16.0（hotkeys 双向 + lastKnownDisplays 重置）、降级分支
- `normalizeMonitorOrder`（旧格式排序去重）、`updateInterval===999 → 100` override
- 单测：键数/默认值抽查/版本比较/旧格式升级/用户值保留/updateInterval override 全通过

## Monitors Worker 完整化（Round 4 · 2026-11-22）

新增/重写模块（全部逐段对照 Monitors.js）：

| 文件 | 覆盖源 | 要点 |
|---|---|---|
| `all_monitors.rs` | getAllMonitors L770-L978 | 十一段合并：WMI→Win32→Studio→DDC→Studio(重放)→内屏亮度→HDR→gamma→software→hideClosedLid→name/num |
| `wmi_fallback.rs` | L1216-L1294 + L2005-L2149 + L1183-L1211 | 桥优先 + **WMIC 回退链保留**（wmicExists 探测、BRIDGE_FAILURE_LIMIT=3 计数切换） |
| `brightness_ops.rs` | setBrightness L1738-L1785、getBrightnessDDC L1636-L1703、readKnownBrightness L644-L720、setVCP L1884-L1905 | 四分支降级链（studio/software/highLevel/DDC）+ catastrophic→[50,100] 兜底 |
| `software_brightness.rs` | L20-L165 | gammaFeaturesActive 三条件、按 path 去重读 ramp、restoreSoftwareBrightness |
| `feature_snapshots.rs` | L453-L537 | 快照保存/应用/失效；Fast 读数优先于快照的合并语义 |
| `studio_display.rs` | L1005-L1083 | WinUSB 直调（**论证后无需 nusb**）；report id 0x01、nits 400-60000 |
| `src/worker_ipc.rs` | handleMonitorMessage L175-L305 | 全 15 类消息分发；settings/ddcBrightnessVCPs 的 changedFeatureMonitorIds + applyMethodOverride 双向覆写 |
| `src/monitor_host.rs` | electron.js L237-L291 / L345-L406 | ready 四连发（settings/ddcBrightnessVCPs/wmi-bridge-ok/localization）、generation 过滤、命令队列接入 |
| `src/main.rs` | L1-L130 / L862-L868 / L1201-L1214 | 启动序列 + `--app-smoke` 自检入口 |
| `src/app.rs` | L24-L27 / L113 / L862-L868 | 路径解析（含 portable 的 config/ 与 ddc-unstable.json 恒在 userData 的差异） |
| `src/logger.rs` | Logger.js 全文 | 会话日志、MAX_KEEP=3 轮转、MAX_BYTES=50MB 封顶、ANSI 剥离 |

### 关键修复

- **thread_manager 就绪标志失效**：原实现用 `thread_local! READY_FLAG` 桥接，但从未赋值，
  导致 `wait_ready` 必然 30s 超时。改为 `Arc<AtomicBool>` 由 stdout 读取线程直接置位
  （等价 electron.js 中主进程 message 监听器的 ready 分支）。

### 真机对拍（2026-11-22）

```
# 主进程启动序列
twinkle-tray-rust.exe --app-smoke --console
→ settings keys: 99
→ worker ready: true / connected: true
→ WMI bridge available: true
→ monitor count: 1
→ CMN1521 brightness=60, connector=internal, type=wmi, hwnd 对齐 \\?\DISPLAY#CMN1521#4&2cf91c03&0&UID8388688

# worker 行协议
echo '{"type":"refreshMonitors","fullRefresh":true,"clearCache":true,"generation":1}' | app.exe --monitor-worker
→ {"type":"ready"} → getMonitorsInternal/Win32/Studio/DDC 各段耗时 → refreshMonitors 回包
```

## 质量状态（回归 2026-11-22 · Round 4）

- **构建**: 零错误零警告（cargo build --workspace）
- **测试**: 12 个 crate 全部通过（63 测试用例）
- **e2e**: `--monitor-worker` 子进程 stdin/stdout 行协议全链路真机验证通过
  （ready → refreshMonitors 真机枚举 CMN1521 → 字段名与原版 camelCase 一致）

## 复现验证

```powershell
# 各 crate 测试
cargo test --manifest-path crates/wmibridge/Cargo.toml -- --nocapture
cargo test --manifest-path crates/ddcci/Cargo.toml ddcci_refresh -- --nocapture
cargo test --manifest-path crates/twutils/Cargo.toml -- --nocapture
# 主进程启动序列 e2e
.\target\debug\twinkle-tray-rust.exe --app-smoke --console
# 本地化校验
node tools/convert-localization.mjs --check
```
## 剩余可深化项完成记录（Round 7 · 2026-11-23）

### 1. 托盘 HMENU 子菜单层级
- `crates/tray/src/tray_icon.rs`：`TrayMenuItem.children: Vec<TrayMenuItem>`，`append_items()` 递归构建
  （MF_POPUP=0x10 追加子菜单句柄；子菜单项点击时 TrackPopupMenu 返回其 id，路由不变）
- `src/main.rs build_tray_menu`：profiles 改为 `SETTINGS_PROFILES_TITLE` 子菜单（id 仍 2100+）；
  DEBUG 改为完整 10 项子菜单（RESTART PANEL/RECREATE TRAY/MINIMIZE PANEL/HIDE PANEL/OPACITY 0-1/
  DO CURRENT TOD/REMOVE ACRYLIC/PAUSE MOUSE/LAST ACTIVE WIN，id 2004-2013）
- 单测：`menu_item_default_children`

### 2. 原生亮度键长按重复（equiv electron.js L627-L714）
- 常量 NATIVE_BRIGHTNESS_KEY_REPEAT_LIMIT=10000 / REPEAT_INTERVAL=120 / 400ms 启动延迟（逐字对照 L636-L639）
- `SystemEvents` 状态机字段：native_key / native_repeat_delay_due / native_repeat_interval_due / native_repeat_until；
  tick 驱动（equiv L683-L688 的 setTimeout/setInterval）
- **补齐 Raw Input 注册**（此前缺失）：`sync_native_brightness_keys()` 按是否配置 BrightnessUp/Down 热键
  注册/注销；`apply_hotkeys` 跳过 BrightnessUp/Down 的 RegisterHotKeyW（equiv L1811-L1815）
- 单测：`native_key_release_clears_state` / `native_key_non_updown_ignored`

### 3. GA4 埋点 / 更新检查 / 坐标（WinHTTP 等价移植，§1.2 预定方案）
- `src/net.rs`：WinHTTP 同步内核（Open→Connect→OpenRequest→SendRequest→ReceiveResponse→ReadData）；
  状态码 WINHTTP_QUERY_STATUS_CODE|FLAG_NUMBER（status=0 bug 修复）、自定义头 WINHTTP_QUERY_CUSTOM；
  URL 手写解析（http/https）
- `ping_analytics`（ga4-mp GA_ENDPOINT 等价）/ `fetch_github_releases` / `fetch_geo_coordinates` / `os_release`
- `src/update_host.rs`：UpdateState 单例 + check_for_updates（getVersionValue 比较 + branch prerelease 过滤 +
  dismissedUpdate）+ download_and_install（流式下载 + 大小校验 + DETACHED_PROCESS spawn）+ ignore/clear
- 接线：settings 按钮 check-for-updates/get-coordinates；设置窗口打开即检查（L211 等价）；
  background 60s 检查（L5850 等价）；checkForUpdates 开关响应（L1376-L1383 等价）；面板更新条 install/dismiss + tick 轮询；
  SystemEvents analytics 29min 周期 + 28min 最小间隔 + 开关变化重启（L1429-L1440 等价）
- 单测：net（url_parsing/wide_conversion）+ update_host（version_value/find_latest_release 三分支）
- 真机验证：GitHub API 200（141KB）/ 坐标 X-Geo-Lat=22.9481 X-Geo-Long=113.9276 /
  beta 分支找到 v1.18.0-beta2（current 同版本不显示，等价 L5054）/ master 跳过 prerelease 无更新

**质量状态（Round 7）**：cargo build --workspace 零错误零警告；cargo test --workspace 104 个测试全部通过


## Round 8 · 2026-11-23：设置落盘 + UDP 服务器 + debug 页 + 运行时开关补全

### 审计发现与修复（对照 origin 逐键审计 99 个 defaultSettings 键）

| 缺项 | 源位置 | 修复 |
|---|---|---|
| **settings 磁盘落盘完全缺失**（update_setting 只写内存） | electron.js:L1221-L1237 writeSettings（333ms 防抖） | `monitor-thread-manager::write_settings_file`（tab 缩进序列化，等价 JSON.stringify(settings,null,'\t')）+ `settings_host::schedule_write_settings`（333ms 防抖单例，等价 writeSettingsTimeout 语义）；update_setting/reset-settings 两条路径均接线 |
| **TT_SETTINGS_PATH 被读取但从未设置**（Open Settings 按钮失效） | — | main.rs 启动时 set_var |
| **UDP 服务器完全缺失** | electron.js:L6145-L6371 | `src/udp_host.rs`：handleClientMessage（list/get/set/setvcp/checktime/refresh + udpKey 校验 + findMonitor 子串匹配 + determineVCP 映射）+ udp.start（端口候选 [base,base+13137,base+1603] 逐候选降级 + udpRemote 绑定域 + udpPortActive 写回）+ udp.stop；WinSock 直调（白名单内）；收包线程 + RT_SLOT 模块级单例等价原版闭包引用 |
| **设置页缺 debug 页**（原版 8 页，Rust 7 页） | SettingsWindow.jsx:L1719-L1798 | settings_view `debug_page` 36 项：Refresh/Clear Cache/Save Report/Open Settings + Other 区全部开关（isDev/UDP 四键/preferredDDCCIMethod 下拉/恢复秒数三键/checkVCPWaitMS/scrollFlyoutAmount/事件源开关 6 键/reloadTray/reloadFlyout/logging/showConsole/useTaskbarRegistry/disableMouseEvents）；en.ftl +52 键（原版硬编码英文，走 FTL 回退链行为一致） |
| **resume 恢复链未接**（PowerBroadcast 空分支） | electron.js:L5319-L5360 + L5462 | PBT_APMRESUMEAUTOMATIC/RESUMESUSPENDED → recentlyWokeUp/resume_recovery 标志 + restartOnWake 整机重启分支（等价 app.relaunch/exit）+ worker 重启（manager.stop/start(true)）+ disableAutoRefresh 条件保留 |
| **killWhenIdle 无运行时引用** | electron.js:L3684-L3688 | 面板隐藏分支接线；Slint 无窗口销毁 API，以隐藏降级（差异记录） |
| **useNativeAnimation 无运行时引用** | electron.js:L4333-L4340/L3694 | apply_window_material Win10 分支：false 时退化为无材质（tryVibrancy(false) 等价） |
| **overrideTaskbarPosition/Gap 无运行时引用** | electron.js:L4105-L4115 | default_panel_position 接线（BOTTOM 假定 + gap 12 的既有降级保留，override 键原样支持） |

### 真机对拍（2026-11-23）

```
# UDP（--udp-smoke：启动 → list → refresh → get → 错误 key → stop）
udp_start: true / listening 127.0.0.1:14715
list: OK (monitors JSON) → refresh 后 monitors: 1
get brightness: 64.0（findMonitor "UID" 子串命中内屏 hwid2）
bad key: no response（udpKey 校验正确拒绝）
# settings 落盘
write_settings_file tab 缩进 + 读回等价（单测）；update_setting → 333ms 防抖落盘（单测）
# UI
ui-smoke: settings pages: 8 / page debug -> 36 items
```

### 关键实现记录（Round 8）

19. **monitors 的键是 hwid[2]**（instance name 第 3 段如 `5&xxx&0&UID440`，不含厂商名）——UDP findMonitor 的搜索串应传键子串（如 "UID"），与原版 Object.keys(monitors) 行为一致。
20. **udp_port_active 写回后立即 schedule_write_settings**（等价原版 writeSettings 在 listening 回调里的调用）。
21. **resume 链的 worker 重启**用 manager.stop→start(true)（allowWhileWindowsIdle 等价）；restartOnWake 分支等价 app.relaunch+exit（spawn 自身 + exit(0)）。
22. **debug 页按钮** refresh-monitors/clear-cache 接入 handle_settings_button（flushvcp 命令已在 COMMANDS_MUTATE_WORKER_STATE 集合）。

**质量状态（Round 8）**：cargo build --workspace 零错误零警告；cargo test --workspace 110 个测试全部通过；--udp-smoke/--ui-smoke/--app-smoke 真机对拍通过

## Round 9 · 2026-11-23：修复「启动后托盘图标不出现」

### 症状

程序启动后任务栏通知区**没有任何图标**，托盘完全不可用（左键/右键/菜单全部失效）。
日志仅一行 `[MAIN:ERR] Tray icon asset not found.`，之后 `Tray window unavailable; hotkeys not registered.`。

### 根因（三处叠加）

| # | 缺陷 | 位置 | 后果 |
|---|---|---|---|
| 1 | **图标资产路径解析失败** | `src/main.rs start_tray` | 只查 `exe_dir/assets/tray-icons/...`，而 dev 运行时 exe 在 `target/debug/`，资产在仓库根 `origin/src/assets/`。找不到 → `return (None, None)` → **托盘线程根本没启动**，热键也随之失效 |
| 2 | **NOTIFYICONDATAW.cbSize 恒为 0** | `crates/tray/src/tray_icon.rs` | windows 0.62 的 `Default` 是 `core::mem::zeroed()`，`cbSize` 默认 0。`Shell_NotifyIconW` 要求 `cbSize == sizeof(NOTIFYICONDATAW)`，否则 NIM_ADD **静默返回 FALSE**（无错误码、无日志） |
| 3 | **面板定位在 DPI≠100% 时跑到屏幕外** | `src/panel_controller.rs default_panel_position` | 硬编码 `1920x1080` 且逻辑/物理像素口径混用。真机 125% 缩放下面板落在 `1940,1123`，超出 1920×1080 屏幕 → 点托盘后面板"弹出"但用户看不见 |

### 修复

1. **图标搜索链**（`main.rs`）：按优先级尝试 5 个候选路径——exe 旁 `assets/`（发布布局）、exe 旁 `origin/src/assets/`、exe 上两级仓库根、当前工作目录的 `origin/src/assets/`。失败时记录已搜索的全部路径（便于现场诊断）。
2. **cbSize 显式填充**（`tray_icon.rs`）：NIM_ADD / NIM_MODIFY / NIM_DELETE 三处均设置
   `nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32`，并附 MSDN 依据注释。
3. **面板定位**（`panel_controller.rs`）：改用 `SystemParametersInfoW(SPI_GETWORKAREA)` 读真实工作区（自动扣除任务栏），统一换算到逻辑像素计算后乘 `scale_factor` 返回 `PhysicalPosition`；新增 `Win32_UI_HiDpi` feature 用于 `GetDpiForSystem`。

### 附带修复

- `spawn_tray_thread` 返回值由 `Receiver<TrayEvent>` 改为 `(Receiver<TrayEvent>, Option<TrayIconHandle>)`：
  主线程现在能拿到图标句柄（供 `update_tray_icon` 主题切换），并在 NIM_ADD 失败时明确记录错误。
  HWND 非 Send，跨线程以 `(usize, u32)` 传递后重建；`TrayIconHandle` 派生 `Clone, Copy`。
- `settings_host::schedule_write_settings_debounces` 测试隔离：改用带进程号+纳秒的唯一临时目录，
  并在排期前强制清空残留的 `WRITE_DEBOUNCE` 单例（此前与其他测试共享 `TT_SETTINGS_PATH` 会偶发失败）。

### 真机对拍（2026-11-23 · 1920×1080 @ 125% DPI，Win11）

```
# 修复前
[MAIN:ERR] Tray icon asset not found.
[MAIN:WAR] Tray window unavailable; hotkeys not registered.
→ 任务栏无图标，Shell_NotifyIconGetRect 无法查到窗口

# 修复后
[MAIN] Tray icon: G:\...\origin\src\assets\tray-icons\light\icon.ico
[MAIN] Tray icon thread started.
Hotkeys registered: 0/0

Shell_NotifyIconGetRect → HRESULT=0x00000000 (S_OK)
  icon rect: 1558,1030,1588,1080     ← 在任务栏通知区内，30×50 标准尺寸
  像素采样: maxLum=249（明亮图标像素已绘制）
左键点击（PostMessage WM_TRAY_ICON/WM_LBUTTONUP）
  → 面板 '调整亮度' rect=1460,866,445,148  inside_screen=True
```

面板定位修复前后对比：

| | 位置 | 是否在屏幕内 |
|---|---|---|
| 修复前 | 1940,1123 | ❌ 超出 1920×1080 |
| 修复后 | 1460,866 | ✅ 右下角贴任务栏 |

### 回归测试

新增 `tray_icon::tests::notify_icon_data_cbsize_is_required`：
断言 `NOTIFYICONDATAW::default().cbSize == 0`（记录 zeroed 语义），并验证显式赋值后等于
`size_of::<NOTIFYICONDATAW>()`、x64 下 ≥ 88 字节——防止该字段再被遗漏。

**质量状态（Round 9）**：cargo build --workspace 零错误零警告；cargo test --workspace **111 个测试全部通过**；
真机 e2e（图标可见性 + 点击弹出面板 + 面板定位）通过。

## Round 10 · 2026-11-23：修复「亮度调整窗没有滑动条」

### 症状

托盘左键弹出面板后，窗口里只有一个标题栏和一行 **"正在检测显示器..."**（或空白），
**没有任何亮度滑条**；换 `--ui-demo` 强制注模能看到滑条，但滑条会冲出窗口右边界、
右侧数字框完全不可见。

### 根因（三处独立缺陷，叠加成"没滑条 + 拖不动 + 一点就崩"）

| # | 缺陷 | 位置 | 后果 |
|---|---|---|---|
| 1 | **worker 事件监听器在 `start()` 时被快照成单个回调** | `crates/monitor-thread-manager/src/thread_manager.rs` | `main` 的顺序是 `install_handlers()` → `start()` → 建面板控制器。`start()` 里 `let cb = event_callback.lock().clone()` 把 host 的回调**复制**进读取线程；随后 `PanelController::new` 用 `set_event_callback` **覆盖**了字段，但读取线程仍持有旧回调 → 面板永远收不到 `refreshMonitors`/`monitorsEnriched` → `state.monitors` 恒为空 → `visible-count == 0` → 只渲染"正在检测显示器" |
| 2 | **`TtSlider` 根元素 `width: 100%`** | `ui/panel.slint` | 布局内的百分比宽被解析成"整行宽度"，滑块吃掉同行的 52px 数值框空间并把数值框顶出窗口；窗口右侧因此看不到数字。原版 CSS 是 `.rangeGroup{flex:2}` + `.val{width:52px}`，滑块应占剩余宽度而非整行 |
| 3 | **节流器用 `Instant::now() - 3600s` 表示"很久以前"** | `src/panel_controller.rs BrightnessThrottle::request` | 系统开机不足 1 小时时 `Instant - Duration` 直接 **panic**（`overflow when subtracting duration from instant`）。真机复现：面板刚出现滑条，点一下 → 整个主进程崩溃 |

### 修复

1. **事件回调改为多监听者广播**（`thread_manager.rs`）：
   `event_callback: Mutex<Option<Arc<..>>>` → `event_callbacks: Arc<Mutex<Vec<Arc<..>>>>`，
   新增 `add_event_callback()`（等价 EventEmitter 的 `.on()`，可重复注册）。
   读取线程不再在 `start()` 时快照回调，而是**每收到一条消息取当前列表广播**（先克隆列表再调用，避免回调内注册导致自锁）。
   `monitor_host.rs` 与 `panel_controller.rs` 两处调用点改为追加注册 → host 缓存与面板 UI 同时收到事件。
2. **滑块宽度交给布局分配**（`ui/panel.slint`）：
   去掉根元素 `width: 100%`，改 `min-width: 40px`，实例侧统一 `horizontal-stretch: 1`；
   轨道/进度/手柄改为 `parent.width` 驱动。顺带对齐原版视觉：轨道 4px、手柄 8×22 圆角胶囊、
   hover 变 `--tray-text`（原来的 14px 圆点是自造样式）。
3. **节流器 `last_time` 改 `Option<Instant>`**（`panel_controller.rs`）：
   `None` 表示"从未写入 → 首次立即放行"，比较用 `saturating_duration_since`，
   排队到期时间以 `unwrap_or(now)` 为基准 —— 语义与"很久以前"完全等价，但不再依赖开机时长。
4. **列表模型增量更新**（`panel_controller.rs refresh_ui`）：模型只创建一次（`PanelState::panel_model`），
   之后按行 `set_row_data` / `push` / `remove`，逐字段比较（`panel_monitor_equal`）。
   原来每 50ms tick 都新建 `VecModel` 并 `set_monitors` 整表重建，等价于 React 里把整棵列表卸载重建：
   拖动中的 `TouchArea` 会丢 pressed（**拖一格就断**）、正在编辑的数值框会失焦。
5. **补齐数值框与图标**（`ui/panel.slint` + `src/monitor_view.rs`）：
   - `FeatureRow` 增加数值框（原版 `.feature-row .input--val` 是存在的，此前缺项）；
   - 特征图标按原版 `DDCCISliders.jsx`：对比度 `E793`、音量 `E767`、输入源 `E839`、自定义默认 `E897`；
   - 输入源行补 `E839` 图标列（原版 `.feature-icon`）；
   - 数值框编辑改用内建 `TextInput` + `FocusScope{forward-focus: editor}`：
     std-widgets 的 `LineEdit` 的 `min-width` 硬编码 160px，塞进 52px 数值框会把布局撑破，
     且默认拿不到键盘焦点（`FocusScope` 不设 `forward-focus` 时按键不会到 `TextInput`）。
     进入编辑态 `select-all()`，直接输入即替换旧值（否则会在 "70" 里插入成 "4570"）。
6. **面板高度显式绑定**（`ui/panel.slint`）：`body.height = content.preferred-height + 2px`，窗口 `height: body.height`，
   消除"窗口高度 ↔ body 100% 高度"的循环（此前扩展布局最后一行会被窗口底边切掉半行）。

### 真机对拍（2026-11-23 · 1920×1080 @ 125% DPI，Win11，内屏 CMN1521 / WMI 亮度）

```
# 修复前
面板 rect=1460,866,445,148  → 内容只有 "正在检测显示器..."（monitors 恒空）
--ui-demo 注模：滑条右端溢出到窗口外，数值框不可见

# 修复后（真实托盘点击，未注模）
面板 rect=1460,848,445,168  内容: "Display 1" + 滑条 + 数值 64
鼠标点击滑条 25% 处 → 面板数值 24；Get-CimInstance WmiMonitorBrightness → 24   ✅
按住拖动 25% → 70%   → 面板数值 70；Get-CimInstance → 70                      ✅
点击数值框 → 输入 "45" → 回车 → 面板 45；Get-CimInstance → 45                 ✅
托盘左键隐藏 → 再左键显示 → 面板 rect 与数值不变，无崩溃                       ✅
恢复原值：滑条点回 64% → Get-CimInstance → 64                                 ✅
```

扩展布局（`--ui-demo` 注入 0x12/0x62/0x60/0xD6）：

```
调整亮度 | Display 1 (关机)
  [E706] 亮度      ———●———          70
  [E793] 对比度    ——●————          50
  [E839] [VGA-1][DVI-1][DisplayPort-1][HDMI-1]
  [E767] 音量      —●—————          40
```

### 回归测试

`cargo test --workspace` 全绿（含 `throttle_writes_immediately_then_queues` 覆盖"首次请求立即放行"语义，
`throttle_keeps_latest_value`、`vcp_key_normalization` 覆盖节流器其余行为）。

**质量状态（Round 10）**：`cargo build --workspace` 零错误零警告；真机 e2e（滑条渲染 / 点击 / 拖动 / 数值框 / 显隐循环）通过。


## Round 11 · 2026-11-24：修复「显示器设置」页面全空

### 症状

托盘左键弹出面板 → 齿轮打开设置 → 点击左侧「显示器设置」，右侧内容区**完全空白**：
既没有「所有显示器」信息块，也没有亮度更新速率/隐藏内部显示器等设置项。
`--ui-demo` 下因手工注入 monitors 模型才看得到内容，掩盖了真实路径的缺陷。

### 根因（四处叠加）

| # | 缺陷 | 位置 | 后果 |
|---|---|---|---|
| 1 | **打开设置窗口时从未注入 monitors 模型** | `settings_host::open_settings_window` | `root.monitors` 恒为 `[]`，逐台列表零行 |
| 2 | **monitors 页的 items 被渲染分支排除** | `ui/settings.slint` 的 `if (active-page != "monitors" && != "hotkeys")` | `build_items("monitors")` 构建的 8 个全局项（更新速率/隐藏内屏/睡眠动作/电源值/平衡亮度说明）永远不渲染 |
| 3 | **`refresh_settings_view` 从未被调用** | 全仓无调用点 | worker 的 `refreshMonitors`/`monitorsEnriched` 只更新面板，设置窗口数据陈旧 |
| 4 | **monitors 页缺原版内容** | `ui/settings.slint` + `settings_view::monitors_page` | 缺「All Displays」明细（名称/内部名称/通信方式/当前亮度/最大亮度/统一区间/HDR）、缺归一化 min/max 滑块与校准点 |

### 修复

1. **打开即注入**（`settings_host.rs`）：`open_settings_window` 用 `host.monitors_snapshot()` 构造
   `SettingsMonitorEntry` 列表注入 `set_monitors`，并请求一次 `refresh_monitors(false, false)`
   （equiv 原版 `window.requestMonitors()`）。
2. **事件驱动同步**（`panel_controller.rs` + `settings_host.rs`）：tick 每 50ms 调
   `refresh_settings_monitors`，等价原版 `window.addEventListener("monitorsUpdated", this.recievedMonitors)`。
   以 `last_monitors` 判重 + `needs_monitor_refresh` 脏标记，命中才重建；模型单实例 + 按行
   `set_row_data` 增量更新（整表重建会让正在编辑的重命名框失焦，同 Round 10 教训）。
3. **Slint 结构对齐原版三段**（`ui/settings.slint`）：
   - 抽出 `SettingsItems` 组件，供 general/features/time/updates/light-sensor/debug 与 monitors 页共用，
     消除「monitors 页不渲染 items」的分支缺陷；
   - monitors 页 = ①All Displays（`getInfoMonitors` 明细，空时显示 `GENERIC_NO_COMPATIBLE_DISPLAYS`）
     ②全局 SettingsOption 列表 ③逐台控制卡（重命名 / 隐藏 / 排序↑↓ / Gamma / Extend Minimum+断点 /
     SDR / min-max 滑块 / 校准点增删）；
   - 新增 `MiniSlider`（equiv `Slider.jsx type=min/max`）与逐台 `calibration` 模型字段。
4. **显示语义对齐 `getInfoMonitors`**（`settings_view.rs`）：`brightness`/`maximum`/`normalization`
   按 `displayLevel` 只取整；`type=ddcci` 且无 `brightnessType` → `"???"`；`type=none` → Not supported。
5. **remaps 写回**（`settings_host.rs`）：`monitor_remap` 实现原版 min/max 互锁（间隔 ≥10、边界 0/100），
   `calibration_*` 实现增删改；均走 `apply_side_effects` 下发 worker 并标记脏。

### 真机对拍（2026-11-24 · 1920×1080 @125%，Win11，内屏 CMN1521 / WMI）

```
# 修复前（真实托盘路径）
托盘左键 → 面板 → 齿轮 → 「显示器设置」→ 右侧内容区 0 个可见元素

# 修复后（真实托盘路径，未注模）
PostMessage(TwinkleTrayTrayClass, WM_TRAY_ICON/WM_LBUTTONUP)
  → 面板「调整亮度」hwnd=1575286 rect=1168,678,1524,812
点击面板齿轮 → 设置窗口 hwnd=1967168 rect=154,154,914,714
点击侧边栏「显示器设置」→
  所有显示器：Display 1 / 名称 Display 1 / 内部名称 CMN1521 / 通信方式 WMI
              当前亮度 66 / 最大亮度 不支持 / 统一亮度区间 0 - 100 / HDR 不支持
  显示器设置：亮度更新速度（一般 500ms）/ 隐藏不活动的内部显示器 / 睡眠动作 PowerShell …
  （继续下滚）平衡亮度段 + Display 1 控制卡：重命名输入框、隐藏/Gamma/SDR 开关、
   Extend Minimum、最小值 0 / 最大值 100 滑块、+ 校准点
```

`--ui-demo` 已改为走真实 `open_settings_window`（此前手工 `set_monitors` 掩盖缺陷），
输出 `settings monitors: 1`，截图与上述一致。

### 回归测试

`cargo test --workspace` 全绿（新增/更新 7 个用例）：
`monitor_entry_connection_labels`（detail/亮度语义）、`monitor_entry_ddcci_without_brightness_type_shows_unknown`、
`monitor_entry_none_type_reports_not_supported`、`monitors_page_renders_all_display_items`、
`update_remap_min_keeps_gap_and_caps_max`、`update_remap_max_keeps_gap_and_caps_min`、
`calibration_points_add_update_remove`。

**质量状态（Round 11）**：`cargo build --workspace` 零错误零警告；真机 e2e（托盘 → 面板 → 设置 → 显示器设置）通过；
点击 min 滑块可落盘到 `settings.remaps`（实测 `{"DISPLAY1":{"min":90,"max":100}}`）。

### 已知差异（记录，不阻塞）

- 面板/设置窗口的关闭采用 `hide()` 而非销毁（Slint 无等价 Electron `window.close()`），沿用 Round 8 记录。
- 设置页重排（拖拽）降级为上下移按钮（Slint 无拖拽列表），语义等价。
- `SETTINGS_MONITORS_GAMMA_SLIDER_TITLE` 在 zh-Hans.ftl 缺失 → 回退 en（源 zh-Hans.json 同样缺该键，属本地化数据差异）。

## Round 12 · 2026-11-24：设置页下拉框改 ComboBox + 语言/主题/UI 样式生效

### 症状

通用设置页的「语言」「主题」「Windows UI 样式」三项是"点一次轮换到下一项"的伪下拉，
且切换后**不生效**：语言不变、主题不重着色、Win11/Win10 样式不切换。

### 根因

| # | 缺陷 | 位置 | 后果 |
|---|---|---|---|
| 1 | select 项渲染为 click-to-cycle 按钮（Slint 早期无下拉方案时的过渡实现） | `ui/settings.slint` | 交互与原版 `<select>` 不符 |
| 2 | `item_choice` 只写 settings，无任何外观/语言副作用 | `settings_host.rs` | 语言：`tt_localize` 永不重载 → 不生效；主题：仅靠 monitors 轮询顺带刷新；windowsStyle：`isWin11` 从未映射 → 恒为启动值 |

### 修复

1. **真实下拉**（`ui/settings.slint` + `settings_view.rs`）：select 项改 `std-widgets ComboBox`
   （fluent 样式，等价原版 `<select>`）。`SettingsItem` 以同序 `option-values`/`option-labels`
   + `selected-index` 替代 `selected-label`/`next-value`/`options([SettingsChoice])`（结构体删除）。
   ComboBox 的 `selected` 回调只传显示文本，Rust 侧 `resolve_choice_label` 按当前 items 模型
   按 labels 位置映射回 values（等价 `e.target.value`；数值/文本输入项无选项表，原样透传）。
   此改动对全部下拉项生效（updateInterval/sleepAction/preferredDDCCIMethod 等同步受益）。
2. **windowsStyle → isWin11**（`settings_host.rs`，equiv electron.js:L1345-L1355）：
   `win11→true / win10→false / system→真实检测`（`monitor-thread-manager::is_windows_11` 转公开）。
3. **语言切换**（`settings_host.rs reload_language`，equiv L1277-L1280 getLocalization + rebuildTray）：
   重载 FTL 语料（system → `window_state::system_locale()` 检测，检测函数自 main.rs 移入 lib）→
   重建侧边栏页名 + 设置窗口文案 + 面板文案（`panel_controller::apply_panel_texts`，
   新增 PANEL_WINDOW 主线程 Weak 槽位）→ 托盘菜单重建
   （`tray::update_tray_menu` 仅替换菜单快照，图标不重建；main.rs 经
   `settings_host::set_tray_menu_rebuilder` 注册钩子）。
4. **主题广播**（equiv L1252-L1254：settings.theme 恒真，每次 processSettings 都发）：
   `item_choice` 路径统一调 `apply_settings_theme`（设置窗口）+ `apply_panel_theme`（面板），
   windowsStyle 写回 isWin11 后由同一广播生效。

### 已知差异（记录，不阻塞）

- `selected` 回调选中后内部赋值会覆盖 `current-index` 外部绑定，此后同页项的外部重建
  （如 reset-settings）不再回写选中下标，依赖 ComboBox 的 `changed model` 自校正；
  正常"选择 → 写回 → 重建"链路显示始终正确。
- 托盘菜单结构键（has_tod/profiles/isDev）读 settings 文件快照（333ms 防抖落盘），
  与语言无关；文案经 tt_localize 即时生效。

### 回归测试

`cargo test --workspace` 全绿：新增 `select_resolves_current_index`、
`select_items_carry_aligned_options`；其余 54+ 用例（含 settings_view/settings_host 全部）
0 失败。`cargo build --workspace` 零错误。

## Round 13 · 2026-11-24：修复「疑难解答」6 个开关无字

### 症状

设置 → 通用 →「疑难解答」下 6 个开关（Apple Studio Displays / DDC/CI HL / HDR /
WMIC / WMI-Bridge / Win32-DisplayConfig）只有右侧圆钮，左侧标题整行为空。

### 根因

`general_page` 把这 6 项的原版硬编码英文标题（`SettingsWindow.jsx` L1417-L1422 的
`<SettingsChild title={"Apple Studio Displays"} .../>`）当作 i18n key 传给
`tt_localize::t()`。源语料无对应键 → 经 en.ftl 回退链返回空串 → `SettingsRow`
渲染空标题（`ui/settings.slint` L223-L228 直接 `text: item.title`）。

### 修复

`src/settings_view.rs` 新增 `ItemBuilder::toggle_literal(key, title, settings)`，
直接写入字面量标题、不经本地化；6 项改用该方法。语义与原版一致（原版所有语言下
这 6 个标题都是英文硬编码，未纳入本地化语料）。

### 对拍验证

真机 `--ui-demo` 切 general 页，滚动到「疑难解答」：6 个开关标题分别显示
`Apple Studio Displays` / `DDC/CI HL` / `HDR` / `WMIC` / `WMI-Bridge` /
`Win32-DisplayConfig`，右侧开关控件正常，点击路径不变（settings_host 副作用链未改）。

### 回归测试

新增 `troubleshooting_toggles_keep_literal_titles`：断言 6 项标题逐字匹配原版、
且 general 页不存在空标题项。`cargo test --lib` 55 passed / 0 failed；
`cargo build` 零错误。

## Round 14 · 2026-11-24：修复「快捷键」设置页全空 + 热键增删全链

### 症状

设置 → Hotkeys & Shortcuts 页几乎全空：无标题/说明/开关，只有（空字符串的）占位文本
和一个 "+ Add Hotkey" 小按钮；且点击 "+ Add Hotkey" 无任何反应，页面永远空白。

### 根因（三层）

1. **items 构建了但从未渲染**：`hotkeys_page()` 构建的标题/说明/break/scroll 等项
   挂在 `root.items` 上，而 `settings.slint` 内容区 `SettingsItems` 分支的条件
   （`active-page != "monitors" && != "hotkeys"`）把 hotkeys 页排除，
   hotkeys 分支只渲染热键列表本体 → items 全部丢失。
2. **失效的 i18n 键**：`SETTINGS_HOTKEYS_NONE`（语料中不存在，占位文本恒空）、
   `SETTINGS_HOTKEYS_SCROLL_AMOUNT_TITLE`（正确键为 `SETTINGS_HOTKEYS_SCROLL_AMOUNT`）、
   `SETTINGS_HOTKEYS_PERCENT_TITLE`（不存在，且原版页面本无 hotkeyPercent 行）。
3. **"+ Add Hotkey" 断链**：`add_hotkey`/`remove_hotkey` 只改内存 settings，
   既不刷新 UI 模型，也不落盘/下发 worker/重注册热键（原版
   `window.sendSettings({hotkeys})` → processSettings → applyHotkeys 全链）。

### 修复

- `ui/settings.slint`：新增 `items-after` 属性；hotkeys 分支改为
  「列表前 items（标题/说明/原生亮度警告）→ 热键列表 + "+ Add Hotkey" →

## Round 16 · 2026-11-24：修复「设置窗口无法拖动」+ 拖动全程掉帧/松手飘位

### 症状

设置窗口（无标题栏边框的自绘标题栏窗口）**完全拖不动**：按住标题栏拖 200px 松手，
窗口纹丝不动；而首次运行引导窗口（同样 no-frame）重新打开后也再挪不动。
附带的同类现象：面板「拖动后松手位置和鼠标对不上」，拖动过程中画面/定时器停摆。

### 根因（两层，均为等价性缺口而非新 bug）

| # | 缺陷 | 位置 | 后果 |
|---|---|---|---|
| 1 | **标题栏没有拖动区域**。原版 `Titlebar.jsx` 的 `.titlebar-drag-region` 用 `-webkit-app-region: drag` 支撑整个标题栏；Slint 版只画了标题与三枚按钮，没有 `WindowMoveArea`。`no-frame: true` 下窗口没有系统标题栏可用，等于没有可拖拽的像素 | `ui/settings.slint` / `ui/intro.slint` | 按住标题栏拖动无任何反应 |
| 2 | **拖动期间 Slint 定时器停摆**。拖动走 `WM_NCLBUTTONDOWN(HTCAPTION)` 进入系统的「尺寸调整模态循环」；winit 的 `MsgWaitForMultipleObjectsEx` 在模态循环期间拿不到「有新消息」，Slint 的 50ms tick 与渲染停住 → 拖动只跳一下、松手后迟到的 `SetWindowPos` 再飘一次 | winit 0.30 派生行为 + `init_slint_backend` | 拖动体验碎裂、松手位置与鼠标不一致 |

### 修复

1. **标题栏交给 Slint 的 `WindowMoveArea`**（`ui/settings.slint`）：标题栏改为 `Rectangle` 容器，
   第一个子元素是铺满整条的 `WindowMoveArea {}`（等价 `.titlebar-drag-region` 的 drag 区），
   标题文本与三枚窗口按钮排在它之上的 `HorizontalLayout` 里——按钮自身的 `TouchArea` 先拿到
   指针事件，等价原版 `.window-controls-container` 的 `no-drag`。点击不移动窗口，按钮不受影响。
   同时补 `resize-border-width: 6px`（Slint 默认 4px）：无边框可调窗口的「上边缘缩放」
   否则会把标题栏顶部一整段吃掉，取 6px 让可见的拖动区域仍占标题栏主体。
2. **引导窗口补拖动条**（`ui/intro.slint`）：原版 `IntroWindow` 同样是无框且无标题栏，
   窗口自身没有任何拖动区；顶部加一条 26px 的 `WindowMoveArea` 承担 drag 区，避免「一旦被移到
   屏幕外就再也拿不回来」。
3. **模态循环补充泵**（`src/window_drag.rs` + `src/main.rs::init_slint_backend`）：
   - `init_slint_backend()` 用 `BackendSelector::backend_name("winit")` +
     `with_winit_event_loop_builder(...)` 选后端，并在 winit 的 `EventLoopBuilder` 上装
     `with_msg_hook`；原代码依赖 Slint 默认后端（同样是 winit），行为不变但显式化。
   - 钩子在 `WM_ENTERSIZEMOVE..WM_EXITSIZEMOVE`（移动/缩放两种系统循环）区间内，用
     `PostMessageW` 把消息转投给 winit 的「Winit Thread Event Target」窗口（其窗口过程就是
     winit 的 WndProc），于是拖动全程 UI 定时器与渲染继续跑；转投顺序保证 `WM_EXITSIZEMOVE`
     处理时窗口位置已在 `WM_EXIT` 前设置好，不再飘。用 `thread_local` 防自递归，投递失败时
     退回 winit 的正常派发（不 panic，AGENTS.md §5）。
   - 钩子需要 `BackendBuilder::with_window_msg_hook`，而 slint 1.19 没暴露这个入口；
     **离线构建补丁**：`tools/slint-winit-patch/apply.ps1`（幂等）在 `i-slint-backend-winit-1.19.0`
     的 `lib.rs` 里补该入口（真正落到 winit `EventLoopBuilder::with_msg_hook`，winit 0.30
     没有运行期安装接口）。构建前需执行一次；脚本已入库。
4. **DPI 换算口径统一**（`src/window_state.rs` 新增 `window_hwnd` / `window_dpi_scale`）：
   Slint 的 `set_position` 走 winit，winit 按 `GetDpiForWindow(hwnd)/96` 换算，而代码原先用
   `window().scale_factor()`；两者不一致时窗口落点错位（真机实测：`--ui-demo` 里设置窗口
   要求 (440,60)、实际落在 (192,192)，因为系统 125% 但进程 DPI 为 150%）。面板定位与
   demo 摆位统一改用 `window_dpi_scale`，拿不到 HWND 时退回 `scale_factor`（不新增失败路径）。
5. `--ui-demo` 增加 `--demo-pos=x,y`：把两个 demo 窗口摆到指定物理坐标，便于用自动化脚本
   做「按住标题栏拖动」的交互对拍（窗口默认居中的位置会盖住面板、让脚本点不到按钮）。

### 依赖变更（AGENTS.md §1.1）

| 依赖 | 论证 |
|---|---|
| `slint` 增加 `unstable-winit-030` feature | 只借用 winit 的 `EventLoopBuilder` 装消息钩子。该 feature 自身即 `["backend-winit", ...]`，**不引入新 crate** |
| `i-slint-core`（`=1.19.0`，`raw-window-handle-06`） | 必要性：`slint::Window` 不直接暴露 HWND，只有 `i-slint-core` 的 `Window::window_handle()`（feature 门控）能取。等价性：只用于读句柄与 DPI，不改变任何 Slint 行为。最小化：与 slint 1.19 同版本的传递依赖，**依赖树无净增** |
| `raw-window-handle`（0.6） | 同上，i-slint-core 的传递依赖（0.6.2），只用来解读上面那个 handle |

### 真机对拍（2026-11-24 · 1536×864 @125%、进程 DPI 150%，Win11）

```
# A. 拖动（修复前 vs 修复后，同一条自动化拖拽脚本：按下→24 步移动→松开）
修复前（--demo-pos=40,40，窗口 336,32）：拖动后 rect=336,32  → ❌ 纹丝不动
修复后：逐帧采样（每 40ms 一次 GetWindowRect + GetCursorPos）
  i=5  cur=570,244 win=282,218   i=15 cur=510,304 win=222,278
  i=25 cur=450,364 win=162,338   i=30 cur=420,394 win=132,368
  → 窗口全程 1:1 跟随鼠标、松手后不再偏移                    ✅

# B. 面板不受影响（原版面板无 drag 区，不应可拖）
面板标题栏拖动 → rect 40,200 不变                             ✅

# C. 标题栏按钮仍可点（WindowMoveArea 不吞点击）
从右往左点击 → 最右为关闭（无反应/隐藏），其次为最大化（rect 变 1550x878），
再次为最小化（IsIconic=True）                                  ✅
```

### 回归测试

`cargo build --workspace` 零错误零警告（除 registry 增量锁文件删除的系统级无关警告）；
`cargo test --workspace` 全部通过（30 个测试二进制，0 失败）。交互类改动由上述真机脚本对拍，
不做自动化单测（Slint 无 headless 输入注入）。

### 已知差异 / 限制

- 离线构建必须运行一次 `tools/slint-winit-patch/apply.ps1`；否则 `with_window_msg_hook` 不存在，
  编译失败并直接指向该模块（不会静默退化）。
- 拖动时窗口被拖出屏幕外（例如整个窗口移到屏幕左侧之外）后再拖动标题栏，鼠标可点区域变窄，
  属于 Windows 正常语义，未做特殊处理。

## Round 19 · 2026-11-24：修复「设置下拉框样式只跟随系统深浅色」

### 症状

设置窗口里 std-widgets 下拉框（ComboBox，以及 LineEdit/ScrollView 等一切 fluent 标准控件）
的深浅色**只跟随 Windows 系统设置**：系统深色 + 应用选浅色主题时，自绘部分是浅色、
下拉框却是深色，两套配色脱节。Round 12 引入 ComboBox 后暴露。

### 根因

两套配色来源不同步：

| 配色域 | 来源 | 受控方 |
|---|---|---|
| 自绘控件（滑条/行/按钮等） | 自定义 `Theme` 全局（ui/theme.slint） | `apply_theme`/`apply_settings_theme` 按 settings.theme 设 `dark` |
| std-widgets（fluent 样式） | Slint 内置 `Palette` 全局的 `color-scheme` | **从未设置** → 保持 `Unknown` → 跟随系统 |

### 修复

1. **`ui/all.slint`**：`import { Palette } from "std-widgets.slint"; export { Palette }`
   （fluent 的 std-widgets.slint 导出该全局；不 re-export 则生成的 Rust API 不含它，
   实测 `export { Palette }` 裸写报 `'Palette' not found`）。
2. **`src/window_state.rs`**：新增 `palette_set_dark(&Palette, dark)` —— 按解析出的深浅色
   调 `Palette.set_color_scheme(Dark/Light)`（类型来自
   `slint::private_unstable_api::re_exports::ColorScheme`）。三个主题应用点
   （`apply_theme` 面板 / `apply_settings_theme` 设置窗口 / `apply_theme_generic` intro）全部接线。
   四窗口同属 all.slint 编译单元 → Palette 全局实例共享，设一次全生效；
   ComboBox 弹出层与宿主组件共享 globals，下拉列表同样跟随。
3. **`src/main.rs` ui-smoke**：新增 `palette color-scheme=... (expect ...)` 自检输出，
   断言方向为「与 Theme.dark 一致」。

### 真机对拍（2026-11-24 · Win11 系统深色）

```
# 系统深色 + 用户真实 settings（跟随系统）→ dark=true / palette Dark   ✅
.\target\debug\twinkle-tray-rust.exe --ui-smoke --console

# 系统深色 + --portable 临时 settings.json 强制 theme=light
#   → theme dark=false / palette color-scheme=Light (expect Light)      ✅
#   （修复前 palette 恒 Unknown，std-widgets 跟系统呈深色 —— 即本缺陷）
.\target\debug\twinkle-tray-rust.exe --ui-smoke --console --portable
```

cargo test --workspace 全绿（0 失败）；cargo build 零错误
（仅既有的 `target_os = "win32"` 死分支 warning，与本次无关）。

### 踩坑记录

- 内置全局 `Palette` 不是编译器裸作用域符号，必须从 `std-widgets.slint` import 后 re-export
  才会出现在 include_modules 的 Rust API 里。
- Rust 侧 `ColorScheme` 枚举未在 slint 顶层公开，位于
  `slint::private_unstable_api::re_exports::ColorScheme`（Unknown/Dark/Light）。

## Round 20 · 2026-11-25：程序图标（exe 资源嵌入 + 窗口图标）

### 目标

等价原版 electron-builder 的图标语义（origin/package.json build.win.icon = src/assets/logo.ico）：
exe 文件图标 + 文件元数据（VERSIONINFO）+ 各窗口（BrowserWindow.icon）标题栏/任务栏图标。

### 实现

| 部分 | 文件 | 要点 |
|---|---|---|
| exe 图标 + VERSIONINFO | `build.rs` | 动态生成资源脚本（版本号取 CARGO_PKG_VERSION，避免双源；纯 ASCII，图标路径相对 manifest 目录、rc.exe 以 manifest_dir 为 cwd 编译）→ SDK rc.exe 编译 .res → cvtres 转 COFF .obj → `cargo:rustc-link-arg-bins` 直接作为最终二进制链接输入。rc.exe/cvtres 缺失或失败时 `cargo:warning` 降级跳过，不阻塞构建 |
| 窗口图标 | `ui/{panel,settings,intro,hud}.slint` | Slint `Window.icon` 原生属性：`icon: @image-url("../assets/logo-square.png")`（编译期嵌入，无运行时资产依赖）。winit 后端在窗口创建/update_window_properties 时经 winit `set_window_icon` + Windows `set_taskbar_icon` 原生设置 |

### 踩坑记录（关键）

1. **`.res` 复制为 `.lib` 链接无效**：cargo 报 `Unsupported archive identifier`（.res 是资源表
   格式不是 COFF 档案）。正确链路是 cvtres 转 COFF obj。
2. **`rustc-link-lib=static` 拉入 COFF 档案也不行**：无符号引用的档案成员不被拉入（资源 obj
   无任何符号），图标不进 exe。必须 `cargo:rustc-link-arg-bins` 显式传给最终链接——显式 obj
   输入不走档案成员拉取规则，`.rsrc` 段必然合并。
3. **cvtres 绝对路径参数不可靠**：工程路径含非 ASCII 字符（G:\杂项\...）时 cvtres 在 ANSI
   代码页下无法打开目标文件（CVT1101）。让 cvtres 以 out_dir 为 cwd、用 ASCII 相对文件名工作。
4. **窗口图标放弃手写 WM_SETICON**：winit 后端的 raw-window-handle 实现
   （`window_handle_06_rc` → `winit_window_or_none.as_window()`）在窗口 show 且事件循环
   未跑起来前返回 `HandleError`，show 后 200ms 重试仍拿不到（winit 延迟创建窗口）。
   Slint `Window.icon` 属性是官方入口：窗口创建时即注入 winit `window_icon`，任务栏图标
   同步设置（`set_taskbar_icon`），无需自己碰 Win32。
5. rc.exe 对无 BOM 文件按**活动代码页**解码——资源脚本保持纯 ASCII（中文注释会乱码损坏）。

### 真机对拍（2026-11-25）

```
# exe 资源
[System.Drawing.Icon]::ExtractAssociatedIcon(exe) → 32x32 关联图标可用
(Get-Item exe).VersionInfo → desc=Twinkle Tray / prod=Twinkle Tray / ver=0.1.0 / company=Xander Frangos / trademark=Xander Frangos
exe 尺寸增长 ≈ +330KB（logo.ico 9 档嵌入），FileDescription/Twinkle Tray 的 UTF-16LE 串在 exe 中命中

# 窗口图标（--ui-demo 真机，EnumWindows + WM_GETICON(0x7F) 查询）
'调整亮度'      ICON_SMALL=0x1E00A63  ✅ 非零
'Twinkle Tray 设置' ICON_SMALL=0x9E10B31  ✅ 非零
（修复前两窗口恒 0x0；任务栏/Alt-Tab 显示 Twinkle Tray 图标）

# 回归
cargo build --workspace：零错误零警告（增量检查 clean）
cargo test --workspace：全部通过
```

### 等价性说明

- exe 图标/元数据：与 electron-builder 产物等价（同一 logo.ico 资源 + 同名字段 FileDescription/
  ProductName/CompanyName/LegalTrademarks，版本号同源 Cargo.toml ↔ package.json）。
- 窗口图标：原版每个 BrowserWindow 继承 build.win.icon；Rust 侧四窗口统一声明同一 PNG 资产
  （同一 logo 的 512px 方形版，ico 内 9 档位图由 winit/Slint 自行取档）。行为语义一致：
  窗口标题栏 + 任务栏 + Alt-Tab 均显示应用图标。
- 资源编译失败仅降级为「无图标 exe」（外观缺失），不影响任何功能路径（§5 出错降级不崩溃）。

