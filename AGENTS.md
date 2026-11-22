# AGENTS.md — Twinkle Tray Rust 重写规范

> 本文件约束 AI 在本仓库内的所有重写行为。规则按优先级排列：**安全 > 等价性 > 依赖政策 > 风格**。
> 与本文件冲突的口头指示，须先更新本文件再执行。

## 0. 项目目标

将本仓库（Electron + 8 个自研 C++ N-API 模块的 Windows 显示器亮度托盘工具，v1.18.0-beta2）
整体重写为 **Rust + Slint** 的原生应用。参照实现为 `sidit77/rusty-twinkle-tray`（XAML 路线），
但我们采用 Slint fluent 样式路线。

- 源架构：`src/electron.js`（主进程 6,431 行）+ `src/Monitors.js`（worker 子进程 2,299 行）+ React UI + 8 个 C++ 模块
- 目标架构：Rust 主程序 + Slint UI（fluent 样式）+ `windows` crate 直调 Win32/WinRT/COM
- **验收标准**：真机上与原版行为逐项对拍一致（见 §6）

## 1. 依赖政策（最重要规则）

**除非有必要，一律不使用外部 crates。**

### 1.1 判定标准

引入任何外部 crate 前，必须满足全部三条，并在 PR/提交说明中记录论证：

1. **必要性**：std + `windows` crate 无法合理实现，或自实现的风险（正确性/安全性）显著高于复用；
2. **等价性**：该 crate 不会改变或简化原有行为语义；
3. **最小化**：选择依赖树最窄的实现，禁止引入带全局运行时的库。

### 1.2 预批准白名单

| crate | 理由 | 备注 |
|---|---|---|
| `windows` | Win32/COM/WinRT 官方绑定，FFI 唯一正解 | feature 按需开启，禁止 `features = []` 全量引入 |
| `slint` | UI 框架，用户指定 | backend-winit + renderer-femtovg |
| `serde` + `serde_json` | settings.json 兼容解析 | 手写 JSON 解析器的转义/Unicode 边角风险不可控，必要性成立 |

**此外一切 crate 均需事前申请。** 以下原方案中的候选已被本规则否决，对应功能改为自研等价实现：

| 被否决 | 替代实现 |
|---|---|
| `tokio` | `std::thread` + channel；Win32 消息循环跑专用线程（原项目即单线程事件驱动） |
| `reqwest` | WinHTTP（`Win32_Networking_WinHTTP`），仅 GA4 埋点/坐标/更新下载三处 |
| `wmi` | 等价移植 `src/modules/wmi-bridge/wmi-bridge.cc`（COM 直调 IWbemServices） |
| `winreg` | `Win32_System_Registry` 直调（原 `native-reg` 即此实现） |
| `tray-icon` / `betrayer` | `Shell_NotifyIconW` 自写托盘 + HMENU 弹出菜单 |
| `global-hotkey` | `RegisterHotKeyW`（等价 Electron globalShortcut） |
| `window-vibrancy` | 等价移植 `acrylic.cc`（SetWindowCompositionAttribute）+ `windows_window_material.cc`（DwmSetWindowAttribute） |
| `rdev` | `SetWindowsHookExW(WH_MOUSE_LL)` 低级鼠标钩子 |
| `suncalc` | suncalc 为 ~200 行纯数学，按原 JS 逐函数移植 |
| `tiny-skia`/`fontdue` | **禁止自绘图标**——原版托盘图标是预生成资产（`src/assets/tray-icons/{dark,light}/`，fluent/mdl2/icon 三套 × 多档缩放），直接加载现有 ico/png |
| `nusb` | Studio Display USB 控制传输先尝试用 WinUSB 直调等价移植；仅当工作量不可控时才申请引入，须记录论证 |

### 1.3 源码内模块

原生功能一律放进 workspace 内的 crate（参照原 `src/modules/*` 与 rusty-twinkle-tray 的 `lib/` 布局），
不得为"省事"把 Win32 调用散落在 UI/业务代码里。

## 2. 本地化：JSON → FTL

`src/localization/*.json`（38 种语言 × 304 键）转换为 FTL 格式。

### 2.1 事实约束（已核实，转换器必须按此实现）

- 源 JSON 为**单层平铺**：`{ "KEY": "text" }`，无嵌套对象；
- 占位符统一为 `{{1}}` 形式（数字从 1 开始，当前语料仅有 {{1}}，转换器仍须支持 {{N}}）；
- 值内**不存在** {{N}} 之外的花括号（已扫描确认），无需花括号转义分支；
- 19 个键的值含 `'`、`"`、`&` 等字符——FTL 值内原样保留，不做转义；
- 最长值 356 字符（`SETTINGS_FEATURES_DDC_BLOCK_DESC`），须支持 FTL 多行续行；
- 值中如出现 ` = `（首个分隔符之外），原样保留在值内——解析按**首个** ` = ` 切分。

### 2.2 转换规则

```ftl
# en.ftl
GENERIC_OK = OK
SETTINGS_UPDATES_VERSION = You have Twinkle Tray {{1}}.
SETTINGS_FEATURES_DDC_BLOCK_DESC = <首行>
    <续行缩进 4 空格，FTL 多行消息；转换器按需折行，语义等价即可>
```

1. **格式**：一条消息 = `KEY = VALUE`，保持"键=值"简单形式。**禁止使用 FTL 高级特性**（selector/variant/plural/attribute/术语），源语料不含这些结构。
2. **占位符**：`{{1}}` **原样保留**，不转换为 FTL 标准 `{ $1 }`。理由：
   - 原 `Translate.js` 的 `makeTranslation()` 即按 `{{N}}` 顺序替换，原样保留可逐字节复刻该语义；
   - 避免 fluent-runtime 系列外部 crate（违反 §1）。
3. **键名**：原键名为 `[A-Z0-9_]+`，是合法 FTL identifier，原样保留。
4. **文件命名**：按 BCP-47 规范化，映射表（唯一两个非平凡项）：
   - `zh_Hans.json` → `zh-Hans.ftl`（下划线改连字符）
   - `en-GB.json` → `en-GB.ftl`；其余 `<lang>.json` → `<lang>.ftl`
5. **产物位置**：`src/localization/*.ftl`，与源 JSON 并存（源 JSON 是唯一事实来源，FTL 是生成物）。
6. **转换器**：一次性脚本（Node 或 Rust 皆可），支持 `--check` 模式做双向 diff 校验（304 键零丢失、值逐字节相等、占位符数量一致）。转换产物提交入库，脚本不进入构建链。
7. **运行时加载**（等价 `Translate.js` 语义，自写解析器约 50 行，禁止引入 fluent crate）：
   - 解析：逐条消息按首个 ` = ` 切分；多行续行拼接；忽略 `#` 注释与空行；
   - **回退链**：当前语言缺键**或值为空** → 用 `en.ftl` 回退 → 仍无则返回空串（与 `Translate.js.getString` 一致）；
   - 替换：`t(key, args)` 按 `{{1}}, {{2}}, ...` 顺序替换；args 耗尽后剩余占位符**原样保留**（原行为如此，不得"修复"）。

## 3. 逻辑等价性原则

**不得简化原有逻辑，但可以替换成对应的等价写法。**

### 3.1 边界定义

- ✅ **等价替换**（允许）：语法层面改写，输入输出与可观察行为（时序、重试、降级、副作用）不变。
  例：JS Promise 链 → 显式状态机；`for` 循环 → iterator；`child_process.fork` → 同二进制 `--monitor-worker` 子命令自调用；`try/catch` → `Result` 逐层传播。
- ❌ **简化**（禁止）：删除分支、合并重试、缩短降级链、去掉防御性代码、把常量改成"看起来更合理"的新值。

### 3.2 必须原样保留的清单（易被 AI"顺手优化"的高危区）

| 原逻辑 | 位置 | 为什么不能简化 |
|---|---|---|
| DDC 崩溃哨兵：sentinel 文件写 stage 证据 → 启动时检测 → 自动降级 → `ddc-unstable.json` 拉黑 | `Monitors.js:329-455` | 原生调用可致整个 worker 进程崩溃，这是唯一的自愈机制 |
| 亮度方法降级链：DDC → 原生 WMI 桥 → WMIC.exe → gamma 软件亮度 | `Monitors.js` 全文 | WMIC 在 Win11 部分版本仍存在，**保留** `wmicExists()` 检测与失败计数切换（此前"抛弃 WMIC"的提法作废） |
| worker 崩溃隔离 + 自动重启 + 30s 失败标志复位 | `electron.js:345-495` | DDC 崩溃不能带倒主进程 |
| 全部时序/数值常量：`updateInterval=500`、`checkVCPWaitMS=20`、`SOFTWARE_BRIGHTNESS_MIN=20`、Studio Display nits 400–60000、重试/超时值 | `Monitors.js:1004-1010` 等 | 均为真机调校结果，等价移植时逐字保留 |
| `defaultSettings` 全部 84 项及版本迁移逻辑 | `electron.js` | settings.json 必须与原版双向兼容（老用户配置文件可直接用） |
| 显示器三源 hwid 对齐逻辑（win32/WMI/DDC 标识合并） | `Monitors.js` refresh 流程 | 错位会导致控制错显示器 |
| linked features（对比度/色温等跟随亮度）、extended-minimum 断点、gamma ramp 读回 | `electron.js:1475-1650`、`Monitors.js` | 行为语义核心 |
| Windows 空闲检测、全屏检测、媒体播放检测联动 | `electron.js:5499-5625` | 联动条件之间是"与"关系，删任一分支改变行为 |
| 命令队列 + 节流 + flush 时序 | `electron.js:168-291` | DDC 写入过频会失灵，节流是功能的一部分 |
| 双事件源开关（`useWin32Event` / `useElectronEvents` / `useWmDisplayChangeEvent` 等设置项） | `electron.js:5210+` | 是用户可配的故障排查手段，不是冗余代码 |

### 3.3 对拍验证义务

每个功能模块完成移植后，须真机验证并在提交说明中记录对拍结果：
同显示器、同操作序列，新旧版本的可观察行为（读数、时序、降级路径）一致。
无法自动化对拍的（如 DDC 异常恢复），人工执行并记录步骤。

## 4. 架构映射

| 原模块 | Rust 目标 crate（workspace 内） | 要点 |
|---|---|---|
| `src/modules/node-ddcci` (1,666 行 C++) | `ddcci` | `Win32_Devices_Display`：物理监视器枚举 + High/Low Level Monitor Configuration API；等价移植全部 capabilities 解析 |
| `src/modules/win32-displayconfig` | `displayconfig` | Query/SetDisplayConfig、刷新率上下文、显示变化侦听（message-only window） |
| `src/modules/wmi-bridge` | `wmibridge` | COM 直调，等价移植 .cc 逐函数 |
| `src/modules/tt-windows-utils` | `twutils` | gamma ramp、Raw Input 亮度键、电源事件、WinRT 媒体、StartupTask、窗口工具六个子模块 |
| `src/modules/windows-hdr` | 并入 `displayconfig` | DisplayConfigSetDeviceInfo |
| `src/modules/windows-ambient-sensor` | `sensors` | COM Sensor API |
| `src/modules/acrylic` + `windows_window_material` | `material` | 亚克力/Mica，DwmSetWindowAttribute |
| `src/modules/node-active-window` | `activewindow` | SetWinEventHook + 进程/包信息；AppxManifest 解析如工作量不可控，降级为仅读取可执行路径（记录论证） |
| `Monitors.js` worker | 主程序 `--monitor-worker` 模式 | 哨兵机制原样移植 |
| React UI（4,084 行 JSX + 2,891 行 SCSS） | `.slint` 文件（fluent 样式） | panel / settings / intro / hud 四窗口；**重写而非照抄布局**，但功能项与交互语义一一对应，不得缺项 |

## 5. 编码规范

- **unsafe 边界**：所有 `unsafe` FFI 调用集中在模块 crate 内部，对外暴露安全接口；每个 unsafe 块附一行注释说明不变量。
- **错误处理**：FFI 层返回 `Result`，禁止 `unwrap`/`expect` 出现在业务路径（原 JS 的 try/catch 语义 = 捕获并降级，不是崩溃）。原代码"出错则返回 false/跳过"的位置，Rust 侧必须是降级分支而非 panic。
- **崩溃隔离**：worker 子进程内禁止依赖主进程状态；IPC 用 JSON 行协议（等价 `fork` 的 message 通道），消息类型与原版一一对应（`ready`/`settings`/`ddcBrightnessVCPs`/`wmi-bridge-ok`/...）。
- **线程模型**：主线程 = Slint 事件循环；Win32 消息循环（托盘/热键/电源/显示变化）跑专用线程；`RoInitialize(RO_INIT_SINGLETHREADED)` 仅在需要 WinRT 的线程调用。
- **命名**：模块/文件名对应原 C++ 模块名；函数名尽量保留原 JS 函数名（`refreshMonitors`/`updateBrightness`...），便于对拍时逐行对照。
- **注释**：等价移植的函数头部标注原位置（`// equiv: src/electron.js:L1234-L1290`）。

## 6. 提交纪律

1. 一个模块一个 PR，先骨架（编译通过 + 接口签名与 §4 映射一致）后实现。
2. 每个实现 PR 的说明必须包含：等价性论证（改了哪些写法、为什么行为不变）+ 依赖变更（新增 crate 必须有 §1.1 三条论证）。
3. 禁止一次性大爆炸提交；禁止在实现 PR 里顺手改规范（规范改动单独 PR 修改本文件）。

## 7. 快速禁止事项（速查）

- [ ] 不经论证引入白名单外 crate
- [ ] 把 `{{1}}` 改成 FTL `{ $var }` 语法或引入 fluent 库
- [ ] 使用 FTL selector/plural 等高级特性
- [ ] 删除任何 §3.2 清单中的防御逻辑、降级分支或常量
- [ ] 用"更现代"的实现替换原有时序（sleep/轮询/重试参数）
- [ ] 自绘托盘图标（用现有资产）
- [ ] 业务代码里出现 `unwrap()`/`panic!` 代替原有降级路径
- [ ] 缺项移植 UI 功能（设置页每个开关都要有对应物）
- [ ] 跳过对拍验证直接宣称"完成"
