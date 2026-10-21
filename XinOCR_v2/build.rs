// 把 ScreenCapture 的静态库链进来。
//
// 静态库由 ..\ScreenCapture_v2\Src\ScreenCaptureLib.vcxproj 产出（ConfigurationType = StaticLibrary），
// 里面装着整套截图/标注窗口（Ling + Direct2D + Yoga）。Rust 侧只调 sc_api.h 里那几个 C 函数。
//
// 链接要点：
//   · 静态库不会自动带上自己的依赖 —— Ling.lib / Yoga.lib 必须在这儿显式再列一次；
//   · C++ 的静态构造函数（Ling 里那些全局对象、Direct2D 工厂的初始化）只有在目标文件
//     真的被链进来时才会跑到。C ABI 函数之间互相引用，链接器一般会整份拉入，
//     但为了稳妥用 whole-archive 把 ScreenCaptureLib 的 obj 全数保留；
//   · C++ 运行时（msvcprt / vcruntime / msvcrt）由 Rust 通过 link.exe 传递，
//     这里显式加上默认库，避免"未解析的外部符号 std::*"。

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // 允许用环境变量覆盖，便于在别的机器/CI 上换路径
    let root = std::env::var("SC_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("ScreenCapture_v2"));
    let sc = root.join("Src");

    // 产物目录：vcxproj 里 IntDir 指到 $(SolutionDir)$(Platform)\$(Configuration)LibTemp\，
    // 而静态库的输出目录（OutDir）默认是 $(SolutionDir)$(Platform)\$(Configuration)\
    let out = std::env::var("SC_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join("x64").join("Release"));

    let ling_lib = root.join("Ling").join("x64").join("Release");

    println!("cargo:rustc-link-search=native={}", out.display());
    println!("cargo:rustc-link-search=native={}", ling_lib.display());
    println!("cargo:rerun-if-changed={}", sc.join("sc_api.h").display());
    // 库文件可能还没构建出来（首次 cargo build 之前），不存在时别让 cargo 反复重跑
    let scl = out.join("ScreenCaptureLib.lib");
    if scl.is_file() {
        println!("cargo:rerun-if-changed={}", scl.display());
    }

    // 截图库本体 + 它的两个静态依赖（Ling 框架、Yoga flexbox）
    println!("cargo:rustc-link-lib=static:+whole-archive=ScreenCaptureLib");
    println!("cargo:rustc-link-lib=static=Ling");
    println!("cargo:rustc-link-lib=static=Yoga");

    // C++ 运行时：Rust 自己不带这些，静态库里的 std::wstring / std::vector 都要用到。
    // 不加 static= 前缀 —— 它们是 .lib 形式由链接器按默认库规则解析即可
    for lib in ["msvcprt", "vcruntime", "msvcrt", "oldnames"] {
        println!("cargo:rustc-link-lib={}", lib);
    }

    // 系统库：静态库里有 C++ 代码（Ling 用到了 cxx / WinRT），由 Rust 侧负责补齐
    for lib in [
        "dwmapi", "windowsapp", "comctl32", "imm32", "version", "ntdll", "userenv",
        "d2d1", "d3d11", "dxgi", "dwrite", "windowscodecs", "ole32", "oleaut32",
        "uuid", "user32", "gdi32", "shell32", "shlwapi", "advapi32", "comdlg32",
        "msimg32",
    ] {
        println!("cargo:rustc-link-lib={}", lib);
    }
}
