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

## 未完成（按 AGENTS.md §4 映射）

- [x] ~~`activewindow`~~（SetWinEventHook + 进程/包信息，AppxManifest 解析降级为 package family name，已记录论证）
- [x] ~~`sensors`~~（COM Sensor API，环境光；CLSID_SensorManager={77A1C827-FCD2-4689-8915-9D613CC5FA3E} 已核实）
- [x] ~~`material`~~（acrylic.cc SetWindowCompositionAttribute + DWM 属性齐备）
- [x] ~~Monitors worker~~（`--monitor-worker` 子命令 + DDC 哨兵 + 降级链 + IPC 行协议，全量消息分发）
- [ ] 主程序（settings 84 项 ✅ / 托盘 ✅ 运行时接线 / 热键解析 ✅ 注册待接 / 命令队列节流 ✅ / 事件源开关）
- [x] ~~Slint UI（panel / settings / intro / hud 四窗口）~~（Round 5 实质化完成，见上）
- [ ] HDR（并入 displayconfig，DisplayConfigSetDeviceInfo）

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