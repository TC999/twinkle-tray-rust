@echo off
rem Rust/MSVC 构建环境(rustup 安装在 D:\Software\Rust,VS 重定向到 D:\Software\Visual Studio)
set "RUSTUP_HOME=D:\Software\Rust\.rustup"
set "CARGO_HOME=D:\Software\Rust\.cargo"
set "MSVC=D:\Software\Visual Studio\VC\Tools\MSVC\14.41.34120"
set "SDK=C:\Program Files (x86)\Windows Kits\10"
set "SDKV=10.0.22621.0"
set "PATH=%CARGO_HOME%\bin;%MSVC%\bin\Hostx64\x64;%PATH%"
set "INCLUDE=%MSVC%\include;%SDK%\Include\%SDKV%\ucrt;%SDK%\Include\%SDKV%\um;%SDK%\Include\%SDKV%\shared;%SDK%\Include\%SDKV%\winrt;%SDK%\Include\%SDKV%\cppwinrt"
set "LIB=%MSVC%\lib\x64;%SDK%\Lib\%SDKV%\ucrt\x64;%SDK%\Lib\%SDKV%\um\x64"
