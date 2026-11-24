# 离线构建补丁：为 i-slint-backend-winit 1.19.0 增加 BackendBuilder::with_window_msg_hook。
#
# 背景（PROGRESS.md Round 16）：无边框窗口拖动/缩放时 Windows 进入模态循环，winit 的
# 光标消息泵在模态循环期间拿不到「有新消息」，Slint 的 50ms tick 停摆：拖动会跳一下
# 再断、松手后位置再飘一次。修复需要在 EventLoopBuilder 上装 winit 的 msg_hook；
# slint 1.19 已提供 BackendSelector::with_winit_event_loop_builder，本补丁只补包装后的入口。
#
# 用法（构建前执行一次，幂等）：pwsh -File tools/slint-winit-patch/apply.ps1
param(
    [string]$CargoHome = $env:CARGO_HOME
)
$ErrorActionPreference = 'Stop'
if (-not $CargoHome) { throw 'CARGO_HOME is not set' }
$crate = Get-ChildItem (Join-Path $CargoHome 'registry\src') -Directory |
    ForEach-Object { Get-ChildItem $_.FullName -Directory -Filter 'i-slint-backend-winit-1.19.0' } |
    Select-Object -First 1
if (-not $crate) { throw 'i-slint-backend-winit-1.19.0 not found under CARGO_HOME/registry/src' }
$lib = Join-Path $crate.FullName 'lib.rs'
$text = Get-Content -Raw -LiteralPath $lib
if ($text.Contains('with_window_msg_hook')) { Write-Output 'already patched'; exit 0 }

$method = (Get-Content -Raw (Join-Path $PSScriptRoot 'with_window_msg_hook.rs.txt')) + "`r`n"
$builderAnchor = "    #[must_use]`r`n    pub fn with_event_loop_builder(mut self, event_loop_builder: EventLoopBuilder) -> Self {"
if (-not $text.Contains($builderAnchor)) { throw 'builder anchor not found' }
$text = $text.Replace($builderAnchor, $method + $builderAnchor)

$fieldAnchor = "        Option<Box<dyn Fn(winit::window::WindowAttributes) -> winit::window::WindowAttributes>>,`r`n    renderer_name: Option<String>,"
$fieldRepl = "        Option<Box<dyn Fn(winit::window::WindowAttributes) -> winit::window::WindowAttributes>>,`r`n    window_msg_hook: Option<Box<dyn FnMut(*const core::ffi::c_void) -> bool>>,`r`n    renderer_name: Option<String>,"
if (-not $text.Contains($fieldAnchor)) { throw 'builder field anchor not found' }
$text = $text.Replace($fieldAnchor, $fieldRepl)

$initAnchor = "            window_attributes_hook: None,`r`n            renderer_name: None,"
$initRepl = "            window_attributes_hook: None,`r`n            window_msg_hook: None,`r`n            renderer_name: None,"
if (-not $text.Contains($initAnchor)) { throw 'builder init anchor not found' }
$text = $text.Replace($initAnchor, $initRepl)

$buildAnchor = '    pub fn build(self) -> Result<Backend, PlatformError> {'
if (-not $text.Contains($buildAnchor)) { throw 'build anchor not found' }
$text = $text.Replace($buildAnchor, '    pub fn build(mut self) -> Result<Backend, PlatformError> {')

$loopAnchor = '            self.event_loop_builder.unwrap_or_else(winit::event_loop::EventLoop::with_user_event);'
$loopRepl = $loopAnchor + "`r`n" + "`r`n        // 见 BackendBuilder::with_window_msg_hook。`r`n        if let Some(hook) = self.window_msg_hook.take() {`r`n            use winit::platform::windows::EventLoopBuilderExtWindows as _;`r`n            let mut hook = Some(hook);`r`n            event_loop_builder.with_msg_hook(move |raw| match hook.as_mut() {`r`n                Some(f) => f(raw),`r`n                None => false,`r`n            });`r`n        }"
if (-not $text.Contains($loopAnchor)) { throw 'event loop anchor not found' }
$text = $text.Replace($loopAnchor, $loopRepl)

$passAnchor = "            event_loop_state: Default::default(),`r`n            window_attributes_hook: self.window_attributes_hook,"
$passRepl = "            event_loop_state: Default::default(),`r`n            window_attributes_hook: self.window_attributes_hook,`r`n            window_msg_hook: self.window_msg_hook,"
if (-not $text.Contains($passAnchor)) { throw 'backend init anchor not found' }
$text = $text.Replace($passAnchor, $passRepl)

$backendAnchor = "    pub window_attributes_hook:`r`n        Option<Box<dyn Fn(winit::window::WindowAttributes) -> winit::window::WindowAttributes>>,"
$backendRepl = $backendAnchor + "`r`n`r`n    /// 见 BackendBuilder::with_window_msg_hook。`r`n    pub window_msg_hook: Option<Box<dyn FnMut(*const core::ffi::c_void) -> bool>>,"
if (-not $text.Contains($backendAnchor)) { throw 'backend field anchor not found' }
$text = $text.Replace($backendAnchor, $backendRepl)

Set-Content -LiteralPath $lib -Value $text -NoNewline
Write-Output "patched $lib"
