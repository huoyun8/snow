# build.ps1 —— 整合版构建脚本（XinOCR_v2 + ScreenCapture_v2 静态库）
#
# 在「同时包含 XinOCR_v2/ 与 ScreenCapture_v2/ 两个目录」的目录下运行：
#     .\build.ps1
#
# 前置环境：
#   · Visual Studio（勾选「使用 C++ 的桌面开发」+ Windows 10/11 SDK）
#   · Rust（stable-x86_64-pc-windows-msvc）；本机若没装会用 rustup 切过去
#
# 产物：XinOCR_v2\target\release\xin-ocr.exe（单文件，内含 OCR 引擎资产）

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $MyInvocation.MyCommand.Definition
$sc   = Join-Path $root "ScreenCapture_v2"
$xin  = Join-Path $root "XinOCR_v2"

function Run([string]$exePath, [string[]]$argsArr) {
    Write-Host ">> $exePath $($argsArr -join ' ')"
    & $exePath @argsArr
    if ($LASTEXITCODE -ne 0) { throw "命令失败 (exit=$LASTEXITCODE): $exePath $($argsArr -join ' ')" }
}

# 1) yoga.lib（Ling 的 flexbox 依赖）
Run "msbuild" @(
    "$sc\Ling\yoga\yoga.vcxproj",
    "/p:Configuration=Release", "/p:Platform=x64",
    "/p:SolutionDir=$sc\Ling\", "/nologo", "/v:minimal"
)

# 2) Ling.lib（C++/WinRT + Direct2D 框架）
Run "msbuild" @(
    "$sc\Ling\Ling.vcxproj",
    "/p:Configuration=Release", "/p:Platform=x64",
    "/p:SolutionDir=$sc\Ling\", "/nologo", "/v:minimal"
)

# 3) ScreenCaptureLib.lib（截图/标注窗口本体，静态库）
#    OutDir 必须落到 ScreenCapture_v2\x64\Release —— build.rs 默认到这个目录找它；
#    SolutionDir 必须传，因为 vcxproj 的 IntDir 用了 $(SolutionDir)。
Run "msbuild" @(
    "$sc\Src\ScreenCaptureLib.vcxproj",
    "/p:Configuration=Release", "/p:Platform=x64",
    "/p:SolutionDir=$sc\", "/p:OutDir=$sc\x64\Release\",
    "/nologo", "/v:minimal"
)

# 4) 确保 Rust 工具链（若本机已是 msvc 目标则这步无副作用）
try {
    Run "rustup" @("default", "stable-x86_64-pc-windows-msvc")
} catch {
    Write-Host "（rustup 不可用或已就位，跳过）"
}

# 5) cargo build —— build.rs 会自动去找上面三个 .lib 并链进来
Run "cargo" @("build", "--release", "--manifest-path", "$xin\Cargo.toml")

$exe = Join-Path $xin "target\release\xin-ocr.exe"
if (Test-Path $exe) {
    Write-Host ""
    Write-Host "BUILD OK -> $exe"
} else {
    throw "未找到产物 $exe"
}
