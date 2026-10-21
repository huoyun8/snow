#![windows_subsystem = "windows"]

// 截图能力：链进 ScreenCapture 的静态库（见 build.rs），这里只是薄薄一层 Rust 绑定。
// 框选、放大镜、选区调整、标注工具条全在库里，本程序不再自绘遮罩窗口。
mod sc;

use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use winapi::ctypes::c_void;

use qrcode::{QrCode, EcLevel};
use rqrr::PreparedImage;
use image::GenericImageView;

use winapi::shared::minwindef::{WPARAM, LPARAM, LRESULT, UINT, HKEY};
use winapi::shared::windef::{HWND, RECT, HBRUSH, HMENU, POINT};
use winapi::um::winnt::{KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_SZ};
use winapi::um::errhandlingapi::GetLastError;
use winapi::um::libloaderapi::{GetModuleFileNameW, GetModuleHandleW, GetProcAddress};
use winapi::shared::minwindef::FILETIME;
use winapi::um::processthreadsapi::{GetExitCodeProcess, GetProcessTimes, OpenProcess, TerminateProcess};
use winapi::um::synchapi::CreateMutexW;
use winapi::um::memoryapi::{CreateFileMappingW, MapViewOfFile, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE};
use winapi::um::winnt::PAGE_READWRITE;
use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
use winapi::um::winbase::{GlobalAlloc, GlobalFree, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use winapi::um::reason::SHTDN_REASON_MAJOR_OTHER;
use winapi::um::commdlg::{GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW};
use winapi::um::winreg::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY_CURRENT_USER,
};
use winapi::um::tlhelp32::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
    TH32CS_SNAPPROCESS,
};
use winapi::um::shellapi::{Shell_NotifyIconW, NOTIFYICONDATAW, NIM_ADD, NIM_DELETE, NIF_ICON, NIF_MESSAGE, NIF_TIP};
use winapi::um::winuser::{
    AppendMenuW, BeginPaint, BS_OWNERDRAW, BS_PUSHBUTTON, CreatePopupMenu, CreateWindowExW,
    CS_HREDRAW, CS_VREDRAW, DefWindowProcW, DestroyMenu, DispatchMessageW,
    DRAWITEMSTRUCT, DrawTextW, DT_CENTER, DT_SINGLELINE, DT_VCENTER, EmptyClipboard,
    EndPaint, EnumWindows, FindWindowW, GetAsyncKeyState, GetCursorPos, GetDlgItem, GetMessageW,
    GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, IsWindow,
    IDC_ARROW, LoadCursorW, LoadIconW, MF_CHECKED, MF_STRING, MOD_ALT, MOD_CONTROL, MOD_SHIFT,
    MOD_WIN, ODS_SELECTED, OpenClipboard, PAINTSTRUCT, PostQuitMessage, RegisterClassExW,
    RegisterHotKey, SetForegroundWindow, SetWindowTextW, SW_HIDE, SW_RESTORE, IsIconic,
    SW_SHOW, ShowWindow, SendMessageW, SetClipboardData, CloseClipboard, TrackPopupMenu,
    TranslateMessage, TPM_BOTTOMALIGN, TPM_RETURNCMD, TPM_RIGHTBUTTON, UnregisterHotKey,
    WM_HOTKEY, WM_CLOSE, WS_CHILD, WS_VISIBLE, WM_COMMAND, WM_CREATE,
    WM_DESTROY, WM_DRAWITEM, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONUP, WM_MOUSELEAVE, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_USER,
    WM_CTLCOLORSTATIC, GetDlgCtrlID,
    WNDCLASSEXW, DestroyWindow, FillRect, GetClientRect, GetDC, GetWindowRect,
    InvalidateRect, ReleaseDC, TrackMouseEvent, TME_LEAVE,
    TRACKMOUSEEVENT, UpdateWindow, MSG, ES_AUTOHSCROLL, WS_BORDER, WS_POPUP,  VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    WS_CLIPCHILDREN,
    AdjustWindowRect, SetProcessDPIAware, SetWindowLongPtrW, GetWindowLongPtrW, GWLP_USERDATA,
    WM_SETFONT, VK_RETURN, PostMessageW, SetFocus, SetTimer, KillTimer, WM_TIMER, SetWindowPos,
    ES_MULTILINE, ES_AUTOVSCROLL, ES_WANTRETURN, SWP_NOMOVE, SWP_NOZORDER, GWL_STYLE,
    ExitWindowsEx, WM_SYSCOMMAND, BM_SETCHECK, BM_GETCHECK,
    BS_AUTORADIOBUTTON, BS_AUTOCHECKBOX, EWX_POWEROFF,
    EnableWindow,
    MessageBoxW, MB_OK, MB_ICONINFORMATION,
    ES_CENTER, WS_GROUP,
    PeekMessageW, PM_REMOVE, WM_QUIT,
    SS_CENTER, SS_CENTERIMAGE,
    SetLayeredWindowAttributes,
};
use winapi::um::wingdi::{
    CreateRoundRectRgn, CreateSolidBrush, CreateFontW, CreateBitmap, DeleteObject, FillRgn,
    SelectObject, SetBkMode, SetTextColor, StretchBlt, FW_BOLD, FF_SWISS, OUT_DEFAULT_PRECIS,
    CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, VARIABLE_PITCH, TRANSPARENT,
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, SRCCOPY, GetDeviceCaps, LOGPIXELSX,
    CLEARTYPE_QUALITY, SelectClipRgn,
};

// DWM 合成接口：DwmFlush 让合成器立即应用隐藏/移动的窗口，避免抓屏还带旧画面
#[link(name = "dwmapi")]
extern "system" {
    fn DwmFlush() -> i32;
}

const TARGET_PROCESSES: &[&str] = &[
    // 主流解压缩软件
    "7zFM.exe", "7z.exe", "7zz.exe",
    "WinRAR.exe", "Rar.exe", "UnRAR.exe",
    "Bandizip.exe", "HaoZip.exe", "360zip.exe",
    "PeaZip.exe", "NanaZip.exe", "WinZip.exe",
    "IZArc.exe", "PowerArchiver.exe", "BreeZip.exe",
    "2345Zip.exe", "Kuaizip.exe", "WinMount.exe",
    "uzip.exe", "Zipper.exe", "ExtractNow.exe",
];

const WM_TRAYICON: UINT = WM_USER + 1;
const ID_TRAY_SHOW: usize = 2001;
const ID_TRAY_EXIT: usize = 2002;
const ID_TRAY_AUTOSTART: usize = 2003;
const ID_TRAY_SETTINGS: usize = 2005;

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "Xin OCR";

// 快捷键设置存 HKCU\Software\Xin OCR\Settings
const SETTINGS_KEY: &str = "Software\\Xin OCR";
const HOTKEY_REC_VALUE: &str = "HotkeyRec";   // 识别（跟随当前页）  默认 F1
const HOTKEY_SHOW_VALUE: &str = "HotkeyShow"; // 显示/隐藏            默认 F10
const WM_HOTKEY_REC_ID: i32 = 104;
const WM_HOTKEY_SHOW_ID: i32 = 102;

// 二维码窗口控制ID（名称与按钮功能一一对应）
const QR_EDIT: usize = 4101;
const QR_BTN_DOWNLOAD: usize = 4102; // 下载二维码
const QR_BTN_SCREEN: usize = 4103;   // 识别二维码
const QR_BTN_COPY: usize = 4105;     // 复制二维码
const QR_BTN_UPLOAD: usize = 4106;   // 上传 LOGO
const QR_BTN_CLEAR: usize = 4107;    // 清空

// 文字识别页控件
const OCR_EDIT: usize = 4190;      // 大大的识别结果框（多行）
const OCR_SEARCH_EDIT: usize = 4194; // 顶部搜索框（快速定位）
const OCR_BTN_SEARCH: usize = 4195;  // 搜索框右侧“查找”按钮
const OCR_LBL_COUNT: usize = 4196;   // 识别字符数显示（“N 字符”，仅文字识别页显示）

// 窗口客户区基准尺寸（585×445 @100%），随 DPI 缩放
const CW: i32 = 585;
const CH: i32 = 445;
const OCR_BTN_REC: usize = 4191;   // 识别（框选位置→OCR）
const OCR_BTN_COPY: usize = 4192;  // 复制
const OCR_BTN_CLEAR: usize = 4193; // 清空

// 设置页：关闭方式
const SET_RAD_EXIT: usize = 5230;  // 关闭时退出（默认）
const SET_RAD_TRAY: usize = 5231;  // 关闭时到托盘
const SET_LBL_CLOSE: usize = 5232; // “关闭按钮：”标签
const CLOSE_MODE_VALUE: &str = "CloseMode"; // 0=退出 1=到托盘
const SET_LBL_REC_HIDE: usize = 5233; // “识别时隐藏主界面：”标签
const SET_RAD_HIDE_YES: usize = 5234; // 识别时隐藏主界面（默认）
const SET_RAD_HIDE_NO: usize = 5235;  // 识别时不隐藏主界面
const HIDE_ON_REC_VALUE: &str = "HideOnRec"; // 0=不隐藏 1=隐藏(默认)
const SET_CHK_ENGINE: usize = 5236;        // “识别引擎常驻”勾选框
const KEEP_ENGINE_VALUE: &str = "KeepEngine"; // 0=每次识别后退出(默认) 1=常驻复用

// 页面切换标签（顶部，共4个）
const QR_TAB_OCR: usize = 4119;   // “文字识别”页
const QR_TAB_QR: usize = 4120;    // “二维码”页
const QR_TAB_SHUT: usize = 4121;  // “定时关机”页
const QR_TAB_ANTI: usize = 4123;  // “防多开”页
const QR_TAB_SET: usize = 4122;   // “设置”页
const PAGE_OCR: i32 = 0;
const PAGE_QR: i32 = 1;
const PAGE_SHUT: i32 = 2;
const PAGE_SET: i32 = 3;
const PAGE_ANTI: i32 = 4;
const ANTI_BTN_CLOSE: usize = 4700; // “全部关闭”：关闭所有压缩软件窗口

// 定时关机页控件
const SD_TIMER_ID: usize = 98;     // 定时关机倒计时定时器
// 倒计时输入
const SD_ED_DAY: usize = 4520;
const SD_ED_H: usize = 4521;
const SD_ED_M: usize = 4522;
const SD_ED_S: usize = 4523;
// 上下调节箭头（▲▼）
const SD_UP_DAY: usize = 4600; const SD_DN_DAY: usize = 4601;
const SD_UP_H: usize = 4602;   const SD_DN_H: usize = 4603;
const SD_UP_M: usize = 4604;   const SD_DN_M: usize = 4605;
const SD_UP_S: usize = 4606;   const SD_DN_S: usize = 4607;
// 按钮
const SD_START: usize = 4560;   // 开始定时器（蓝）
const SD_PAUSE: usize = 4561;   // 暂停定时器（灰）
const SD_CLEAR: usize = 4563;   // 清空（灰，复位时间并停止倒计时）
// 标签
const SD_LBL_DAY: usize = 4584;    // “天数”
const SD_LBL_HOUR: usize = 4585;   // “小时数”
const SD_LBL_MIN: usize = 4586;    // “分钟数”
const SD_LBL_SEC: usize = 4587;    // “秒数”

// 设置页控件（两行：识别(跟随当前页) / 显示隐藏）
const SET_LBL_REC: usize = 5221;
const SET_LBL_SHOW: usize = 5224;
const SET_COMBO_REC: usize = 5211;
const SET_COMBO_SHOW: usize = 5208;
const SET_RECORD_REC: usize = 5213;
const SET_RECORD_SHOW: usize = 5207;
const SET_HINT: usize = 5205;
const SET_TIMER_ID: usize = 99;

// 当前悬停的按钮（1001=隐藏，1002=关闭，0=无）
static BTN_HOVER: AtomicI32 = AtomicI32::new(0);

// 主窗口句柄
static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

// 当前已生成的二维码图像 (w,h,BGRA)，识别结果文本，状态文本
static QR_IMG: Mutex<Option<(u32, u32, Vec<u8>)>> = Mutex::new(None);
static QR_TEXT: Mutex<String> = Mutex::new(String::new());

// 上传的 Logo (w,h,BGRA)，有值时叠加到二维码中央（白色圆角矩形底）
static LOGO_IMG: Mutex<Option<(u32, u32, Vec<u8>)>> = Mutex::new(None);

// 截图框选：遮罩窗口、选区状态、整屏截图缓冲全部搬进了 ScreenCapture 静态库（sc_api），
// 这里只留"本次要做什么"的意图标记，供 sc_on_done 回调判断走二维码还是 OCR。
static SEL_MODE: AtomicI32 = AtomicI32::new(1); // 0=仅二维码, 1=仅文字OCR, 2=先二维码无则文字
static SEL_QR_HIT: AtomicBool = AtomicBool::new(false); // 模式2：本次是否识别到二维码

// 识别结果消息：工作线程识别完成后通知主线程回填输入框并刷新二维码
const WM_QR_RESULT: UINT = 0x0400 + 100;

// 自定义快捷键录制（两快捷键：识别(跟随当前页) / 显示隐藏）
static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURE_TARGET: AtomicUsize = AtomicUsize::new(0); // 0=识别, 1=显示/隐藏
static CUSTOM_REC: Mutex<Option<(u32, u32, String)>> = Mutex::new(None);
static CUSTOM_SHOW: Mutex<Option<(u32, u32, String)>> = Mutex::new(None);

// 文字识别页结果文本
static OCR_TEXT: Mutex<String> = Mutex::new(String::new());

// 截图识别的一次性识别结果（与二维码当前内容 QR_TEXT 分开，避免污染）
static RECOG_RESULT: Mutex<String> = Mutex::new(String::new());

// DPI 感知：启动时按屏幕缩放系数把布局坐标等比放大，避免高分屏拉伸变形
static DPI_CACHE: AtomicI32 = AtomicI32::new(96);

// 输入框字体句柄（退出时 DeleteObject）
static EDIT_FONT: AtomicIsize = AtomicIsize::new(0);

// 二维码输入框高度（未缩放基准，使用处再套 sc）。固定值 —— 原版的"随内容自动增高"
// 早已被改成恒定 101，留着那个 AtomicI32 只会让人误以为它会变，故改为常量
const INPUT_H: i32 = 101;

// 两个全局快捷键的注册结果，bit0 = 识别，bit1 = 显示/隐藏。
// 启动时和每次改完快捷键都会刷新，进设置页时据此决定提示行文案（见 hotkey_status_text）
static HOTKEY_OK: AtomicI32 = AtomicI32::new(0b11);

// 当前页面：0=文字识别, 1=二维码, 2=定时关机, 3=设置
static PAGE_CUR: AtomicI32 = AtomicI32::new(0);
// 定时关机运行状态
static SD_RUNNING: AtomicBool = AtomicBool::new(false);
static SD_PAUSED: AtomicBool = AtomicBool::new(false);
static SD_REMAIN: AtomicI64 = AtomicI64::new(0); // 剩余秒数
static SD_ALERT_SHOWN: AtomicBool = AtomicBool::new(false); // 最后10秒弹出提示是否已触发
static SD_MAX: AtomicI64 = AtomicI64::new(0); // 本次总时长（进度条用）
static SD_END: AtomicI32 = AtomicI32::new(0); // 0=无 1=已完成 2=已取消
static SD_LABEL_BG: AtomicUsize = AtomicUsize::new(0); // 天/时/分/秒 标签芯片底色（懒建）
static SET_FONT: AtomicUsize = AtomicUsize::new(0); // 设置页统一字体（懒建）
static HINT_COUNT: AtomicUsize = AtomicUsize::new(0); // “请设置计时器”轻提示同时条数（最多5）

fn dpi() -> i32 {
    let d = DPI_CACHE.load(Ordering::Relaxed);
    if d <= 0 { 96 } else { d }
}
// 把 96dpi 基准坐标按当前缩放换算成实际像素（窗口固定 590×450，控件不放大）
fn sc(v: i32) -> i32 {
    (v * dpi()) / 96
}

fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

// 返回进程创建时间（FILETIME 合并为 u64），失败返回 0
fn proc_creation_time(pid: u32) -> u64 {
    unsafe {
        let h = OpenProcess(0x1000, 0, pid); // PROCESS_QUERY_LIMITED_INFORMATION
        if h.is_null() { return 0; }
        let mut ct: FILETIME = std::mem::zeroed();
        let mut et: FILETIME = std::mem::zeroed();
        let mut kt: FILETIME = std::mem::zeroed();
        let mut ut: FILETIME = std::mem::zeroed();
        let ok = GetProcessTimes(h, &mut ct, &mut et, &mut kt, &mut ut);
        winapi::um::handleapi::CloseHandle(h);
        if ok != 0 {
            ((ct.dwHighDateTime as u64) << 32) | (ct.dwLowDateTime as u64)
        } else {
            0
        }
    }
}

// 枚举窗口回调：收集 (所属PID, 窗口标题)
extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> i32 {
    unsafe {
        let vec_ptr = lparam as *mut Vec<(u32, String)>;
        if vec_ptr.is_null() { return 1; }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid == 0 || IsWindowVisible(hwnd) == 0 { return 1; }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), 512);
        if n > 0 {
            let title = String::from_utf16_lossy(&buf[..n as usize]).trim().to_string();
            if !title.is_empty() {
                (*vec_ptr).push((pid, title));
            }
        }
        1
    }
}

// 从窗口标题提取压缩包名：去掉尾部 " - Bandizip" 等
fn archive_identity(title: &str) -> String {
    if let Some(idx) = title.find(" - ") {
        title[..idx].trim().to_string()
    } else {
        title.trim().to_string()
    }
}

// RGB 转 COLORREF（0xBBGGRR）
fn rgb(r: u32, g: u32, b: u32) -> u32 {
    (b << 16) | (g << 8) | r
}

// ================= 统一配色（科学和谐的现代蓝体系，全局单一来源） =================
const C_PRIMARY: (u32, u32, u32) = (56, 103, 224);    // 主操作蓝 #3867E0
const C_DANGER: (u32, u32, u32) = (228, 72, 72);      // 危险红 #E44848
const C_DANGER_D: (u32, u32, u32) = (190, 50, 50);    // 红·按下 #BE3232
const C_DANGER_L: (u32, u32, u32) = (240, 110, 110);  // 红·悬停 #F06E6E
const C_TEXT: (u32, u32, u32) = (45, 57, 74);         // 主文字 #2D394A
const C_BORDER: (u32, u32, u32) = (208, 216, 228);    // 边框 #D0D8E4
const C_FILL_GR: (u32, u32, u32) = (228, 234, 242);   // 灰填充(悬停/箭头) #E4EAF2
const C_BG_TOP: (u32, u32, u32) = (247, 249, 252);    // 页面背景（纯净纯色）
const C_LABEL_BG: (u32, u32, u32) = (219, 234, 254);  // 天/时/分/秒 标签底色
const C_LABEL_TX: (u32, u32, u32) = (30, 58, 138);    // 标签文字
const C_TRACK: (u32, u32, u32) = (148, 168, 210);      // 进度条轨道(恢复原浅灰)
const C_WHITE: (u32, u32, u32) = (255, 255, 255);      // 纯白(文字/卡片底)
const C_BTN_GR: (u32, u32, u32) = (203, 213, 225);     // 中性按钮默认(箭头/置灰)
const C_BTN_GR_D: (u32, u32, u32) = (158, 172, 190);   // 中性按钮按下
const C_BTN_BLUE: (u32, u32, u32) = (3, 83, 182);      // 蓝色按钮默认(41复制同色)
const C_BTN_BLUE_H: (u32, u32, u32) = (27, 113, 219);  // 蓝色按钮悬停
const C_BTN_BLUE_D: (u32, u32, u32) = (41, 128, 185);  // 蓝色按钮按下

// 本程序 exe 的完整路径
fn exe_path() -> String {
    unsafe {
        let mut buf = [0u16; 1024];
        let n = GetModuleFileNameW(std::ptr::null_mut(), buf.as_mut_ptr(), 1024);
        String::from_utf16_lossy(&buf[..n as usize]).trim().to_string()
    }
}

// 开机自动启动：写入/删除 HKCU\...\Run 注册表项
fn set_autostart(on: bool) {
    unsafe {
        let key = wide(RUN_KEY);
        let mut h: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_SET_VALUE, &mut h) == 0 {
            if on {
                let val = wide(RUN_VALUE);
                let path = wide(&format!("\"{}\"", exe_path()));
                RegSetValueExW(h, val.as_ptr(), 0, REG_SZ,
                    path.as_ptr() as *const u8, (path.len() * 2) as u32);
            } else {
                let val = wide(RUN_VALUE);
                RegDeleteValueW(h, val.as_ptr());
            }
            RegCloseKey(h);
        }
    }
}

// 查询是否已设置开机自启
fn is_autostart() -> bool {
    unsafe {
        let key = wide(RUN_KEY);
        let mut h: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_QUERY_VALUE, &mut h) == 0 {
            let val = wide(RUN_VALUE);
            let mut ty: u32 = 0;
            let mut sz: u32 = 0;
            let r = RegQueryValueExW(h, val.as_ptr(), std::ptr::null_mut(), &mut ty,
                std::ptr::null_mut(), &mut sz);
            RegCloseKey(h);
            r == 0
        } else {
            false
        }
    }
}

// 从灰度图像解码二维码内容（gray 为 0-255 灰度，按行排列）
fn decode_qr(gray: &[u8], w: u32, h: u32) -> Option<String> {
    if gray.len() < 100 || w < 21 || h < 21 { return None; }
    let gimg = image::GrayImage::from_raw(w, h, gray.to_vec())?;
    let mut prepared = PreparedImage::prepare(gimg);
    for grid in prepared.detect_grids() {
        if let Ok((_corners, content)) = grid.decode() {
            return Some(content);
        }
    }
    None
}

// 用 qrcode 生成二维码 → BGRA 位图（编码参考草料：UTF-8；
// 无 Logo 时优先小版本，纠错按字符数在 15%(M)/25%(Q) 间自动选；
// 有 Logo 时用 30%(H) 高容错并把 Logo 居中叠加到白色圆角矩形底上）
// 按指定边长直接生成二维码（不做缩放），返回 (dim, dim, BGRA)
fn gen_qr_bgra_sized(text: &str, dim: u32) -> Option<(u32, u32, Vec<u8>)> {
    if dim == 0 { return None; }
    let has_logo = LOGO_IMG.lock().unwrap().is_some();
    let n = text.chars().count();
    let level = if has_logo { EcLevel::H } else if n < 100 { EcLevel::M } else { EcLevel::Q };
    let code = QrCode::with_error_correction_level(text.as_bytes(), level).ok()?;
    let size = code.width();
    let colors = code.to_colors(); // size*size, Color::Dark / Light
    let qz = 1u32; // 静区模块数（四周留 1 个色块白边）
    let full = size as u32 + qz * 2; // 含静区的模块总数
    let mut buf = vec![0xFFu8; (dim as usize * dim as usize * 4) as usize]; // 白底 BGRA
    // 直接把每个像素按模块分数比例落格，做到真正指定分辨率、无需二次缩放
    for y in 0..dim {
        let my = (y as u64 * full as u64) / dim as u64;
        if my < qz as u64 || my >= (qz as u64 + size as u64) { continue; }
        let row = my - qz as u64;
        for x in 0..dim {
            let mx = (x as u64 * full as u64) / dim as u64;
            if mx < qz as u64 || mx >= (qz as u64 + size as u64) { continue; }
            let col = mx - qz as u64;
            let dark = colors[row as usize * size + col as usize] == qrcode::Color::Dark;
            if dark {
                let idx = ((y as usize * dim as usize + x as usize) * 4) as usize;
                buf[idx] = 0; buf[idx + 1] = 0; buf[idx + 2] = 0; buf[idx + 3] = 255;
            }
        }
    }
    // 叠加 Logo：白色圆角矩形底 + 居中 logo
    overlay_logo(&mut buf, dim);
    Some((dim, dim, buf))
}

// 叠加 Logo 到二维码中央（白色直角矩形底，四周窄留白）
fn overlay_logo(buf: &mut [u8], dim: u32) {
    if let Some((lw, lh, lbgra)) = LOGO_IMG.lock().unwrap().clone() {
        if lw > 0 && lh > 0 {
            let target = (dim as f32 * 0.20) as u32; // logo 最大边长约 20%
            let sc_f = (target as f32) / (lw.max(lh) as f32);
            let tw = ((lw as f32) * sc_f).round() as i32;
            let th = ((lh as f32) * sc_f).round() as i32;
            let pad = ((target as f32) * 0.09).round() as i32; // 白色底留白（直角，四周窄留白）
            let bgw = tw + pad * 2;
            let bgh = th + pad * 2;
            let ox = ((dim as i32) - bgw) / 2;
            let oy = ((dim as i32) - bgh) / 2;
            for yy in 0..bgh {
                for xx in 0..bgw {
                    let px = ox + xx;
                    let py = oy + yy;
                    if px < 0 || py < 0 || px >= dim as i32 || py >= dim as i32 { continue; }
                    let di = (((py as u32) * dim + (px as u32)) * 4) as usize;
                    let lx = xx - pad;
                    let ly = yy - pad;
                    if lx >= 0 && ly >= 0 && lx < tw && ly < th {
                        let sx = ((lx as f32) * (lw as f32) / (tw as f32)) as usize;
                        let sy = ((ly as f32) * (lh as f32) / (th as f32)) as usize;
                        let pi = (sy * (lw as usize) + sx) * 4;
                        buf[di] = lbgra[pi];
                        buf[di + 1] = lbgra[pi + 1];
                        buf[di + 2] = lbgra[pi + 2];
                        buf[di + 3] = 255;
                    } else {
                        buf[di] = 255; buf[di + 1] = 255; buf[di + 2] = 255; buf[di + 3] = 255;
                    }
                }
            }
        }
    }
}

// 供界面显示使用的二维码：直接按二维码卡的显示像素生成，1:1 绘制不缩放，显示清晰不发虚
fn gen_qr_bgra(text: &str) -> Option<(u32, u32, Vec<u8>)> {
    let dim = (qr_side() - 12).max(24) as u32;
    gen_qr_bgra_sized(text, dim)
}

// 下载用：以整数模块边长生成，网格均匀、边缘锐利（尺寸取最接近 target 的 full 的整数倍）
fn gen_qr_bgra_int(text: &str, target: u32) -> Option<(u32, u32, Vec<u8>)> {
    let has_logo = LOGO_IMG.lock().unwrap().is_some();
    let n = text.chars().count();
    let level = if has_logo { EcLevel::H } else if n < 100 { EcLevel::M } else { EcLevel::Q };
    let code = QrCode::with_error_correction_level(text.as_bytes(), level).ok()?;
    let size = code.width();
    let colors = code.to_colors(); // size*size, Color::Dark / Light
    let qz = 1u32; // 静区模块数
    let full = size as u32 + qz * 2;
    let scale = ((target + full / 2) / full).max(1); // 每模块整数像素，四舍五入贴近 target
    let dim = full * scale;
    let mut buf = vec![0xFFu8; (dim as usize * dim as usize * 4) as usize]; // 白底 BGRA
    for y in 0..size as u32 {
        for x in 0..size as u32 {
            let dark = colors[(y * size as u32 + x) as usize] == qrcode::Color::Dark;
            if dark {
                let py = (y + qz) * scale;
                let px = (x + qz) * scale;
                for dy in 0..scale {
                    for dx in 0..scale {
                        let idx = (((py + dy) * dim + (px + dx)) * 4) as usize;
                        buf[idx] = 0; buf[idx + 1] = 0; buf[idx + 2] = 0; buf[idx + 3] = 255;
                    }
                }
            }
        }
    }
    overlay_logo(&mut buf, dim);
    Some((dim, dim, buf))
}

// 选择 Logo 图片文件
fn pick_logo_file(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        let mut fbuf = [0u16; 1024];
        let filter = wide("图片文件\0*.png;*.jpg;*.jpeg;*.bmp\0所有文件\0*.*\0\0");
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = hwnd;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.nFilterIndex = 1;
        ofn.lpstrFile = fbuf.as_mut_ptr();
        ofn.nMaxFile = fbuf.len() as u32;
        // OFN_FILEMUSTEXIST|OFN_PATHMUSTEXIST|OFN_EXPLORER
        ofn.Flags = 0x00001000 | 0x00000800 | 0x00080000;
        if GetOpenFileNameW(&mut ofn) != 0 {
            let nn = fbuf.iter().position(|&c| c == 0).unwrap_or(0);
            Some(String::from_utf16_lossy(&fbuf[..nn]).trim().to_string())
        } else {
            None
        }
    }
}

// 保存二维码对话框：返回要保存的 PNG 路径
fn pick_save_png_path(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        let mut fbuf = [0u16; 1024];
        // 默认文件名
        let def = wide("二维码.png");
        let def_len = def.len() - 1; // 去掉末尾 \0
        fbuf[..def_len].copy_from_slice(&def[..def_len]);
        let filter = wide("PNG 图片\0*.png\0所有文件\0*.*\0\0");
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = hwnd;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.nFilterIndex = 1;
        ofn.lpstrFile = fbuf.as_mut_ptr();
        ofn.nMaxFile = fbuf.len() as u32;
        // OFN_OVERWRITEPROMPT|OFN_PATHMUSTEXIST|OFN_EXPLORER
        ofn.Flags = 0x00000002 | 0x00000800 | 0x00080000;
        if GetSaveFileNameW(&mut ofn) != 0 {
            let nn = fbuf.iter().position(|&c| c == 0).unwrap_or(0);
            let mut p = String::from_utf16_lossy(&fbuf[..nn]).trim().to_string();
            // 若没写扩展名则补 .png
            if !p.to_lowercase().ends_with(".png") { p.push_str(".png"); }
            Some(p)
        } else {
            None
        }
    }
}

// 读取图片为 BGRA
fn load_logo_bgra(path: &str) -> Option<(u32, u32, Vec<u8>)> {    let img = image::open(path).ok()?;
    let (w, h) = img.dimensions();
    let rgba = img.to_rgba8();
    let mut bgra = Vec::with_capacity((w * h * 4) as usize);
    for p in rgba.pixels() {
        bgra.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    Some((w, h, bgra))
}

// 截取主屏幕为灰度图
// 读取图片文件为灰度图
// 把文本复制到剪贴板（CF_UNICODETEXT）
fn copy_text_clipboard(s: &str) -> bool {
    unsafe {
        let mut w: Vec<u16> = s.encode_utf16().collect();
        w.push(0);
        let n = w.len() * 2;
        let hmem = GlobalAlloc(GMEM_MOVEABLE, n);
        if hmem.is_null() { return false; }
        let ptr = GlobalLock(hmem) as *mut u16;
        if ptr.is_null() { GlobalFree(hmem); return false; }
        std::ptr::copy_nonoverlapping(w.as_ptr(), ptr, w.len());
        GlobalUnlock(hmem);
        if OpenClipboard(std::ptr::null_mut()) == 0 {
            GlobalFree(hmem);
            return false;
        }
        EmptyClipboard();
        let r = SetClipboardData(13 /*CF_UNICODETEXT*/, hmem);
        CloseClipboard();
        if r.is_null() { GlobalFree(hmem); return false; }
        true
    }
}

// 把二维码写入剪贴板：只写 CF_BITMAP 一种图片格式，保证粘贴只出图、绝不出文字。
fn copy_qr_full_clipboard(w: u32, h: u32, buf: &[u8]) -> bool {
    unsafe {
        let hbmp = CreateBitmap(w as i32, h as i32, 1, 32,
            buf.as_ptr() as *const winapi::ctypes::c_void);
        if hbmp.is_null() { return false; }
        let opened = OpenClipboard(std::ptr::null_mut()) != 0;
        if !opened {
            DeleteObject(hbmp as *mut winapi::ctypes::c_void);
            return false;
        }
        let emptied = EmptyClipboard() != 0;
        if !emptied {
            CloseClipboard();
            DeleteObject(hbmp as *mut winapi::ctypes::c_void);
            return false;
        }
        let r = SetClipboardData(2 /*CF_BITMAP*/, hbmp as *mut winapi::ctypes::c_void);
        CloseClipboard();
        // 成功时系统接管句柄；失败才需要释放
        if r.is_null() { DeleteObject(hbmp as *mut winapi::ctypes::c_void); }
        true
    }
}

// 把 BGRA 位图存为 PNG
fn save_bgra_png(w: u32, h: u32, buf: &[u8], path: &str) -> bool {
    // BGRA → RGBA
    let mut rgba = Vec::with_capacity((w as usize * h as usize * 4) as usize);
    for p in buf.chunks_exact(4) {
        rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    let im = image::RgbaImage::from_raw(w, h, rgba);
    match im {
        Some(img) => img.save(path).is_ok(),
        None => false,
    }
}

// 快捷键：写入本地注册表持久生效；未设置时默认 F1
// 读注册表 DWORD 值；取不到时返回默认 (mod,vk)
fn load_hotkey(value_name: &str, def_mod: u32, def_vk: u32) -> (u32, u32) {
    unsafe {
        let key = wide(SETTINGS_KEY);
        let mut h: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_QUERY_VALUE, &mut h) != 0 {
            return (def_mod, def_vk);
        }
        let name = wide(value_name);
        let mut v: u32 = 0;
        let mut sz: u32 = 4;
        let r = RegQueryValueExW(h, name.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(),
            &mut v as *mut u32 as *mut u8, &mut sz);
        RegCloseKey(h);
        if r != 0 || sz != 4 { return (def_mod, def_vk); }
        ((v >> 16) & 0xFFFF, v & 0xFFFF)
    }
}

// 写注册表 DWORD 值
fn save_hotkey(value_name: &str, reg_mod: u32, vk: u32) {
    unsafe {
        let key = wide(SETTINGS_KEY);
        let mut h: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, std::ptr::null_mut(),
            0, KEY_SET_VALUE, std::ptr::null_mut(), &mut h, std::ptr::null_mut()) == 0 {
            let name = wide(value_name);
            let v = ((reg_mod & 0xFFFF) << 16) | (vk & 0xFFFF);
            RegSetValueExW(h, name.as_ptr(), 0, REG_DWORD,
                &v as *const u32 as *const u8, 4);
            RegCloseKey(h);
        }
    }
}

// 读注册表普通 DWORD 值（取不到返回默认）
fn load_dword(value_name: &str, def: u32) -> u32 {
    unsafe {
        let key = wide(SETTINGS_KEY);
        let mut h: HKEY = std::ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, KEY_QUERY_VALUE, &mut h) != 0 {
            return def;
        }
        let name = wide(value_name);
        let mut v: u32 = 0;
        let mut sz: u32 = 4;
        let r = RegQueryValueExW(h, name.as_ptr(), std::ptr::null_mut(), std::ptr::null_mut(),
            &mut v as *mut u32 as *mut u8, &mut sz);
        RegCloseKey(h);
        if r != 0 || sz != 4 { return def; }
        v
    }
}

// 写注册表普通 DWORD 值
fn save_dword(value_name: &str, v: u32) {
    unsafe {
        let key = wide(SETTINGS_KEY);
        let mut h: HKEY = std::ptr::null_mut();
        if RegCreateKeyExW(HKEY_CURRENT_USER, key.as_ptr(), 0, std::ptr::null_mut(),
            0, KEY_SET_VALUE, std::ptr::null_mut(), &mut h, std::ptr::null_mut()) == 0 {
            let name = wide(value_name);
            RegSetValueExW(h, name.as_ptr(), 0, REG_DWORD,
                &v as *const u32 as *const u8, 4);
            RegCloseKey(h);
        }
    }
}

// 应用当前注册表里的全局快捷键（识别 / 显示隐藏）。
// 返回两行各自是否注册成功 (rec_ok, show_ok)，调用方可据此提示用户。
//
// 原版的问题：卸载/注册都不看返回值，于是
//   ① 用户录了一个已被别的软件（QQ、微信、输入法）占用的组合，RegisterHotKey 静默失败，
//      界面上显示着这个快捷键，按下去却毫无反应，用户完全无从判断；
//   ② 两行设成同一个组合时，先是 rec 注册成功、show 失败，界面上两个框都写着同一个键，
//      只有一个生效。
// 现在：先检测两行是否重复，重复则把第二行判为无效并明确报出来；注册失败的行同样回报。
fn apply_hotkeys() -> (bool, bool) {
    unsafe {
        let hwnd = MAIN_HWND.load(Ordering::Relaxed) as HWND;
        if hwnd.is_null() { return (false, false); }
        UnregisterHotKey(hwnd, WM_HOTKEY_REC_ID);
        UnregisterHotKey(hwnd, WM_HOTKEY_SHOW_ID);

        let (rm, rv) = load_hotkey(HOTKEY_REC_VALUE, 0, 0x70);  // 识别(跟随当前页) 默认 F1
        let (sm, sv) = load_hotkey(HOTKEY_SHOW_VALUE, 0, 0x79); // 显示/隐藏 默认 F10

        // 两行配成同一个组合：只有前者生效，后者必被判失败，别去碰 RegisterHotKey
        let dup = (rm, rv) == (sm, sv);
        // MOD_NOREPEAT：按住不放时不重复触发（Win7+ 支持），避免长按 F1 把识别刷屏
        let norep = 0x4000;
        let rec_ok = if rm == 0 && rv == 0 { false }
                     else { RegisterHotKey(hwnd, WM_HOTKEY_REC_ID, rm | norep, rv) != 0 };
        let show_ok = if dup || (sm == 0 && sv == 0) { false }
                      else { RegisterHotKey(hwnd, WM_HOTKEY_SHOW_ID, sm | norep, sv) != 0 };
        (rec_ok, show_ok)
    }
}

// 把注册结果显示到设置页提示行：全成功显示简洁提示，任一失败指出是哪一行、什么原因
fn hotkey_status_text(rec: (u32, u32), show: (u32, u32), ok: (bool, bool)) -> String {
    let mut bad: Vec<String> = Vec::new();
    let rname = hotkey_name(rec.0, rec.1);
    let sname = hotkey_name(show.0, show.1);
    if !ok.0 { bad.push(format!("「识别」{} 注册失败（可能已被其它软件占用）", rname)); }
    if !ok.1 {
        if rec == show {
            bad.push(format!("「显示/隐藏」{} 与「识别」重复，未生效", sname));
        } else {
            bad.push(format!("「显示/隐藏」{} 注册失败（可能已被其它软件占用）", sname));
        }
    }
    if bad.is_empty() {
        format!("快捷键已生效：{} / {}", rname, sname)
    } else {
        bad.join("；")
    }
}

// 免分配判断 exe 名是否命中目标压缩软件（sz 为 szExeFile 到首个空字符的切片，名称均为 ASCII）
fn is_target(sz: &[u16]) -> bool {
    let n = sz.len();
    if n == 0 { return false; }
    TARGET_PROCESSES.iter().any(|t| {
        let tb = t.as_bytes();
        if tb.len() != n { return false; }
        tb.iter().zip(sz).all(|(&b, &u)| {
            let lo = if (b'A' as u16..=b'Z' as u16).contains(&u) { u + 32 } else { u };
            let bl = if (b'A'..=b'Z').contains(&b) { b + 32 } else { b };
            lo as u8 == bl
        })
    })
}

// 窗口枚举回调：把属于目标压缩软件进程的所有可见窗口发送 WM_CLOSE（优雅关闭）
extern "system" fn close_archiver_enum(hwnd: HWND, lparam: LPARAM) -> i32 {
    unsafe {
        let pids = lparam as *mut Vec<u32>;
        if pids.is_null() { return 1; }
        if IsWindowVisible(hwnd) == 0 { return 1; }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 && (*pids).contains(&pid) {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
        }
        1
    }
}

// 窗口枚举回调：给指定 PID 列表里的进程发 WM_CLOSE（按 PID 关而不是按进程名关，
// 避免把"我正要保留的那个实例"也一起关掉）
extern "system" fn close_pid_enum(hwnd: HWND, lparam: LPARAM) -> i32 {
    unsafe {
        let pids = lparam as *mut Vec<u32>;
        if pids.is_null() { return 1; }
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if pid != 0 && (*pids).contains(&pid) {
            // 主窗口才发；隐藏的辅助窗口收到 WM_CLOSE 反而可能卡住退出流程
            if IsWindowVisible(hwnd) != 0 {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
        1
    }
}

// “一键关闭”：关闭所有开着的压缩软件窗口（复用 TARGET_PROCESSES 名单）
fn close_archiver_windows() {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap.is_null() { return; }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut pids: Vec<u32> = Vec::with_capacity(8);
        if Process32FirstW(snap, &mut entry) == 1 {
            loop {
                let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                if is_target(&entry.szExeFile[..end]) {
                    pids.push(entry.th32ProcessID);
                }
                if Process32NextW(snap, &mut entry) != 1 { break; }
            }
        }
        winapi::um::handleapi::CloseHandle(snap);
        if pids.is_empty() { return; }
        EnumWindows(Some(close_archiver_enum), &mut pids as *mut Vec<u32> as LPARAM);
    }
}

fn monitor_thread(running: Arc<AtomicBool>) {
    while running.load(Ordering::Relaxed) {
        let active;
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap.is_null() {
                std::thread::sleep(Duration::from_millis(800));
                continue;
            }

            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            let mut pids: Vec<u32> = Vec::with_capacity(8);

            if Process32FirstW(snap, &mut entry) == 1 {
                loop {
                    let end = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
                    if is_target(&entry.szExeFile[..end]) {
                        pids.push(entry.th32ProcessID);
                    }
                    if Process32NextW(snap, &mut entry) != 1 { break; }
                }
            }
            winapi::um::handleapi::CloseHandle(snap);
            active = !pids.is_empty();

            if pids.len() > 1 {
                // 按"压缩包"分组：同包多开只保留1个（最新），不同包各自保留
                let mut wins: Vec<(u32, String)> = Vec::new();
                EnumWindows(Some(enum_window_proc),
                    &mut wins as *mut Vec<(u32, String)> as LPARAM);

                // pid -> 压缩包标识（取该进程第一个可见窗口的标题）
                let mut pid_archive: std::collections::HashMap<u32, String> =
                    std::collections::HashMap::new();
                for (pid, title) in wins {
                    if pids.contains(&pid) {
                        pid_archive.entry(pid)
                            .or_insert_with(|| archive_identity(&title));
                    }
                }

                // 每个进程归入对应压缩包分组（无窗口的进程单独成组，不会被误杀）
                let mut groups: std::collections::HashMap<String, Vec<u32>> =
                    std::collections::HashMap::new();
                for &pid in &pids {
                    let ident = pid_archive.get(&pid).cloned()
                        .unwrap_or_else(|| format!("pid{}", pid));
                    groups.entry(ident).or_default().push(pid);
                }

                // 每个压缩包：同包进程>1 时保留创建时间最新的，关闭其余
                for group in groups.values() {
                    if group.len() < 2 { continue; }
                    let mut keep = group[0];
                    let mut keep_t = proc_creation_time(group[0]);
                    for i in 1..group.len() {
                        let t = proc_creation_time(group[i]);
                        if t > keep_t {
                            keep_t = t;
                            keep = group[i];
                        }
                    }
                    // 关掉多余的实例：先发 WM_CLOSE 让它自己走正常退出流程（落盘、清锁文件、
                    // 提示"是否保存"），等 1.5 秒还没退再强杀。
                    // 原版直接 TerminateProcess 一刀切，正在解压/正写盘的那个实例会被当场打断，
                    // 留下半截文件或坏压缩包 —— 用户只会觉得"这软件把我文件搞坏了"。
                    let mut victims: Vec<u32> = group.iter().copied()
                        .filter(|&pid| pid != keep).collect();
                    EnumWindows(Some(close_pid_enum),
                        &mut victims as *mut Vec<u32> as LPARAM);
                    // 宽限期：给被关的进程留出收尾时间
                    for _ in 0..15 {
                        if victims.is_empty() { break; }
                        std::thread::sleep(Duration::from_millis(100));
                        victims.retain(|&pid| {
                            let h = OpenProcess(0x1000 /*QUERY_LIMITED_INFORMATION*/, 0, pid);
                            if h.is_null() { return false; } // 已经退了
                            let mut code = 0u32;
                            let alive = GetExitCodeProcess(h, &mut code) != 0
                                && code == 259 /*STILL_ACTIVE*/;
                            winapi::um::handleapi::CloseHandle(h);
                            alive
                        });
                    }
                    // 还没退（比如弹了个"确实要退出吗"的对话框）才强杀
                    for &pid in &victims {
                        let h = OpenProcess(0x0001, 0, pid);
                        if !h.is_null() {
                            TerminateProcess(h, 0);
                            winapi::um::handleapi::CloseHandle(h);
                        }
                    }
                }
            }
        }
        // 自适应轮询：有压缩软件在跑 → 800ms 快扫（防多开灵敏）；空闲无目标 → 3000ms 省 CPU
        std::thread::sleep(Duration::from_millis(if active { 800 } else { 3000 }));
    }
}

// 从输入框内容生成二维码。quiet=true 时只更新码、不改状态栏（供实时输入用）
fn generate_from_edit(hwnd: HWND, quiet: bool) {
    unsafe {
        let edit = GetDlgItem(hwnd, QR_EDIT as i32);
        let mut buf = [0u16; 4096];
        let n = GetWindowTextW(edit, buf.as_mut_ptr(), 4096);
        let text = String::from_utf16_lossy(&buf[..n as usize]).trim().to_string();
        if text.is_empty() {
            if !quiet { set_status(hwnd, "请先输入二维码内容"); }
            return;
        }
        match gen_qr_bgra(&text) {
            Some((w, h, data)) => {
                *QR_IMG.lock().unwrap() = Some((w, h, data));
                *QR_TEXT.lock().unwrap() = text.clone();
                InvalidateRect(hwnd, std::ptr::null_mut(), 1);
            }
            None => if !quiet { set_status(hwnd, "生成失败：内容过长或无法编码") },
        }
    }
}
// 输入框下方内容起始 Y（顶部标签 + 输入框之后）。
// 输入框高度是固定的（见 INPUT_H 声明），不再有"随内容自动增高"那套逻辑
fn below_y() -> i32 {
    42 + INPUT_H + 10
}

// 二维码方框边长 = 左侧按钮列总高（5×46 + 4×12 间隙 = 278）
fn qr_side() -> i32 {
    sc(5 * 46 + 4 * 12)
}

// 右侧二维码显示区（正方形，右侧贴边对齐，随输入框增高而下移）
fn qr_area() -> RECT {
    let b = below_y();
    let side = qr_side();
    RECT { left: sc(585) - sc(14) - side, top: sc(b), right: sc(585) - sc(14), bottom: sc(b) + side }
}

// 统一布局：窗口固定 585×445，边距 14，纵向间隙 10，按钮 132×46、按钮间间隙 12
unsafe fn relayout(hwnd: HWND) {
    let p = PAGE_CUR.load(Ordering::Relaxed);
    // 顶部四个标签：固定每页宽度（高度加高）
    let t0 = GetDlgItem(hwnd, QR_TAB_OCR as i32);
    if !t0.is_null() { SetWindowPos(t0, std::ptr::null_mut(), sc(14), sc(2), sc(84), sc(30), SWP_NOZORDER); }
    let t1 = GetDlgItem(hwnd, QR_TAB_QR as i32);
    if !t1.is_null() { SetWindowPos(t1, std::ptr::null_mut(), sc(100), sc(2), sc(72), sc(30), SWP_NOZORDER); }
    let t2 = GetDlgItem(hwnd, QR_TAB_SHUT as i32);
    if !t2.is_null() { SetWindowPos(t2, std::ptr::null_mut(), sc(174), sc(2), sc(84), sc(30), SWP_NOZORDER); }
    let t3 = GetDlgItem(hwnd, QR_TAB_ANTI as i32);
    if !t3.is_null() { SetWindowPos(t3, std::ptr::null_mut(), sc(260), sc(2), sc(58), sc(30), SWP_NOZORDER); }
    let t4 = GetDlgItem(hwnd, QR_TAB_SET as i32);
    if !t4.is_null() { SetWindowPos(t4, std::ptr::null_mut(), sc(322), sc(2), sc(58), sc(30), SWP_NOZORDER); }

    if p == PAGE_QR {
        let b = below_y();
        let edit = GetDlgItem(hwnd, QR_EDIT as i32);
        if !edit.is_null() {
            SetWindowPos(edit, std::ptr::null_mut(), sc(14), sc(42), sc(557), sc(INPUT_H), SWP_NOZORDER);
        }
        // 左侧按钮列：宽132、高46、纵向间隙12（与文字识别页一致）
        let btns = [QR_BTN_CLEAR, QR_BTN_SCREEN, QR_BTN_COPY, QR_BTN_DOWNLOAD, QR_BTN_UPLOAD];
        for (i, id) in btns.iter().enumerate() {
            let bd = GetDlgItem(hwnd, *id as i32);
            if !bd.is_null() {
                SetWindowPos(bd, std::ptr::null_mut(), sc(14), sc(b + 58 * i as i32), sc(132), sc(46), SWP_NOZORDER);
            }
        }
    } else if p == PAGE_OCR {
        let sbox = GetDlgItem(hwnd, OCR_SEARCH_EDIT as i32);
        if !sbox.is_null() {
            SetWindowPos(sbox, std::ptr::null_mut(), sc(14), sc(42), sc(443), sc(28), SWP_NOZORDER);
        }
        let sb = GetDlgItem(hwnd, OCR_BTN_SEARCH as i32);
        if !sb.is_null() {
            SetWindowPos(sb, std::ptr::null_mut(), sc(471), sc(42), sc(100), sc(28), SWP_NOZORDER);
        }
        let ocr = GetDlgItem(hwnd, OCR_EDIT as i32);
        if !ocr.is_null() {
            // 识别框填满搜索框与按钮之间的空白（上下间隙统一 10）
            SetWindowPos(ocr, std::ptr::null_mut(), sc(14), sc(80), sc(557), sc(295), SWP_NOZORDER);
        }
        // 三个按钮彼此独立（宽132，间隙12）
        for (id, x) in [(OCR_BTN_REC, 14), (OCR_BTN_COPY, 158), (OCR_BTN_CLEAR, 302)] {
            let bd = GetDlgItem(hwnd, id as i32);
            if !bd.is_null() {
                SetWindowPos(bd, std::ptr::null_mut(), sc(x), sc(385), sc(132), sc(46), SWP_NOZORDER);
            }
        }
        // “识别 N 字符”显示：紧贴清空右侧
        let cnt = GetDlgItem(hwnd, OCR_LBL_COUNT as i32);
        if !cnt.is_null() {
            SetWindowPos(cnt, std::ptr::null_mut(), sc(446), sc(385), sc(125), sc(46), SWP_NOZORDER);
        }
    } else if p == PAGE_SHUT {
        sd_center_layout(hwnd);
    } else if p == PAGE_ANTI {
        // 防多开页：标题在 WM_PAINT 绘制（上移），按钮在下（窄版居中）
        let b = GetDlgItem(hwnd, ANTI_BTN_CLOSE as i32);
        if !b.is_null() { SetWindowPos(b, std::ptr::null_mut(), sc(192), sc(180), sc(200), sc(46), SWP_NOZORDER); }
    } else if p == PAGE_SET {
        sd_center_settings(hwnd);
    }
    // 所有页面统一窗口尺寸：客户区随 DPI 缩放（585×445 @100%），与控件同比例，避免高分屏控件溢出
    let style = GetWindowLongPtrW(hwnd, GWL_STYLE) as u32;
    let mut rc = RECT { left: 0, top: 0, right: sc(CW), bottom: sc(CH) };
    AdjustWindowRect(&mut rc, style, 0);
    SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0,
        rc.right - rc.left, rc.bottom - rc.top, SWP_NOMOVE | SWP_NOZORDER);
    InvalidateRect(hwnd, std::ptr::null_mut(), 1);
}

// ===================== 定时关机页 =====================
fn sd_ctrl(hwnd: HWND, id: usize) -> HWND { unsafe { GetDlgItem(hwnd, id as i32) } }

fn sd_text(hwnd: HWND, id: usize) -> String {
    unsafe {
        let c = sd_ctrl(hwnd, id);
        if c.is_null() { return String::new(); }
        let mut b = [0u16; 512];
        let n = GetWindowTextW(c, b.as_mut_ptr(), 512);
        if n == 0 { return String::new(); }
        String::from_utf16_lossy(&b[..n as usize])
    }
}

// 定时关机页控件定位（供左右居中；坐标统一按 DPI 缩放，与全窗口一致）
unsafe fn sd_pos(hwnd: HWND, id: usize, x: i32, y: i32, w: i32, h: i32) {
    let c = GetDlgItem(hwnd, id as i32);
    if !c.is_null() { SetWindowPos(c, std::ptr::null_mut(), sc(x), sc(y), sc(w), sc(h), SWP_NOZORDER); }
}

// 定时关机页：PShutdown 极简样式——顶部“关闭电脑倒计时”标题（WM_PAINT 绘制），
// 四组 天数/小时数/分钟数/秒数 数值输入+▲▼，底部 开始定时器(蓝)/暂停定时器(灰)
unsafe fn sd_center_layout(hwnd: HWND) {
    // 四组输入：与下方按钮行同宽铺满（14..571），每组 127 宽，输入框拉长到 87
    let gx = [14, 157, 300, 443];
    for (i, x) in gx.iter().enumerate() {
        let (ed, lbl, up, dn) = match i {
            0 => (SD_ED_DAY, SD_LBL_DAY, SD_UP_DAY, SD_DN_DAY),
            1 => (SD_ED_H, SD_LBL_HOUR, SD_UP_H, SD_DN_H),
            2 => (SD_ED_M, SD_LBL_MIN, SD_UP_M, SD_DN_M),
            _ => (SD_ED_S, SD_LBL_SEC, SD_UP_S, SD_DN_S),
        };
        sd_pos(hwnd, lbl, *x, 128, 127, 26);
        sd_pos(hwnd, ed, *x, 162, 87, 44);
        sd_pos(hwnd, up, *x + 87, 162, 40, 21);
        sd_pos(hwnd, dn, *x + 87, 185, 40, 21);
    }
    // 底部按钮：开始/暂停同宽，清空(红，窄)靠右，等距 12px
    sd_pos(hwnd, SD_START, 14, 252, 220, 50);
    sd_pos(hwnd, SD_PAUSE, 246, 252, 220, 50);
    sd_pos(hwnd, SD_CLEAR, 478, 252, 93, 50);
    // 状态行兼作进度条由 WM_PAINT 绘制（y324 区域），无需子控件
}

// 设置页：两行快捷键（显示框+录制）居中，保存/关闭方式/提示统一间距
unsafe fn sd_center_settings(hwnd: HWND) {
    let lo = (585 - 348) / 2; // 行宽 = 110 标签 + 12 + 150 框 + 12 + 64 录制 = 348
    for target in 0..2 {
        let (combo_id, rec_id, _, _, _, _, _, _) = hotkey_row(target);
        let lbl_id = match target { 0 => SET_LBL_REC, _ => SET_LBL_SHOW };
        let y = 42 + 66 * target as i32;
        let l = GetDlgItem(hwnd, lbl_id as i32);
        if !l.is_null() { SetWindowPos(l, std::ptr::null_mut(), sc(lo), sc(y + 4), sc(110), sc(20), SWP_NOZORDER); }
        let c = GetDlgItem(hwnd, combo_id as i32);
        if !c.is_null() { SetWindowPos(c, std::ptr::null_mut(), sc(lo + 122), sc(y), sc(150), sc(28), SWP_NOZORDER); }
        let rec = GetDlgItem(hwnd, rec_id as i32);
        if !rec.is_null() { SetWindowPos(rec, std::ptr::null_mut(), sc(lo + 284), sc(y - 2), sc(64), sc(28), SWP_NOZORDER); }
    }
    // 关闭方式：标签 + 退出/到托盘（默认退出）
    let lc = GetDlgItem(hwnd, SET_LBL_CLOSE as i32);
    if !lc.is_null() { SetWindowPos(lc, std::ptr::null_mut(), sc(lo), sc(226), sc(110), sc(24), SWP_NOZORDER); }
    let re = GetDlgItem(hwnd, SET_RAD_EXIT as i32);
    if !re.is_null() { SetWindowPos(re, std::ptr::null_mut(), sc(lo + 122), sc(222), sc(70), sc(24), SWP_NOZORDER); }
    let rt = GetDlgItem(hwnd, SET_RAD_TRAY as i32);
    if !rt.is_null() { SetWindowPos(rt, std::ptr::null_mut(), sc(lo + 204), sc(222), sc(80), sc(24), SWP_NOZORDER); }
    // 识别时隐藏主界面：标签 + 隐藏/不隐藏（默认隐藏）
    let lh = GetDlgItem(hwnd, SET_LBL_REC_HIDE as i32);
    if !lh.is_null() { SetWindowPos(lh, std::ptr::null_mut(), sc(lo), sc(264), sc(110), sc(24), SWP_NOZORDER); }
    let hy = GetDlgItem(hwnd, SET_RAD_HIDE_YES as i32);
    if !hy.is_null() { SetWindowPos(hy, std::ptr::null_mut(), sc(lo + 122), sc(260), sc(60), sc(24), SWP_NOZORDER); }
    let hn = GetDlgItem(hwnd, SET_RAD_HIDE_NO as i32);
    if !hn.is_null() { SetWindowPos(hn, std::ptr::null_mut(), sc(lo + 194), sc(260), sc(80), sc(24), SWP_NOZORDER); }
    // 识别引擎常驻（勾选框，独占一行）
    let ce = GetDlgItem(hwnd, SET_CHK_ENGINE as i32);
    if !ce.is_null() { SetWindowPos(ce, std::ptr::null_mut(), sc(lo), sc(292), sc(348), sc(24), SWP_NOZORDER); }
    // 提示区（让到勾选框下面）
    let h = GetDlgItem(hwnd, SET_HINT as i32);
    if !h.is_null() { SetWindowPos(h, std::ptr::null_mut(), sc(lo), sc(322), sc(348), sc(100), SWP_NOZORDER); }
}

// 去掉 u16 串两端空格（返回子切片）
fn trim16(v: &[u16]) -> &[u16] {
    let mut s = 0;
    while s < v.len() && v[s] == b' ' as u16 { s += 1; }
    let mut e = v.len();
    while e > s && v[e-1] == b' ' as u16 { e -= 1; }
    &v[s..e]
}

// 文字识别页搜索：在识别结果框中定位第一个匹配并滚动到可见处（按 UTF-16 索引精确选中）
unsafe fn ocr_search_jump(hwnd: HWND) {
    let sbox = GetDlgItem(hwnd, OCR_SEARCH_EDIT as i32);
    let oedit = GetDlgItem(hwnd, OCR_EDIT as i32);
    if sbox.is_null() || oedit.is_null() { return; }
    let mut sb = [0u16; 256];
    let sn = GetWindowTextW(sbox, sb.as_mut_ptr(), 256);
    let needle = trim16(&sb[..sn as usize]);
    if needle.is_empty() { return; }
    let mut tb = [0u16; 65536];
    let tn = GetWindowTextW(oedit, tb.as_mut_ptr(), 65536);
    let hay = &tb[..tn as usize];
    if hay.len() >= needle.len() {
        if let Some(pos) = (0..=hay.len()-needle.len()).find(|&i| &hay[i..i+needle.len()] == needle) {
            SendMessageW(oedit, 0x00B1 /*EM_SETSEL*/, pos as WPARAM, (pos + needle.len()) as LPARAM);
            SendMessageW(oedit, 0x00B7 /*EM_SCROLLCARET*/, 0, 0);
            return;
        }
    }
    // 未匹配：取消选中，避免误导
    SendMessageW(oedit, 0x00B1 /*EM_SETSEL*/, 0, 0);
}

// 更新文字识别页的“识别 N 字符”显示：仅文字识别页显示；没识别到或其它页则隐藏
unsafe fn update_ocr_count(hwnd: HWND) {
    let cnt = GetDlgItem(hwnd, OCR_LBL_COUNT as i32);
    if cnt.is_null() { return; }
    let n = OCR_TEXT.lock().unwrap().chars().count();
    if n > 0 && PAGE_CUR.load(Ordering::Relaxed) == PAGE_OCR {
        let s = format!("{} 字符", n);
        SetWindowTextW(cnt, wide(s.as_str()).as_ptr());
        ShowWindow(cnt, SW_SHOW);
    } else {
        ShowWindow(cnt, SW_HIDE);
    }
}

// 解析倒计时时长：天/时/分/秒
fn sd_num(hwnd: HWND, id: usize) -> Result<i64, String> {
    let v = sd_text(hwnd, id).trim().to_string();
    if v.is_empty() { return Ok(0); }
    v.parse::<i64>().map_err(|_| "只能填数字".to_string())
}

// 解析倒计时时长：天/时/分/秒
fn sd_time_secs(hwnd: HWND) -> Result<i64, String> {
    let d = sd_num(hwnd, SD_ED_DAY)?;
    let h = sd_num(hwnd, SD_ED_H)?;
    let m = sd_num(hwnd, SD_ED_M)?;
    let s = sd_num(hwnd, SD_ED_S)?;
    if d < 0 || h < 0 || m < 0 || s < 0 { return Err("时长不能为负".to_string()); }
    if m > 59 || s > 59 { return Err("分、秒需小于 60".to_string()); }
    let total = d * 86400 + h * 3600 + m * 60 + s;
    if total < 1 { return Err("时长需大于 0".to_string()); }
    Ok(total)
}

fn sd_remain_text(secs: i64) -> String {
    let s = secs.max(0);
    let d = s / 86400; let h = (s % 86400) / 3600; let m = (s % 3600) / 60; let s2 = s % 60;
    if d > 0 { format!("剩余 {}天 {:02}:{:02}:{:02}", d, h, m, s2) }
    else { format!("剩余 {:02}:{:02}:{:02}", h, m, s2) }
}

// ===== 轻提示弹窗（请设置计时器等：无按钮、约2秒自动消失，可多条最多5） =====
extern "system" fn hint_proc(hwnd: HWND, msg: UINT, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut cr: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut cr);
            // 半透明黑圆角底（整体透明度由 SetLayeredWindowAttributes 控制）+ 白字
            let hrgn = CreateRoundRectRgn(0, 0, cr.right, cr.bottom, 12, 12);
            SelectClipRgn(hdc, hrgn);
            let bg = CreateSolidBrush(rgb(0, 0, 0));
            FillRect(hdc, &cr, bg as HBRUSH);
            DeleteObject(bg as *mut winapi::ctypes::c_void);
            DeleteObject(hrgn as *mut winapi::ctypes::c_void);
            SelectClipRgn(hdc, std::ptr::null_mut());
            SetBkMode(hdc, TRANSPARENT as i32);
            let f = CreateFontW(sc(26), 0, 0, 0, 600, 0, 0, 0,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS, wide("Microsoft YaHei").as_ptr());
            let old = SelectObject(hdc, f as *mut winapi::ctypes::c_void);
            SetTextColor(hdc, rgb(C_WHITE.0, C_WHITE.1, C_WHITE.2));
            let mut rc = RECT { left: 12, top: 6, right: cr.right - 12, bottom: cr.bottom - 6 };
            // 文字：优先取 GWLP_USERDATA 里的动态消息，否则默认“请设置计时器”
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            let fallback = String::from("请设置计时器");
            let text: &str = if p != 0 {
                (*(p as *const String)).as_str()
            } else {
                fallback.as_str()
            };
            DrawTextW(hdc, wide(text).as_ptr(), -1, &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(hdc, old);
            DeleteObject(f as *mut winapi::ctypes::c_void);
            EndPaint(hwnd, &ps);
            0
        },
        WM_ERASEBKGND => 1,
        WM_TIMER => unsafe { DestroyWindow(hwnd); 0 },
        WM_LBUTTONDOWN | 0x0204 /*WM_RBUTTONDOWN*/ => unsafe { DestroyWindow(hwnd); 0 },
        WM_KEYDOWN => unsafe {
            if wp == VK_ESCAPE as WPARAM || wp == VK_RETURN as WPARAM { DestroyWindow(hwnd); }
            0
        },
        0x0082 /*WM_NCDESTROY*/ => unsafe {
            HINT_COUNT.store(HINT_COUNT.load(Ordering::Relaxed).saturating_sub(1), Ordering::Relaxed);
            let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
            if p != 0 { drop(Box::from_raw(p as *mut String)); }
            0
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

// 弹出轻提示：半透明黑底白字，位于软件窗口中间；无按钮、自动消失；连续点击可多条，最多同时 5 条
unsafe fn show_toast(main: HWND, msg: &str) {
    if HINT_COUNT.load(Ordering::Relaxed) >= 5 { return; }
    let hinstance = GetModuleHandleW(std::ptr::null_mut());
    let cls = wide("XinOCR_HINT");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        style: 0,
        lpfnWndProc: Some(hint_proc),
        cbClsExtra: 0, cbWndExtra: 0,
        hInstance: hinstance,
        hIcon: std::ptr::null_mut(),
        hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
        hbrBackground: std::ptr::null_mut(),
        lpszMenuName: std::ptr::null(),
        lpszClassName: cls.as_ptr(),
        hIconSm: std::ptr::null_mut(),
    };
    RegisterClassExW(&wc);
    let w = 300; let h = 72;
    // 在主窗口中间（而非屏幕中间）
    let mut mr: RECT = std::mem::zeroed();
    GetWindowRect(main, &mut mr);
    let cx = mr.left + (mr.right - mr.left) / 2 - w / 2;
    let cy = mr.top + (mr.bottom - mr.top) / 2 - h / 2;
    // 多条堆叠：新的一条略微左上错开，形成级联
    let n = HINT_COUNT.load(Ordering::Relaxed) as i32;
    let off = n * 8;
    let hwnd = CreateWindowExW(
        0x00000008 /*WS_EX_TOPMOST*/ | 0x00080000 /*WS_EX_LAYERED*/, cls.as_ptr(), wide("提示").as_ptr(),
        WS_POPUP, cx - off, cy - off, w, h,
        std::ptr::null_mut(), std::ptr::null_mut(), hinstance, std::ptr::null_mut());
    if hwnd.is_null() { return; }
    // 存入动态消息，由 hint_proc 绘制时读取
    let boxed = Box::new(String::from(msg));
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(boxed) as isize);
    HINT_COUNT.store(HINT_COUNT.load(Ordering::Relaxed) + 1, Ordering::Relaxed);
    // 半透明（整体透明度 210/255）
    SetLayeredWindowAttributes(hwnd, 0, 210, 0x2 /*LWA_ALPHA*/);
    SetTimer(hwnd, 1, 1200, None);
    ShowWindow(hwnd, SW_SHOW);
}

// 定时关机页“请设置计时器”提示（复用通用轻提示）
unsafe fn sd_hint(main: HWND) {
    show_toast(main, "请设置计时器");
}

// 执行关机（倒计时走完直接关机，不再弹确认）；返回是否已执行
fn sd_execute(_hwnd: HWND) -> bool {
    unsafe {
        // 倒计时走完直接关机，不再弹“确定”确认框（用户要求）
        // 启用 SE_SHUTDOWN_NAME 关机权限，确保 ExitWindowsEx 真正生效
        let cur = winapi::um::processthreadsapi::GetCurrentProcess();
        let mut htok: winapi::shared::ntdef::HANDLE = std::ptr::null_mut();
        let ok = winapi::um::processthreadsapi::OpenProcessToken(
            cur,
            winapi::um::winnt::TOKEN_ADJUST_PRIVILEGES | winapi::um::winnt::TOKEN_QUERY,
            &mut htok);
        if ok != 0 && !htok.is_null() {
            let mut luid: winapi::um::winnt::LUID = std::mem::zeroed();
            if winapi::um::winbase::LookupPrivilegeValueW(
                std::ptr::null_mut(), wide("SeShutdownPrivilege").as_ptr(), &mut luid) != 0 {
                let mut tp: winapi::um::winnt::TOKEN_PRIVILEGES = std::mem::zeroed();
                tp.PrivilegeCount = 1;
                tp.Privileges[0].Luid = luid;
                tp.Privileges[0].Attributes = winapi::um::winnt::SE_PRIVILEGE_ENABLED;
                winapi::um::securitybaseapi::AdjustTokenPrivileges(
                    htok, 0, &mut tp, 0, std::ptr::null_mut(), std::ptr::null_mut());
            }
            winapi::um::handleapi::CloseHandle(htok);
        }
        // 去掉 EWX_FORCE：原版强制关机不通知任何应用，未保存的文档、正在写的文件全丢。
        // 现在走正常关机流程，系统会提示用户保存；EWX_POWEROFF 让机器关电而不只是注销。
        ExitWindowsEx(EWX_POWEROFF, SHTDN_REASON_MAJOR_OTHER);
        true
    }
}

// 把剩余秒数写回四个输入框（倒计时实时显示）
unsafe fn sd_write_fields(hwnd: HWND) {
    let s = SD_REMAIN.load(Ordering::Relaxed).max(0);
    let d = s / 86400; let h = (s % 86400) / 3600; let m = (s % 3600) / 60; let s2 = s % 60;
    for (id, v) in [(SD_ED_DAY, d), (SD_ED_H, h), (SD_ED_M, m), (SD_ED_S, s2)] {
        let c = sd_ctrl(hwnd, id);
        if !c.is_null() {
            let t = if id == SD_ED_DAY { v.to_string() } else { format!("{:02}", v) };
            SetWindowTextW(c, wide(&t).as_ptr());
        }
    }
}

fn sd_update_status(hwnd: HWND) {
    unsafe {
        // 倒计时输入实时回显剩余时间
        if SD_RUNNING.load(Ordering::Relaxed) {
            sd_write_fields(hwnd);
        }
        // 状态文字由 WM_PAINT 绘制，这里仅重绘状态区（坐标与 WM_PAINT 一致），并刷新暂停按钮颜色
        InvalidateRect(hwnd, &RECT { left: sc(14), top: sc(340), right: sc(571), bottom: sc(380) }, 0);
        let p = GetDlgItem(hwnd, SD_PAUSE as i32);
        if !p.is_null() { InvalidateRect(p, std::ptr::null_mut(), 1); }
        // 清空：仅“未开始/已暂停”可用；正在倒计时（未暂停）置灰不可点
        let clear_enabled = !(SD_RUNNING.load(Ordering::Relaxed) && !SD_PAUSED.load(Ordering::Relaxed));
        let c = GetDlgItem(hwnd, SD_CLEAR as i32);
        if !c.is_null() {
            EnableWindow(c, clear_enabled as i32);
            InvalidateRect(c, std::ptr::null_mut(), 1);
        }
    }
}

// 上下箭头：对某个输入框数值 +1/-1（天0-9999、时0-23、分秒0-59），循环绕回
unsafe fn sd_nudge(hwnd: HWND, id: usize, dir: i64) {
    let c = sd_ctrl(hwnd, id);
    if c.is_null() { return; }
    let cur = sd_num(hwnd, id).unwrap_or(0).max(0);
    let (lo, hi) = match id {
        SD_ED_DAY => (0, 9999),
        SD_ED_H => (0, 23),
        SD_ED_M | SD_ED_S => (0, 59),
        _ => (0, 99),
    };
    let mut nv = cur + dir;
    if nv < lo { nv = hi; }
    if nv > hi { nv = lo; }
    let t = if id == SD_ED_DAY { nv.to_string() } else { format!("{:02}", nv) };
    SetWindowTextW(c, wide(&t).as_ptr());
}

fn sd_start(hwnd: HWND) {
    match sd_time_secs(hwnd) {
        Err(e) => {
            if e == "时长需大于 0" {
                // 全 0：弹出轻提示“请设置计时器”（半透明黑底白字、位于软件窗口中间、自动消失，可多条最多5）
                unsafe { sd_hint(hwnd); }
            } else {
                unsafe { MessageBoxW(hwnd, wide(&e).as_ptr(), wide("定时任务").as_ptr(), MB_OK | MB_ICONINFORMATION); }
            }
        }
        Ok(secs) => {
            SD_RUNNING.store(true, Ordering::Relaxed);
            SD_PAUSED.store(false, Ordering::Relaxed);
            SD_REMAIN.store(secs, Ordering::Relaxed);
            SD_MAX.store(secs, Ordering::Relaxed);
            SD_ALERT_SHOWN.store(false, Ordering::Relaxed);
            SD_END.store(0, Ordering::Relaxed);
            unsafe { SetTimer(hwnd, SD_TIMER_ID as usize, 1000, None); }
            sd_update_status(hwnd);
        }
    }
}

// 显示/隐藏各页面控件
fn apply_page(hwnd: HWND) {
    unsafe {
        let p = PAGE_CUR.load(Ordering::Relaxed);
        // 文字识别页
        for id in [OCR_EDIT, OCR_SEARCH_EDIT, OCR_BTN_SEARCH, OCR_BTN_REC, OCR_BTN_COPY, OCR_BTN_CLEAR, OCR_LBL_COUNT] {
            let c = GetDlgItem(hwnd, id as i32);
            if !c.is_null() { ShowWindow(c, if p == PAGE_OCR { SW_SHOW } else { SW_HIDE }); }
        }
        // 二维码页
        for id in [QR_EDIT, QR_BTN_CLEAR, QR_BTN_SCREEN, QR_BTN_COPY, QR_BTN_DOWNLOAD, QR_BTN_UPLOAD] {
            let c = GetDlgItem(hwnd, id as i32);
            if !c.is_null() { ShowWindow(c, if p == PAGE_QR { SW_SHOW } else { SW_HIDE }); }
        }
        // 定时关机页
        for id in [SD_ED_DAY, SD_LBL_DAY, SD_UP_DAY, SD_DN_DAY,
                   SD_ED_H, SD_LBL_HOUR, SD_UP_H, SD_DN_H,
                   SD_ED_M, SD_LBL_MIN, SD_UP_M, SD_DN_M,
                   SD_ED_S, SD_LBL_SEC, SD_UP_S, SD_DN_S,
                   SD_START, SD_PAUSE, SD_CLEAR] {
            let c = GetDlgItem(hwnd, id as i32);
            if !c.is_null() { ShowWindow(c, if p == PAGE_SHUT { SW_SHOW } else { SW_HIDE }); }
        }
        // 设置页
        for id in [SET_LBL_REC, SET_LBL_SHOW, SET_COMBO_REC, SET_COMBO_SHOW,
                   SET_RECORD_REC, SET_RECORD_SHOW, SET_HINT, SET_LBL_CLOSE, SET_RAD_EXIT, SET_RAD_TRAY,
                   SET_LBL_REC_HIDE, SET_RAD_HIDE_YES, SET_RAD_HIDE_NO, SET_CHK_ENGINE] {
            let c = GetDlgItem(hwnd, id as i32);
            if !c.is_null() { ShowWindow(c, if p == PAGE_SET { SW_SHOW } else { SW_HIDE }); }
        }
        // 防多开页
        let ab = GetDlgItem(hwnd, ANTI_BTN_CLOSE as i32);
        if !ab.is_null() { ShowWindow(ab, if p == PAGE_ANTI { SW_SHOW } else { SW_HIDE }); }
        // 文字识别页：按当前结果刷新“识别 N 字符”显示（空则隐藏）
        if p == PAGE_OCR { update_ocr_count(hwnd); }
        InvalidateRect(hwnd, std::ptr::null_mut(), 1);
        // 标签按钮是子窗口，父窗口失效不会重绘它们——逐个失效，保证选中高亮/未选不亮正确刷新
        for id in [QR_TAB_OCR, QR_TAB_QR, QR_TAB_SHUT, QR_TAB_ANTI, QR_TAB_SET] {
            let t = GetDlgItem(hwnd, id as i32);
            if !t.is_null() { InvalidateRect(t, std::ptr::null_mut(), 1); }
        }
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: UINT, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => unsafe {
            let hinstance = winapi::um::libloaderapi::GetModuleHandleW(std::ptr::null_mut());

            // 顶部页面标签（文字识别 / 二维码 / 定时关机 / 设置）：固定每页宽度
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("文字识别").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                sc(14), sc(2), sc(84), sc(30), hwnd, QR_TAB_OCR as HMENU,
                hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("二维码").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                sc(100), sc(2), sc(72), sc(30), hwnd, QR_TAB_QR as HMENU,
                hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("定时关机").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                sc(174), sc(2), sc(84), sc(30), hwnd, QR_TAB_SHUT as HMENU,
                hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("防多开").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                sc(260), sc(2), sc(58), sc(30), hwnd, QR_TAB_ANTI as HMENU,
                hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("设置").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                sc(322), sc(2), sc(58), sc(30), hwnd, QR_TAB_SET as HMENU,
                hinstance, std::ptr::null_mut());

            // ===== 设置页控件（两行快捷键：显示框 + 录制；关闭方式；提示） =====
            // 统一字体（微软雅黑，负字高+ClearType 保证清晰）
            let mut setf = SET_FONT.load(Ordering::Relaxed) as *mut winapi::ctypes::c_void;
            if setf.is_null() {
                setf = CreateFontW(-sc(16), 0, 0, 0, 500, 0, 0, 0,
                    DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS, wide("Microsoft YaHei").as_ptr())
                    as *mut winapi::ctypes::c_void;
                SET_FONT.store(setf as usize, Ordering::Relaxed);
            }
            for target in 0..2 {
                let (combo_id, rec_id, _store, vname, dm, dv, label, _) = hotkey_row(target);
                let lbl_id = match target { 0 => SET_LBL_REC, _ => SET_LBL_SHOW };
                let y = 42 + 66 * target as i32;
                let lc = CreateWindowExW(0, wide("STATIC").as_ptr(), wide(label).as_ptr(),
                    WS_CHILD | WS_VISIBLE, sc(14), sc(y + 4), sc(110), sc(20), hwnd, lbl_id as HMENU,
                    hinstance, std::ptr::null_mut());
                if !lc.is_null() { SendMessageW(lc, WM_SETFONT, setf as WPARAM, 1); }
                // 当前快捷键显示框（只读，默认 F1/F10；录制后更新）
                let (m, v) = load_hotkey(vname, dm, dv);
                let disp = CreateWindowExW(0, wide("EDIT").as_ptr(), wide("").as_ptr(),
                    WS_CHILD | WS_VISIBLE | WS_BORDER | 0x0800 | ES_CENTER,
                    sc(136), sc(y), sc(150), sc(28), hwnd, combo_id as HMENU,
                    hinstance, std::ptr::null_mut());
                if !disp.is_null() {
                    SendMessageW(disp, 0x000C, 0, wide(&hotkey_name(m, v)).as_ptr() as LPARAM);
                    SendMessageW(disp, WM_SETFONT, setf as WPARAM, 1);
                }
                CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("录制").as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_PUSHBUTTON, sc(298), sc(y - 2), sc(64), sc(28), hwnd,
                    rec_id as HMENU, hinstance, std::ptr::null_mut());
            }
            // 关闭方式：退出（默认）/ 到托盘
            let lc = CreateWindowExW(0, wide("STATIC").as_ptr(), wide("关闭按钮：").as_ptr(),
                WS_CHILD | WS_VISIBLE, sc(14), sc(226), sc(110), sc(24), hwnd,
                SET_LBL_CLOSE as HMENU, hinstance, std::ptr::null_mut());
            if !lc.is_null() { SendMessageW(lc, WM_SETFONT, setf as WPARAM, 1); }
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("退出").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTORADIOBUTTON | WS_GROUP,
                sc(136), sc(222), sc(70), sc(24), hwnd, SET_RAD_EXIT as HMENU,
                hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("到托盘").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTORADIOBUTTON,
                sc(218), sc(222), sc(80), sc(24), hwnd, SET_RAD_TRAY as HMENU,
                hinstance, std::ptr::null_mut());
            let cm = load_dword(CLOSE_MODE_VALUE, 0);
            let re = GetDlgItem(hwnd, SET_RAD_EXIT as i32);
            if !re.is_null() { SendMessageW(re, BM_SETCHECK as u32, if cm == 0 { 1 } else { 0 }, 0); }
            let rt = GetDlgItem(hwnd, SET_RAD_TRAY as i32);
            if !rt.is_null() { SendMessageW(rt, BM_SETCHECK as u32, if cm == 1 { 1 } else { 0 }, 0); }
            // 识别时隐藏主界面：隐藏（默认）/ 不隐藏
            let lh = CreateWindowExW(0, wide("STATIC").as_ptr(), wide("识别时隐藏主界面：").as_ptr(),
                WS_CHILD | WS_VISIBLE, sc(14), sc(264), sc(110), sc(24), hwnd,
                SET_LBL_REC_HIDE as HMENU, hinstance, std::ptr::null_mut());
            if !lh.is_null() { SendMessageW(lh, WM_SETFONT, setf as WPARAM, 1); }
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("隐藏").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTORADIOBUTTON | WS_GROUP,
                sc(136), sc(260), sc(60), sc(24), hwnd, SET_RAD_HIDE_YES as HMENU,
                hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("不隐藏").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTORADIOBUTTON,
                sc(204), sc(260), sc(80), sc(24), hwnd, SET_RAD_HIDE_NO as HMENU,
                hinstance, std::ptr::null_mut());
            let hr = load_dword(HIDE_ON_REC_VALUE, 1);
            let hy = GetDlgItem(hwnd, SET_RAD_HIDE_YES as i32);
            if !hy.is_null() { SendMessageW(hy, BM_SETCHECK as u32, if hr == 1 { 1 } else { 0 }, 0); }
            let hn = GetDlgItem(hwnd, SET_RAD_HIDE_NO as i32);
            if !hn.is_null() { SendMessageW(hn, BM_SETCHECK as u32, if hr == 0 { 1 } else { 0 }, 0); }
            // 识别引擎常驻：默认不勾。不勾时每次识别拉起子进程、识别完就退（省内存）；
            // 勾上则引擎进程一直留着，省掉每次 2~4 秒的模型加载，代价是常占约 100MB
            let ck = CreateWindowExW(0, wide("BUTTON").as_ptr(),
                wide("识别引擎常驻（省加载时间，多占约 100MB 内存）").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_AUTOCHECKBOX,
                sc(14), sc(292), sc(348), sc(24), hwnd, SET_CHK_ENGINE as HMENU,
                hinstance, std::ptr::null_mut());
            if !ck.is_null() {
                SendMessageW(ck, WM_SETFONT, setf as WPARAM, 1);
                SendMessageW(ck, BM_SETCHECK as u32,
                    if load_dword(KEEP_ENGINE_VALUE, 0) == 1 { 1 } else { 0 }, 0);
            }
            // 提示行默认显示快捷键注册结果：有键没注册上就在这儿直说，
            // 不再像原版那样"界面写着快捷键、按下去没反应、也没人告诉为什么"
            let hk_bits = HOTKEY_OK.load(Ordering::Relaxed);
            let hk_tip = hotkey_status_text(
                load_hotkey(HOTKEY_REC_VALUE, 0, 0x70),
                load_hotkey(HOTKEY_SHOW_VALUE, 0, 0x79),
                (hk_bits & 1 != 0, hk_bits & 2 != 0));
            let hc = CreateWindowExW(0, wide("STATIC").as_ptr(),
                wide(&hk_tip).as_ptr(),
                WS_CHILD | WS_VISIBLE, sc(14), sc(258), sc(348), sc(30), hwnd, SET_HINT as HMENU,
                hinstance, std::ptr::null_mut());
            if !hc.is_null() { SendMessageW(hc, WM_SETFONT, setf as WPARAM, 1); }

            // ===== 定时关机页控件（默认隐藏） =====
            // 倒计时输入：天/小时/分/秒（常显；▲▼ 上下调节）
            for (id, txt, x) in [(SD_ED_DAY,"0",67),(SD_ED_H,"00",185),(SD_ED_M,"00",303),(SD_ED_S,"00",421)] {
                let c = CreateWindowExW(0, wide("EDIT").as_ptr(), wide(txt).as_ptr(),
                    WS_CHILD | WS_VISIBLE | WS_BORDER | ES_CENTER | 0x2000 /*ES_NUMBER*/,
                    sc(x), sc(126), sc(56), sc(44), hwnd, id as HMENU, hinstance, std::ptr::null_mut());
                if !c.is_null() {
                    // 输入框 56×44 配 24px 粗体（负字高 + ClearType，避免文字与框大小不匹配/发虚）
                    let ef = CreateFontW(-sc(24), 0, 0, 0, 700, 0, 0, 0,
                        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS, wide("Microsoft YaHei").as_ptr());
                    SendMessageW(c, WM_SETFONT, ef as WPARAM, 1);
                }
            }
            for (id, txt, x) in [(SD_LBL_DAY,"天",67),(SD_LBL_HOUR,"时",185),(SD_LBL_MIN,"分",303),(SD_LBL_SEC,"秒",421)] {
                let c = CreateWindowExW(0, wide("STATIC").as_ptr(), wide(txt).as_ptr(),
                    WS_CHILD | WS_VISIBLE | SS_CENTER | SS_CENTERIMAGE,
                    sc(x), sc(94), sc(96), sc(28), hwnd, id as HMENU,
                    hinstance, std::ptr::null_mut());
                if !c.is_null() {
                    // 标签字加大（负字高 + ClearType）
                    let lf = CreateFontW(-sc(20), 0, 0, 0, 600, 0, 0, 0,
                        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS, wide("Microsoft YaHei").as_ptr());
                    SendMessageW(c, WM_SETFONT, lf as WPARAM, 1);
                }
            }
            // 上下调节箭头（▲▼ 自绘）
            for (id, x, y) in [(SD_UP_DAY,67,126),(SD_DN_DAY,67,149),
                               (SD_UP_H,185,126),(SD_DN_H,185,149),
                               (SD_UP_M,303,126),(SD_DN_M,303,149),
                               (SD_UP_S,421,126),(SD_DN_S,421,149)] {
                CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("").as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_OWNERDRAW, sc(x), sc(y), sc(40), sc(21), hwnd,
                    id as HMENU, hinstance, std::ptr::null_mut());
            }
            // 按钮：开始定时器(蓝) / 暂停定时器(灰)
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("开始定时器").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW, sc(14), sc(260), sc(271), sc(48), hwnd,
                SD_START as HMENU, hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("暂停定时器").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW, sc(300), sc(260), sc(271), sc(48), hwnd,
                SD_PAUSE as HMENU, hinstance, std::ptr::null_mut());
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("清空").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW, sc(14), sc(260), sc(177), sc(48), hwnd,
                SD_CLEAR as HMENU, hinstance, std::ptr::null_mut());

            // ===== 文字识别页控件（默认隐藏） =====
            // 顶部搜索框 + 右侧“查找”按钮（蓝色自绘，与其他按钮一致；按钮宽100）
            let sbox = CreateWindowExW(0, wide("EDIT").as_ptr(), wide("").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | ES_AUTOHSCROLL,
                sc(14), sc(42), sc(443), sc(28), hwnd, OCR_SEARCH_EDIT as HMENU,
                hinstance, std::ptr::null_mut());
            if !sbox.is_null() {
                SendMessageW(sbox, 0x1501 /*EM_SETCUEBANNER*/, 0,
                    wide("搜索定位…").as_ptr() as LPARAM);
            }
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("查找").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                sc(471), sc(42), sc(100), sc(28), hwnd, OCR_BTN_SEARCH as HMENU,
                hinstance, std::ptr::null_mut());
            let ocr = CreateWindowExW(0, wide("EDIT").as_ptr(), wide("").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | 0x0100 /*ES_NOHIDESEL 选中常显高亮*/,
                sc(14), sc(80), sc(557), sc(295), hwnd, OCR_EDIT as HMENU,
                hinstance, std::ptr::null_mut());
            if !ocr.is_null() {
                SendMessageW(ocr, 0x1501 /*EM_SETCUEBANNER*/, 0,
                    wide("识别出的文字会显示在这里…").as_ptr() as LPARAM);
            }
            // 三个按钮彼此独立（宽132，间隙12）
            for (id, label, x) in [(OCR_BTN_REC, "识别", 14), (OCR_BTN_COPY, "复制", 158), (OCR_BTN_CLEAR, "清空", 302)] {
                CreateWindowExW(0, wide("BUTTON").as_ptr(), wide(label).as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                    sc(x), sc(385), sc(132), sc(46), hwnd, id as HMENU, hinstance, std::ptr::null_mut());
            }
            // “清空”右侧的“识别 N 字符”显示（空则隐藏；字体在 efont 块里统一应用；水平居中）
            CreateWindowExW(0, wide("STATIC").as_ptr(), wide("").as_ptr(),
                WS_CHILD | SS_CENTER | 0x0200 /*SS_CENTERIMAGE*/,
                sc(446), sc(385), sc(125), sc(46), hwnd, OCR_LBL_COUNT as HMENU,
                hinstance, std::ptr::null_mut());

            // 顶部大输入框（多行，自动换行；固定填满，占位提示“请输入文本内容！”）
            // 存“未缩放”基准高度 101，所有使用处各自套一次 sc()，避免高 DPI 下双重缩放
            let edit = CreateWindowExW(0, wide("EDIT").as_ptr(), wide("").as_ptr(),
                WS_CHILD | WS_VISIBLE | WS_BORDER | ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN,
                sc(14), sc(42), sc(557), sc(INPUT_H), hwnd, QR_EDIT as HMENU,
                hinstance, std::ptr::null_mut());
            // 占位提示：空时显示灰色“请输入文本内容！”，一输入就消失
            if !edit.is_null() {
                SendMessageW(edit, 0x1501 /*EM_SETCUEBANNER*/, 0,
                    wide("请输入文本内容！").as_ptr() as LPARAM);
            }
            // 输入框字体：按 DPI 放大 + ClearType，文字清晰（字调大2px）
            let efont = CreateFontW(sc(22), 0, 0, 0, 400 /*FW_NORMAL*/, 0, 0, 0,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS,
                wide("Microsoft YaHei").as_ptr());
            EDIT_FONT.store(efont as isize, Ordering::Relaxed);
            if !efont.is_null() {
                SendMessageW(edit, WM_SETFONT, efont as WPARAM, 1);
                // 文字识别页的大框、搜索框、查找按钮也用同一字体，保证文字一致清晰
                let ocr = GetDlgItem(hwnd, OCR_EDIT as i32);
                if !ocr.is_null() { SendMessageW(ocr, WM_SETFONT, efont as WPARAM, 1); }
                let sbox = GetDlgItem(hwnd, OCR_SEARCH_EDIT as i32);
                if !sbox.is_null() { SendMessageW(sbox, WM_SETFONT, efont as WPARAM, 1); }
                let sb = GetDlgItem(hwnd, OCR_BTN_SEARCH as i32);
                if !sb.is_null() { SendMessageW(sb, WM_SETFONT, efont as WPARAM, 1); }
                let cnt = GetDlgItem(hwnd, OCR_LBL_COUNT as i32);
                if !cnt.is_null() { SendMessageW(cnt, WM_SETFONT, efont as WPARAM, 1); }
            }
            // 原版这里把 QR_EDIT 子类化成 edit_wnd_proc，但那个过程只做 CallWindowProcW 透传，
            // 没有任何实际逻辑（回车换行由 ES_WANTRETURN 原生支持，实时生成由 EN_CHANGE 负责）。
            // 纯死代码，已删除；顺带省掉一处对全局唯一槽 EDIT_ORIG_WNDPROC 的写。

            // 左侧功能按钮（蓝色圆角自绘，清空为红色）；按钮列总高 = 右侧正方形高度
            let btns = [
                (QR_BTN_CLEAR, "清空", 98),
                (QR_BTN_SCREEN, "识别二维码", 156),
                (QR_BTN_COPY, "复制二维码", 214),
                (QR_BTN_DOWNLOAD, "下载二维码", 272),
                (QR_BTN_UPLOAD, "上传 LOGO", 330),
            ];
            for (id, label, y) in btns {
                CreateWindowExW(0, wide("BUTTON").as_ptr(), wide(label).as_ptr(),
                    WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                    sc(14), sc(y), sc(132), sc(46), hwnd, id as HMENU, hinstance, std::ptr::null_mut());
            }

            // ===== 防多开页控件：全部关闭按钮（标题在 WM_PAINT 直接绘制） =====
            CreateWindowExW(0, wide("BUTTON").as_ptr(), wide("全部关闭").as_ptr(),
                WS_CHILD | WS_VISIBLE | BS_OWNERDRAW,
                sc(192), sc(180), sc(200), sc(46), hwnd, ANTI_BTN_CLOSE as HMENU,
                hinstance, std::ptr::null_mut());

            // 按当前页面（默认文字识别页）隐藏其它页控件
            apply_page(hwnd);

            // 默认显示“请输入文本内容！”的二维码
            if let Some((w, h, data)) = gen_qr_bgra("请输入文本内容！") {
                *QR_IMG.lock().unwrap() = Some((w, h, data));
                *QR_TEXT.lock().unwrap() = "请输入文本内容！".to_string();
            }

            // 托盘图标
            let tip = wide("Xin OCR");
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            nid.uCallbackMessage = WM_TRAYICON;
            nid.hIcon = LoadIconW(hinstance, 1 as *const u16);
            let n = tip.len().min(128);
            nid.szTip[..n].copy_from_slice(&tip[..n]);
            Shell_NotifyIconW(NIM_ADD, &mut nid);
            // 按当前输入框高度统一摆放控件
            relayout(hwnd);
            0
        }

        WM_TRAYICON => unsafe {
            let ev = (lp & 0xFFFF) as u32;
            if ev == WM_LBUTTONUP || ev == WM_LBUTTONDBLCLK {
                // 单击切换：显示→隐藏，隐藏→显示（托盘行为保持原样）
                if IsWindowVisible(hwnd) != 0 {
                    ShowWindow(hwnd, SW_HIDE);
                } else {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                    SetFocus(hwnd); // 焦点还给窗口本身，让输入框占位提示显示
                }
                0
            } else if ev == WM_RBUTTONUP {
                let mut pt: POINT = std::mem::zeroed();
                GetCursorPos(&mut pt);
                let hmenu = CreatePopupMenu();
                AppendMenuW(hmenu, MF_STRING | (if is_autostart() { MF_CHECKED } else { 0 }),
                    ID_TRAY_AUTOSTART as usize, wide("开机自动启动").as_ptr());
                AppendMenuW(hmenu, MF_STRING, ID_TRAY_SETTINGS as usize, wide("设置").as_ptr());
                AppendMenuW(hmenu, MF_STRING, ID_TRAY_SHOW as usize, wide("显示主界面").as_ptr());
                AppendMenuW(hmenu, MF_STRING, ID_TRAY_EXIT as usize, wide("退出").as_ptr());
                SetForegroundWindow(hwnd);
                let cmd = TrackPopupMenu(hmenu,
                    TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | TPM_RETURNCMD,
                    pt.x, pt.y, 0, hwnd, std::ptr::null_mut());
                DestroyMenu(hmenu);
                if cmd == ID_TRAY_SHOW as i32 {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                    SetFocus(hwnd); // 焦点还给窗口本身，让输入框占位提示显示
                } else if cmd == ID_TRAY_EXIT as i32 {
                    PostQuitMessage(0);
                } else if cmd == ID_TRAY_AUTOSTART as i32 {
                    set_autostart(!is_autostart());
                } else if cmd == ID_TRAY_SETTINGS as i32 {
                    // 切换到“设置”标签页并显示主窗口
                    PAGE_CUR.store(PAGE_SET, Ordering::Relaxed);
                    apply_page(hwnd);
                    relayout(hwnd);
                    if IsWindowVisible(hwnd) == 0 {
                        ShowWindow(hwnd, SW_SHOW);
                        SetForegroundWindow(hwnd);
                    }
                }
                0
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }

        // 全局快捷键：识别二维码 / 识别文字 / 两者都识别 / 显示隐藏
        WM_HOTKEY => unsafe {
            let wid = wp as i32;
            if wid == WM_HOTKEY_REC_ID {
                // 二维码页=先识别二维码、无则识别文字；其它页=识别文字（识别前内部会隐藏主界面）
                if PAGE_CUR.load(Ordering::Relaxed) == PAGE_QR {
                    start_region_select(2);
                } else {
                    start_region_select(1);
                }
                0
            } else if wid == WM_HOTKEY_SHOW_ID {
                if IsWindowVisible(hwnd) != 0 {
                    ShowWindow(hwnd, SW_HIDE);
                } else {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                    SetFocus(hwnd); // 焦点还给窗口，让输入框占位提示显示
                }
                0
            } else {
                DefWindowProcW(hwnd, msg, wp, lp)
            }
        }

        // 点 X（关闭）：隐藏到托盘，不退出
        // 关闭按钮：默认退出软件；设置里可选“到托盘”
        WM_CLOSE => unsafe {
            if load_dword(CLOSE_MODE_VALUE, 0) == 1 {
                ShowWindow(hwnd, SW_HIDE);
            } else {
                PostQuitMessage(0);
            }
            0
        },
        // 原生标题栏按钮：最小化“-”最小化到托盘；最大化/还原保持原生
        WM_SYSCOMMAND => unsafe {
            if (wp as u32 & 0xFFF0) == 0xF020 /*SC_MINIMIZE*/ {
                ShowWindow(hwnd, SW_HIDE);
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        },

        // 识别完成：结果回填到当前页面对应的框（工作线程通知）
        WM_QR_RESULT => unsafe {
            let t = RECOG_RESULT.lock().unwrap().clone();
            if !t.is_empty() && !t.starts_with("未识别") {
                let pc = PAGE_CUR.load(Ordering::Relaxed);
                let mode = SEL_MODE.load(Ordering::Relaxed);
                let qr_hit = SEL_QR_HIT.load(Ordering::Relaxed);
                // 在二维码页识别时，无论文字还是二维码都直接进二维码输入框、不跳页；
                // 其它页：二维码识别结果进二维码框，文字识别进文字框。
                let into_qr = pc == PAGE_QR || mode == 0 || (mode == 2 && qr_hit);
                if into_qr {
                    // 二维码识别 → 回填二维码输入框并刷新（quiet：二维码页不做任何弹窗）
                    let edit = GetDlgItem(hwnd, QR_EDIT as i32);
                    if !edit.is_null() { SetWindowTextW(edit, wide(&t).as_ptr()); }
                    generate_from_edit(hwnd, true);
                    PAGE_CUR.store(PAGE_QR, Ordering::Relaxed);
                } else {
                    // 文字识别 → 回填文字识别框
                    *OCR_TEXT.lock().unwrap() = t.clone();
                    let ocr = GetDlgItem(hwnd, OCR_EDIT as i32);
                    if !ocr.is_null() { SetWindowTextW(ocr, wide(&t).as_ptr()); }
                    PAGE_CUR.store(PAGE_OCR, Ordering::Relaxed);
                }
                apply_page(hwnd);
                relayout(hwnd);
                update_ocr_count(hwnd);
                // 识别到内容：自动弹出主界面，让用户看到结果（不再藏在托盘）
                if IsWindowVisible(hwnd) == 0 {
                    ShowWindow(hwnd, SW_SHOW);
                    SetForegroundWindow(hwnd);
                }
            }
            0
        },

        // 禁止默认擦背景（配合双缓冲，避免闪烁）
        WM_ERASEBKGND => 1,

        // 天/时/分/秒 标签：浅蓝芯片底 + 深蓝字
        WM_CTLCOLORSTATIC => unsafe {
            let id = GetDlgCtrlID(lp as HWND) as usize;
            if id == SD_LBL_DAY || id == SD_LBL_HOUR || id == SD_LBL_MIN || id == SD_LBL_SEC {
                SetBkMode(wp as _, 1 /*TRANSPARENT*/);
                SetTextColor(wp as _, rgb(C_LABEL_TX.0, C_LABEL_TX.1, C_LABEL_TX.2));
                let mut h = SD_LABEL_BG.load(Ordering::Relaxed) as HBRUSH;
                if h.is_null() {
                    h = CreateSolidBrush(rgb(C_LABEL_BG.0, C_LABEL_BG.1, C_LABEL_BG.2));
                    SD_LABEL_BG.store(h as usize, Ordering::Relaxed);
                }
                (h as usize) as LRESULT
            } else {
                DefWindowProcW(hwnd, WM_CTLCOLORSTATIC, wp, lp)
            }
        },

        WM_PAINT => unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut cr: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut cr);
            let cw = cr.right;
            let ch = cr.bottom;

            // 非二维码页（文字识别/定时关机/设置）：纯净纯色背景（控件为子窗口自行绘制）
            let pc = PAGE_CUR.load(Ordering::Relaxed);
            if pc != PAGE_QR {
                let bg = CreateSolidBrush(rgb(C_BG_TOP.0, C_BG_TOP.1, C_BG_TOP.2));
                FillRect(hdc, &cr, bg as HBRUSH);
                DeleteObject(bg as *mut winapi::ctypes::c_void);
                // 定时关机页：居中大字标题 + 状态行兼作进度条（纯净底色上）
                if pc == PAGE_SHUT {
                    SetBkMode(hdc, TRANSPARENT as i32);
                    let tf = CreateFontW(-sc(34), 0, 0, 0, 700, 0, 0, 0,
                        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS,
                        wide("Microsoft YaHei").as_ptr());
                    let oldtf = SelectObject(hdc, tf as *mut winapi::ctypes::c_void);
                    SetTextColor(hdc, rgb(C_TEXT.0, C_TEXT.1, C_TEXT.2));
                    let mut tr = RECT { left: sc(14), top: sc(56), right: sc(571), bottom: sc(98) };
                    DrawTextW(hdc, wide("关闭电脑倒计时").as_ptr(), -1, &mut tr,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    SelectObject(hdc, oldtf);
                    DeleteObject(tf as *mut winapi::ctypes::c_void);
                    // 定时关机页状态行兼作进度条：单条（轨道+蓝色填充+白色状态文字）
                    let bar = RECT { left: sc(14), top: sc(340), right: sc(571), bottom: sc(380) };
                    let htrack = CreateSolidBrush(rgb(C_TRACK.0, C_TRACK.1, C_TRACK.2));
                    FillRect(hdc, &bar, htrack as HBRUSH);
                    DeleteObject(htrack as *mut winapi::ctypes::c_void);
                    if SD_RUNNING.load(Ordering::Relaxed) {
                        let max = SD_MAX.load(Ordering::Relaxed).max(1) as f64;
                        let rem = SD_REMAIN.load(Ordering::Relaxed).max(0) as f64;
                        let frac = (1.0 - rem / max).clamp(0.0, 1.0) as f32;
                        let fw = ((bar.right - bar.left) as f32 * frac) as i32;
                        if fw > 2 {
                            let fill = CreateSolidBrush(rgb(C_PRIMARY.0, C_PRIMARY.1, C_PRIMARY.2));
                            FillRect(hdc, &RECT { left: bar.left, top: bar.top, right: bar.left + fw, bottom: bar.bottom },
                                fill as HBRUSH);
                            DeleteObject(fill as *mut winapi::ctypes::c_void);
                        }
                    }
                    let sf = CreateFontW(-sc(20), 0, 0, 0, 700, 0, 0, 0,
                        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS,
                        wide("Microsoft YaHei").as_ptr());
                    let oldsf = SelectObject(hdc, sf as *mut winapi::ctypes::c_void);
                    SetTextColor(hdc, 0xFFFFFF);
                    let mut sr = RECT { left: sc(18), top: sc(341), right: sc(567), bottom: sc(379) };
                    let stxt = if SD_RUNNING.load(Ordering::Relaxed) {
                        if SD_PAUSED.load(Ordering::Relaxed) {
                            format!("已暂停  {}", sd_remain_text(SD_REMAIN.load(Ordering::Relaxed)))
                        } else {
                            format!("正在倒计时  {}", sd_remain_text(SD_REMAIN.load(Ordering::Relaxed)))
                        }
                    } else if SD_END.load(Ordering::Relaxed) == 1 {
                        "已完成".to_string()
                    } else if SD_END.load(Ordering::Relaxed) == 2 {
                        "已取消".to_string()
                    } else {
                        "未开始".to_string()
                    };
                    DrawTextW(hdc, wide(&stxt).as_ptr(), -1, &mut sr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    SelectObject(hdc, oldsf);
                    DeleteObject(sf as *mut winapi::ctypes::c_void);
                }
                // 防多开页：居中大字标题“压缩软件防多开”（WM_PAINT 绘制，无控件背景、不透底）
                if pc == PAGE_ANTI {
                    SetBkMode(hdc, TRANSPARENT as i32);
                    let atf = CreateFontW(-sc(38), 0, 0, 0, 700, 0, 0, 0,
                        DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                        CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS,
                        wide("Microsoft YaHei").as_ptr());
                    let oldatf = SelectObject(hdc, atf as *mut winapi::ctypes::c_void);
                    SetTextColor(hdc, rgb(C_TEXT.0, C_TEXT.1, C_TEXT.2));
                    let mut atr = RECT { left: sc(14), top: sc(100), right: sc(571), bottom: sc(150) };
                    DrawTextW(hdc, wide("压缩软件防多开").as_ptr(), -1, &mut atr,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                    SelectObject(hdc, oldatf);
                    DeleteObject(atf as *mut winapi::ctypes::c_void);
                }
                EndPaint(hwnd, &ps);
                return 0;
            }

            // 二维码页：双缓冲 + 纯色背景 + 卡片（控件为子窗口自行绘制）
            let memdc = CreateCompatibleDC(hdc);
            let hbmp = CreateCompatibleBitmap(hdc, cw, ch);
            let oldbmp = SelectObject(memdc, hbmp as *mut winapi::ctypes::c_void);

            // 纯净纯色背景（与其它页一致，彻底消除花纹）
            let bg = CreateSolidBrush(rgb(C_BG_TOP.0, C_BG_TOP.1, C_BG_TOP.2));
            FillRect(memdc, &RECT { left: 0, top: 0, right: cw, bottom: ch }, bg as HBRUSH);
            DeleteObject(bg as *mut winapi::ctypes::c_void);

            // 右侧二维码显示区：正方形白色卡片 + 浅灰描边
            let area = qr_area();
            let border_brush = CreateSolidBrush(rgb(C_BORDER.0, C_BORDER.1, C_BORDER.2));
            FillRect(memdc, &area, border_brush as HBRUSH);
            DeleteObject(border_brush as *mut winapi::ctypes::c_void);
            let white_brush = CreateSolidBrush(rgb(C_WHITE.0, C_WHITE.1, C_WHITE.2));
            let inner = RECT { left: area.left + 2, top: area.top + 2, right: area.right - 2, bottom: area.bottom - 2 };
            FillRect(memdc, &inner, white_brush as HBRUSH);
            DeleteObject(white_brush as *mut winapi::ctypes::c_void);

            // 二维码：整图缩放绘制（标准格式，一次到位，不会出现条纹）
            {
                let guard = QR_IMG.lock().unwrap();
                if let Some((w, h, buf)) = &*guard {
                    let avail = (inner.right - inner.left) as i64;
                    let availh = (inner.bottom - inner.top) as i64;
                    let s = (avail.min(availh) - 8).max(24) as u32;
                    let dx = (inner.left as i64 + (avail - s as i64) / 2) as i32;
                    let dy = (inner.top as i64 + (availh - s as i64) / 2) as i32;
                    // 用 BGRA 数据直接建 32bpp 位图
                    let qbmp = CreateBitmap(*w as i32, *h as i32, 1, 32,
                        buf.as_ptr() as *const winapi::ctypes::c_void);
                    if !qbmp.is_null() {
                        let qdc = CreateCompatibleDC(memdc);
                        let oldq = SelectObject(qdc, qbmp as *mut winapi::ctypes::c_void);
                        StretchBlt(memdc, dx, dy, s as i32, s as i32,
                            qdc, 0, 0, *w as i32, *h as i32, SRCCOPY);
                        SelectObject(qdc, oldq);
                        DeleteObject(qbmp as *mut winapi::ctypes::c_void);
                        DeleteObject(qdc as *mut winapi::ctypes::c_void);
                    }
                }
            }

            // 一次性呈现到屏幕
            BitBlt(hdc, 0, 0, cw, ch, memdc, 0, 0, SRCCOPY);
            SelectObject(memdc, oldbmp);
            DeleteObject(hbmp as *mut winapi::ctypes::c_void);
            DeleteObject(memdc as *mut winapi::ctypes::c_void);
            EndPaint(hwnd, &ps);
            0
        }

        // 鼠标移动：检测按钮悬停
        WM_MOUSEMOVE => unsafe {
            let mut pt: POINT = std::mem::zeroed();
            GetCursorPos(&mut pt);
            let mut over = 0i32;
            for id in [QR_BTN_CLEAR as i32, QR_BTN_SCREEN as i32,
                       QR_BTN_COPY as i32, QR_BTN_DOWNLOAD as i32, QR_BTN_UPLOAD as i32,
                       OCR_BTN_REC as i32, OCR_BTN_COPY as i32, OCR_BTN_CLEAR as i32,
                       OCR_BTN_SEARCH as i32] {
                let btn = GetDlgItem(hwnd, id);
                if !btn.is_null() {
                    let mut wr: RECT = std::mem::zeroed();
                    GetWindowRect(btn, &mut wr);
                    if pt.x >= wr.left && pt.x < wr.right && pt.y >= wr.top && pt.y < wr.bottom {
                        over = id;
                        break;
                    }
                }
            }
            if BTN_HOVER.swap(over, Ordering::Relaxed) != over {
                InvalidateRect(hwnd, std::ptr::null_mut(), 1);
            }
            let mut tme: TRACKMOUSEEVENT = std::mem::zeroed();
            tme.cbSize = std::mem::size_of::<TRACKMOUSEEVENT>() as u32;
            tme.dwFlags = TME_LEAVE;
            tme.hwndTrack = hwnd;
            TrackMouseEvent(&mut tme);
            0
        }

        // 鼠标离开：清除悬停
        WM_MOUSELEAVE => unsafe {
            BTN_HOVER.store(0, Ordering::Relaxed);
            InvalidateRect(hwnd, std::ptr::null_mut(), 1);
            0
        }

        // 自定义圆角按钮绘制（蓝色）
        WM_DRAWITEM => unsafe {
            let dis = lp as *const DRAWITEMSTRUCT;
            if dis.is_null() { return 0; }
            let item = &*dis;
            let hdc = item.hDC;
            let mut r = item.rcItem;
            // 顶部页面标签：选中=蓝色底白字，未选=浅灰底深字
            if item.CtlID as usize == QR_TAB_OCR || item.CtlID as usize == QR_TAB_QR
                || item.CtlID as usize == QR_TAB_SHUT || item.CtlID as usize == QR_TAB_SET
                || item.CtlID as usize == QR_TAB_ANTI {
                let cur = PAGE_CUR.load(Ordering::Relaxed);
                let sel = (item.CtlID as usize == QR_TAB_OCR && cur == PAGE_OCR)
                    || (item.CtlID as usize == QR_TAB_QR && cur == PAGE_QR)
                    || (item.CtlID as usize == QR_TAB_SHUT && cur == PAGE_SHUT)
                    || (item.CtlID as usize == QR_TAB_ANTI && cur == PAGE_ANTI)
                    || (item.CtlID as usize == QR_TAB_SET && cur == PAGE_SET);
                // 四个标签用 41 版同款配色：浅灰底 rgb(226,232,240) + 深字 rgb(71,85,105)；选中用蓝色底部条 rgb(37,99,235)
                let brush = CreateSolidBrush(rgb(226, 232, 240));
                FillRect(hdc, &r, brush as HBRUSH);
                DeleteObject(brush as *mut winapi::ctypes::c_void);
                if sel {
                    let bar = CreateSolidBrush(rgb(37, 99, 235));
                    FillRect(hdc, &RECT { left: r.left, top: r.bottom - 4, right: r.right, bottom: r.bottom },
                        bar as HBRUSH);
                    DeleteObject(bar as *mut winapi::ctypes::c_void);
                }
                let oldcol = SetTextColor(hdc, rgb(71, 85, 105));
                SetBkMode(hdc, TRANSPARENT as i32);
                let f = CreateFontW(sc(18), 0, 0, 0, 500, 0, 0, 0,
                    DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS,
                    wide("Microsoft YaHei").as_ptr());
                let oldf = SelectObject(hdc, f as *mut winapi::ctypes::c_void);
                let mut rc = r;
                let label = if item.CtlID as usize == QR_TAB_OCR { "文字识别" }
                    else if item.CtlID as usize == QR_TAB_QR { "二维码" }
                    else if item.CtlID as usize == QR_TAB_SHUT { "定时关机" }
                    else if item.CtlID as usize == QR_TAB_ANTI { "防多开" }
                    else { "设置" };
                DrawTextW(hdc, wide(label).as_ptr(), -1, &mut rc,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(hdc, oldf);
                DeleteObject(f as *mut winapi::ctypes::c_void);
                SetTextColor(hdc, oldcol);
                return 0;
            }
            let pressed = (item.itemState & ODS_SELECTED) != 0;
            let hovering = BTN_HOVER.load(Ordering::Relaxed) == item.CtlID as i32;
            let is_arrow = matches!(item.CtlID as usize,
                SD_UP_DAY | SD_UP_H | SD_UP_M | SD_UP_S | SD_DN_DAY | SD_DN_H | SD_DN_M | SD_DN_S);
            let is_clear = item.CtlID == QR_BTN_CLEAR as u32 || item.CtlID == OCR_BTN_CLEAR as u32;
            let base = if is_clear {
                // 危险操作（清空）：红色系
                if pressed { rgb(C_DANGER_D.0, C_DANGER_D.1, C_DANGER_D.2) }
                else if hovering { rgb(C_DANGER_L.0, C_DANGER_L.1, C_DANGER_L.2) }
                else { rgb(C_DANGER.0, C_DANGER.1, C_DANGER.2) }
            } else if is_arrow {
                // ▲▼ 箭头：中性灰系
                if pressed { rgb(C_BTN_GR_D.0, C_BTN_GR_D.1, C_BTN_GR_D.2) } else if hovering { rgb(C_FILL_GR.0, C_FILL_GR.1, C_FILL_GR.2) } else { rgb(C_BTN_GR.0, C_BTN_GR.1, C_BTN_GR.2) }
            } else if item.CtlID == SD_PAUSE as u32 {
                // 暂停定时器：正在倒计时(未暂停)红色可用；未开始/已暂停灰色（已暂停仍可点恢复）
                if SD_RUNNING.load(Ordering::Relaxed) && !SD_PAUSED.load(Ordering::Relaxed) {
                    if pressed { rgb(C_DANGER_D.0, C_DANGER_D.1, C_DANGER_D.2) }
                    else if hovering { rgb(C_DANGER_L.0, C_DANGER_L.1, C_DANGER_L.2) }
                    else { rgb(C_DANGER.0, C_DANGER.1, C_DANGER.2) }
                } else {
                    if pressed { rgb(C_BTN_GR_D.0, C_BTN_GR_D.1, C_BTN_GR_D.2) } else if hovering { rgb(C_FILL_GR.0, C_FILL_GR.1, C_FILL_GR.2) } else { rgb(C_BTN_GR.0, C_BTN_GR.1, C_BTN_GR.2) }
                }
            } else if item.CtlID == SD_CLEAR as u32 {
                // 清空：未开始/已暂停=红色可用；正在倒计时(未暂停)=置灰不可点
                if SD_RUNNING.load(Ordering::Relaxed) && !SD_PAUSED.load(Ordering::Relaxed) {
                    rgb(C_BTN_GR.0, C_BTN_GR.1, C_BTN_GR.2)
                } else if (item.itemState & 0x0080 /*ODS_DISABLED*/) != 0 {
                    rgb(C_BTN_GR.0, C_BTN_GR.1, C_BTN_GR.2)
                } else if pressed { rgb(C_DANGER_D.0, C_DANGER_D.1, C_DANGER_D.2) }
                else if hovering { rgb(C_DANGER_L.0, C_DANGER_L.1, C_DANGER_L.2) }
                else { rgb(C_DANGER.0, C_DANGER.1, C_DANGER.2) }
            } else if item.CtlID == SD_START as u32 || item.CtlID == QR_BTN_DOWNLOAD as u32 || item.CtlID == OCR_BTN_REC as u32 {
                // 开始定时器 / 下载二维码 / 识别：与 41 复制按钮同色
                if pressed { rgb(C_BTN_BLUE_D.0, C_BTN_BLUE_D.1, C_BTN_BLUE_D.2) } else if hovering { rgb(C_BTN_BLUE_H.0, C_BTN_BLUE_H.1, C_BTN_BLUE_H.2) } else { rgb(C_BTN_BLUE.0, C_BTN_BLUE.1, C_BTN_BLUE.2) }
            } else {
                // 常规按钮（复制/识别二维码/复制二维码/上传LOGO/查找等）：与 41 复制按钮同色
                if pressed { rgb(C_BTN_BLUE_D.0, C_BTN_BLUE_D.1, C_BTN_BLUE_D.2) } else if hovering { rgb(C_BTN_BLUE_H.0, C_BTN_BLUE_H.1, C_BTN_BLUE_H.2) } else { rgb(C_BTN_BLUE.0, C_BTN_BLUE.1, C_BTN_BLUE.2) }
            };
            let brush = CreateSolidBrush(base);
            FillRect(hdc, &r, brush as HBRUSH);
            let rad = if is_arrow { 6 } else { 12 };
            let hrgn = CreateRoundRectRgn(r.left, r.top, r.right, r.bottom, rad, rad);
            FillRgn(hdc, hrgn, brush as HBRUSH);
            DeleteObject(brush as *mut winapi::ctypes::c_void);
            DeleteObject(hrgn as *mut winapi::ctypes::c_void);

            if is_arrow {
                // ▲▼ 用符号字体绘制，深色居中小号
                let glyph = if item.CtlID as usize == SD_UP_DAY || item.CtlID as usize == SD_UP_H
                    || item.CtlID as usize == SD_UP_M || item.CtlID as usize == SD_UP_S { "▲" } else { "▼" };
                let af = CreateFontW(-sc(16), 0, 0, 0, 600, 0, 0, 0,
                    DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                    CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS,
                    wide("Segoe UI Symbol").as_ptr());
                let olda = SelectObject(hdc, af as *mut winapi::ctypes::c_void);
                SetTextColor(hdc, rgb(C_TEXT.0, C_TEXT.1, C_TEXT.2));
                SetBkMode(hdc, TRANSPARENT as i32);
                DrawTextW(hdc, wide(glyph).as_ptr(), -1, &mut r,
                    DT_CENTER | DT_VCENTER | DT_SINGLELINE);
                SelectObject(hdc, olda);
                DeleteObject(af as *mut winapi::ctypes::c_void);
                return 0;
            }

            let label = btn_label(item.CtlID as u32);
            // 字体高度按 DPI 放大 + ClearType 渲染，保证文字清晰不虚；“全部关闭”按钮字号加大一号
            let fsize = if item.CtlID as usize == ANTI_BTN_CLOSE { 26 } else { 22 };
            let font = CreateFontW(sc(fsize), 0, 0, 0, FW_BOLD, 0, 0, 0,
                DEFAULT_CHARSET, OUT_DEFAULT_PRECIS, CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY, VARIABLE_PITCH | FF_SWISS,
                wide("Microsoft YaHei").as_ptr());
            let old = SelectObject(hdc, font as *mut winapi::ctypes::c_void);
            SetTextColor(hdc, 0xFFFFFF);
            SetBkMode(hdc, TRANSPARENT as i32);
            DrawTextW(hdc, wide(label).as_ptr(), -1, &mut r,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(hdc, old);
            DeleteObject(font as *mut winapi::ctypes::c_void);
            0
        }

        WM_COMMAND => unsafe {
            let id = (wp & 0xFFFF) as usize;
            let notify = (wp >> 16) & 0xFFFF;
            let edit = GetDlgItem(hwnd, QR_EDIT as i32);
            // 点击输入框时默认全选，方便直接覆盖输入
            if id == QR_EDIT && notify == 0x0100 /*EN_SETFOCUS*/ {
                SendMessageW(edit, 0x00B1 /*EM_SETSEL*/, 0, -1);
            }
            // 文字识别页搜索框：输入即定位识别结果
            if id == OCR_SEARCH_EDIT && notify == 0x0300 /*EN_CHANGE*/ {
                ocr_search_jump(hwnd);
            }
            // 输入内容变化：实时自动生成二维码（输入框固定填满，超出行内滚动）
            // 注意：输入框高度恒为 INPUT_H，"随内容自动增高再 relayout" 那套逻辑早已失效，
            // 原先的 if ih != INPUT_H { ... } 永远不成立，属于死分支，已删除
            if id == QR_EDIT && notify == 0x0300 /*EN_CHANGE*/ {
                generate_from_edit(hwnd, true);
            }
            match id {
                QR_TAB_OCR => {
                    PAGE_CUR.store(PAGE_OCR, Ordering::Relaxed);
                    apply_page(hwnd);
                    relayout(hwnd);
                    0
                }
                QR_TAB_QR => {
                    PAGE_CUR.store(PAGE_QR, Ordering::Relaxed);
                    apply_page(hwnd);
                    relayout(hwnd);
                    0
                }
                QR_TAB_SHUT => {
                    PAGE_CUR.store(PAGE_SHUT, Ordering::Relaxed);
                    apply_page(hwnd);
                    relayout(hwnd);
                    sd_update_status(hwnd);
                    0
                }
                QR_TAB_ANTI => {
                    PAGE_CUR.store(PAGE_ANTI, Ordering::Relaxed);
                    apply_page(hwnd);
                    relayout(hwnd);
                    0
                }
                QR_TAB_SET => {
                    PAGE_CUR.store(PAGE_SET, Ordering::Relaxed);
                    apply_page(hwnd);
                    relayout(hwnd);
                    0
                }
                ANTI_BTN_CLOSE => {
                    // 一键关闭：关闭所有开着的压缩软件窗口
                    close_archiver_windows();
                    0
                }
                SD_UP_DAY | SD_UP_H | SD_UP_M | SD_UP_S
                    | SD_DN_DAY | SD_DN_H | SD_DN_M | SD_DN_S => {
                    let (ed, dir) = match id {
                        SD_UP_DAY => (SD_ED_DAY, 1),
                        SD_DN_DAY => (SD_ED_DAY, -1),
                        SD_UP_H => (SD_ED_H, 1),
                        SD_DN_H => (SD_ED_H, -1),
                        SD_UP_M => (SD_ED_M, 1),
                        SD_DN_M => (SD_ED_M, -1),
                        SD_UP_S => (SD_ED_S, 1),
                        _ => (SD_ED_S, -1),
                    };
                    sd_nudge(hwnd, ed, dir);
                    0
                }
                SET_RAD_EXIT => {
                    save_dword(CLOSE_MODE_VALUE, 0);
                    let h = GetDlgItem(hwnd, SET_HINT as i32);
                    if !h.is_null() { SetWindowTextW(h, wide("关闭按钮将退出软件").as_ptr()); }
                    0
                }
                SET_RAD_TRAY => {
                    save_dword(CLOSE_MODE_VALUE, 1);
                    let h = GetDlgItem(hwnd, SET_HINT as i32);
                    if !h.is_null() { SetWindowTextW(h, wide("关闭按钮将最小化到托盘").as_ptr()); }
                    0
                }
                SET_RAD_HIDE_YES => {
                    save_dword(HIDE_ON_REC_VALUE, 1);
                    let h = GetDlgItem(hwnd, SET_HINT as i32);
                    if !h.is_null() { SetWindowTextW(h, wide("按识别快捷键时先隐藏主界面，避免遮挡").as_ptr()); }
                    0
                }
                SET_RAD_HIDE_NO => {
                    save_dword(HIDE_ON_REC_VALUE, 0);
                    let h = GetDlgItem(hwnd, SET_HINT as i32);
                    if !h.is_null() { SetWindowTextW(h, wide("按识别快捷键时不隐藏主界面").as_ptr()); }
                    0
                }
                SET_CHK_ENGINE => {
                    // BS_AUTOCHECKBOX 已经把勾选状态翻好了，这里直接读控件当前状态
                    let ck = GetDlgItem(hwnd, SET_CHK_ENGINE as i32);
                    let on = SendMessageW(ck, BM_GETCHECK as u32, 0, 0) as i32 == 1;
                    save_dword(KEEP_ENGINE_VALUE, if on { 1 } else { 0 });
                    let h = GetDlgItem(hwnd, SET_HINT as i32);
                    if on {
                        // 立刻把引擎拉起来并保持，别等下一次识别 ——
                        // 勾了却要"下一次才生效"会让人以为没勾上
                        match ocr_engine_warmup() {
                            Ok(()) => if !h.is_null() { SetWindowTextW(h,
                                wide("识别引擎常驻已开启：下次识别省去约 2~4 秒模型加载").as_ptr()); },
                            Err(e) => if !h.is_null() { SetWindowTextW(h, wide(&format!(
                                "识别引擎常驻开启失败：{}；已退回每次启动", e)).as_ptr()); },
                        }
                    } else {
                        // 关掉就把进程收干净，别让它挂在那儿占内存
                        ocr_engine_drop();
                        if !h.is_null() { SetWindowTextW(h,
                            wide("识别引擎常驻已关闭：每次识别后退出引擎进程").as_ptr()); }
                    }
                    0
                }
                OCR_BTN_SEARCH => {
                    ocr_search_jump(hwnd); // 手动“查找”：定位并高亮
                    0
                }
                SET_RECORD_REC | SET_RECORD_SHOW => {
                    let target = if id == SET_RECORD_REC { 0 } else { 1 };
                    let (_, _, _, _, _, _, _, hintmsg) = hotkey_row(target);
                    CAPTURING.store(true, Ordering::Relaxed);
                    CAPTURE_TARGET.store(target, Ordering::Relaxed);
                    SetTimer(hwnd, SET_TIMER_ID as usize, 40, None);
                    let h = GetDlgItem(hwnd, SET_HINT as i32);
                    if !h.is_null() { SetWindowTextW(h, wide(hintmsg).as_ptr()); }
                    0
                }
                OCR_BTN_REC => {
                    start_region_select(1); // 框选位置→识别文字（内部先隐藏主界面）
                    0
                }
                OCR_BTN_COPY => {
                    let t = OCR_TEXT.lock().unwrap().clone();
                    if t.is_empty() {
                        set_status(hwnd, "没有可复制的文字");
                    } else if copy_text_clipboard(&t) {
                        set_status(hwnd, "文字已复制到剪贴板");
                    } else {
                        set_status(hwnd, "复制失败");
                    }
                    0
                }
                OCR_BTN_CLEAR => {
                    *OCR_TEXT.lock().unwrap() = String::new();
                    let ocr = GetDlgItem(hwnd, OCR_EDIT as i32);
                    if !ocr.is_null() { SetWindowTextW(ocr, wide("").as_ptr()); }
                    // 同时清空顶部搜索框
                    let sbox = GetDlgItem(hwnd, OCR_SEARCH_EDIT as i32);
                    if !sbox.is_null() { SetWindowTextW(sbox, wide("").as_ptr()); }
                    update_ocr_count(hwnd);
                    0
                }
                SD_START => {
                    sd_start(hwnd);
                    0
                }
                SD_PAUSE => {
                    if SD_RUNNING.load(Ordering::Relaxed) {
                        let p = !SD_PAUSED.load(Ordering::Relaxed);
                        SD_PAUSED.store(p, Ordering::Relaxed);
                        sd_update_status(hwnd);
                    }
                    0
                }
                SD_CLEAR => {
                    // 清空：四个时间输入复位为 0/00，并停止倒计时
                    for (id, v) in [(SD_ED_DAY, "0"), (SD_ED_H, "00"), (SD_ED_M, "00"), (SD_ED_S, "00")] {
                        let c = sd_ctrl(hwnd, id);
                        if !c.is_null() { SetWindowTextW(c, wide(v).as_ptr()); }
                    }
                    if SD_RUNNING.load(Ordering::Relaxed) {
                        KillTimer(hwnd, SD_TIMER_ID as usize);
                        SD_RUNNING.store(false, Ordering::Relaxed);
                        SD_PAUSED.store(false, Ordering::Relaxed);
                    }
                    sd_update_status(hwnd);
                    0
                }
                QR_BTN_CLEAR => {
                    // 清空：输入框清空、LOGO 清除，但二维码回到默认内容（不清丢）
                    let edit = GetDlgItem(hwnd, QR_EDIT as i32);
                    if !edit.is_null() { SetWindowTextW(edit, wide("").as_ptr()); }
                    *LOGO_IMG.lock().unwrap() = None;
                    *QR_TEXT.lock().unwrap() = String::new();
                    *QR_IMG.lock().unwrap() = gen_qr_bgra("请输入文本内容！");
                    InvalidateRect(hwnd, std::ptr::null_mut(), 1);
                    0
                }
                QR_BTN_SCREEN => {
                    set_status(hwnd, "拖拽框选：识别二维码…（Esc 取消）");
                    start_region_select(2); // 先识别二维码，无则识别文字（内部先隐藏主界面）
                    0
                }
                QR_BTN_COPY => {
                    // 复制二维码：只写 CF_BITMAP 图片格式，粘贴只出图、不出文字
                    let guard = QR_IMG.lock().unwrap();
                    if let Some((w, h, buf)) = &*guard {
                        copy_qr_full_clipboard(*w, *h, buf);
                    }
                    0
                }
                QR_BTN_DOWNLOAD => {
                    // 下载二维码：以整数模块边长生成（网格均匀、边缘锐利，尺寸最接近 500）
                    let text = QR_TEXT.lock().unwrap().clone();
                    let content = if text.trim().is_empty() { "请输入文本内容！".to_string() } else { text };
                    if let Some(path) = pick_save_png_path(hwnd) {
                        match gen_qr_bgra_int(&content, 500) {
                            Some((w, h, buf)) => {
                                if save_bgra_png(w, h, &buf, &path) {
                                    set_status(hwnd, &format!("二维码已保存：{}", path));
                                } else {
                                    set_status(hwnd, "保存失败");
                                }
                            }
                            None => set_status(hwnd, "生成二维码失败"),
                        }
                    }
                    0
                }
                QR_BTN_UPLOAD => {
                    // “上传LOGO”：选图后直接叠加到二维码中央（白色圆角矩形，H/30% 容错）
                    if let Some(path) = pick_logo_file(hwnd) {
                        match load_logo_bgra(&path) {
                            Some(logo) => {
                                *LOGO_IMG.lock().unwrap() = Some(logo);
                                // 若输入框为空则先用默认内容生成，保证 logo 立刻可见
                                let edit = GetDlgItem(hwnd, QR_EDIT as i32);
                                let mut buf = [0u16; 4096];
                                let n = GetWindowTextW(edit, buf.as_mut_ptr(), 4096);
                                let txt = String::from_utf16_lossy(&buf[..n as usize]).trim().to_string();
                                if txt.is_empty() && !edit.is_null() {
                                    SetWindowTextW(edit, wide("请输入文本内容！").as_ptr());
                                }
                                generate_from_edit(hwnd, true);
                            }
                            None => {}
                        }
                    }
                    0
                }
                _ => DefWindowProcW(hwnd, msg, wp, lp),
            }
        }

        WM_TIMER => unsafe {
            // 定时关机倒计时
            if wp as usize == SD_TIMER_ID {
                if SD_RUNNING.load(Ordering::Relaxed) && !SD_PAUSED.load(Ordering::Relaxed) {
                    let r = SD_REMAIN.load(Ordering::Relaxed);
                    if r <= 0 {
                        // 先停表、清运行态，再执行——避免执行期间 WM_TIMER 再次触发
                        KillTimer(hwnd, SD_TIMER_ID as usize);
                        SD_RUNNING.store(false, Ordering::Relaxed);
                        SD_PAUSED.store(false, Ordering::Relaxed);
                        let done = sd_execute(hwnd);
                        SD_END.store(if done { 1 } else { 2 }, Ordering::Relaxed);
                        sd_update_status(hwnd);
                        InvalidateRect(hwnd, std::ptr::null_mut(), 1);
                    } else {
                        SD_REMAIN.store(r - 1, Ordering::Relaxed);
                        // 最后 10 秒：自动弹出主界面并切到“定时关机”标签页，方便暂停/取消（按原软件逻辑）
                        if SD_REMAIN.load(Ordering::Relaxed) <= 10
                            && !SD_ALERT_SHOWN.load(Ordering::Relaxed) {
                            SD_ALERT_SHOWN.store(true, Ordering::Relaxed);
                            if PAGE_CUR.load(Ordering::Relaxed) != PAGE_SHUT {
                                PAGE_CUR.store(PAGE_SHUT, Ordering::Relaxed);
                                apply_page(hwnd);
                                relayout(hwnd);
                            }
                            if IsWindowVisible(hwnd) == 0 {
                                ShowWindow(hwnd, SW_SHOW);
                            }
                            SetForegroundWindow(hwnd);
                        }
                        sd_update_status(hwnd);
                    }
                }
                return 0;
            }
            // 设置页快捷键录制
            if wp as usize == SET_TIMER_ID {
                if CAPTURING.load(Ordering::Relaxed) {
                    let hint = GetDlgItem(hwnd, SET_HINT as i32);
                    if GetAsyncKeyState(VK_ESCAPE) as u16 & 0x8000 != 0 {
                        CAPTURING.store(false, Ordering::Relaxed);
                        KillTimer(hwnd, SET_TIMER_ID as usize);
                        if !hint.is_null() { SetWindowTextW(hint, wide("已取消录制，快捷键未改动").as_ptr()); }
                        return 0;
                    }
                    let mut pressed: Option<u32> = None;
                    for vk in 0x30..=0x39 { if GetAsyncKeyState(vk) as u16 & 1 != 0 { pressed = Some(vk as u32); } }
                    for vk in 0x41..=0x5A { if GetAsyncKeyState(vk) as u16 & 1 != 0 { pressed = Some(vk as u32); } }
                    for vk in 0x70..=0x7B { if GetAsyncKeyState(vk) as u16 & 1 != 0 { pressed = Some(vk as u32); } }
                    if GetAsyncKeyState(0x20) as u16 & 1 != 0 { pressed = Some(0x20); }
                    if GetAsyncKeyState(0x0D) as u16 & 1 != 0 { pressed = Some(0x0D); }
                    if let Some(vk) = pressed {
                        let mut m = 0u32;
                        if GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000 != 0 { m |= MOD_CONTROL as u32; }
                        if GetAsyncKeyState(VK_MENU) as u16 & 0x8000 != 0 { m |= MOD_ALT as u32; }
                        if GetAsyncKeyState(VK_SHIFT) as u16 & 0x8000 != 0 { m |= MOD_SHIFT as u32; }
                        if (GetAsyncKeyState(VK_LWIN) as u16 | GetAsyncKeyState(VK_RWIN) as u16) & 0x8000 != 0 { m |= MOD_WIN as u32; }
                        let target = CAPTURE_TARGET.load(Ordering::Relaxed);
                        let (combo_id, _, store, vname, _, _, _, _) = hotkey_row(target);
                        let disp = GetDlgItem(hwnd, combo_id as i32);
                        let name = hotkey_name(m, vk);
                        if !disp.is_null() {
                            SendMessageW(disp, 0x000C, 0, wide(&name).as_ptr() as LPARAM);
                        }
                        *store.lock().unwrap() = Some((m, vk, format!("自定义：{}", name)));
                        save_hotkey(vname, m, vk);
                        let ok = apply_hotkeys();
                        HOTKEY_OK.store(((ok.0 as i32) | ((ok.1 as i32) << 1)) as i32, Ordering::Relaxed);
                        CAPTURING.store(false, Ordering::Relaxed);
                        KillTimer(hwnd, SET_TIMER_ID as usize);
                        // 录完立即把注册结果回显：占用了别人/被自己占用都当场说清楚，不留"看着设好了其实没生效"
                        let st = hotkey_status_text(
                            load_hotkey(HOTKEY_REC_VALUE, 0, 0x70),
                            load_hotkey(HOTKEY_SHOW_VALUE, 0, 0x79),
                            ok);
                        if !hint.is_null() { SetWindowTextW(hint, wide(&st).as_ptr()); }
                    }
                }
                return 0;
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }

        WM_DESTROY => unsafe {
            let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
            nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            nid.hWnd = hwnd;
            nid.uID = 1;
            Shell_NotifyIconW(NIM_DELETE, &mut nid);
            UnregisterHotKey(hwnd, WM_HOTKEY_REC_ID);
            UnregisterHotKey(hwnd, WM_HOTKEY_SHOW_ID);
            KillTimer(hwnd, SD_TIMER_ID as usize);
            KillTimer(hwnd, SET_TIMER_ID as usize);
            // 收掉可能还挂着的 OCR 引擎子进程（只在开了"引擎常驻"时才会有）
            ocr_engine_drop();
            // 关掉全屏倒计时窗口
            let ef = EDIT_FONT.load(Ordering::Relaxed);
            if ef != 0 {
                DeleteObject(ef as *mut winapi::ctypes::c_void);
                EDIT_FONT.store(0, Ordering::Relaxed);
            }
            PostQuitMessage(0);
            0
        },
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

// 底部状态提示已移除：改用轻提示弹窗反馈，避免复制/保存等结果静默丢失
fn set_status(hwnd: HWND, s: &str) {
    if !hwnd.is_null() {
        unsafe { show_toast(hwnd, s); }
    }
}

// 自绘按钮文字
fn btn_label(id: u32) -> &'static str {
    match id {
        4102 => "下载二维码",
        4103 => "识别二维码",
        4105 => "复制二维码",
        4106 => "上传 LOGO",
        4107 => "清空",
        4191 => "识别",
        4192 => "复制",
        4193 => "清空",
        4195 => "查找",
        4560 => "开始定时器",
        4561 => "暂停定时器",
        4563 => "清空",
        4700 => "全部关闭",
        _ => "",
    }
}

// ==================== OCR：驱动 snow-ocr-process.exe（自带 PaddleOCR+DirectML 引擎）====================
// 协议见 snow-ocr-process/src/protocol.rs：stdin/stdout 二进制帧，20 字节头(SOCR+ver+kind+req+len)+载荷
const OCR_MAGIC: [u8; 4] = *b"SOCR";
const OCR_VER: u16 = 5;
const K_HELLO: u16 = 1;
const K_READY: u16 = 2;
const K_SUBMIT: u16 = 3;
const K_COMPLETE: u16 = 5;
const K_SHUTDOWN: u16 = 6;
const K_PREPARE: u16 = 8;
const K_SESSION_READY: u16 = 9;
const K_ATTACH: u16 = 12;
const K_BUFFER_ATTACHED: u16 = 13;
const K_RECOGNIZE: u16 = 14;
const K_IMAGE_CONSUMED: u16 = 15;
const K_DETACH: u16 = 16;

fn ocr_put_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn ocr_write_frame(w: &mut impl Write, kind: u16, req: u64, payload: &[u8]) -> io::Result<()> {
    let mut h = [0u8; 20];
    h[..4].copy_from_slice(&OCR_MAGIC);
    h[4..6].copy_from_slice(&OCR_VER.to_le_bytes());
    h[6..8].copy_from_slice(&kind.to_le_bytes());
    h[8..16].copy_from_slice(&req.to_le_bytes());
    h[16..20].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    w.write_all(&h)?;
    w.write_all(payload)?;
    w.flush()
}

fn ocr_read_frame(r: &mut impl Read) -> io::Result<(u16, u64, Vec<u8>)> {
    let mut h = [0u8; 20];
    r.read_exact(&mut h)?;
    if &h[..4] != &OCR_MAGIC {
        return Err(io::Error::new(io::ErrorKind::Other, "OCR 协议魔数错误"));
    }
    let kind = u16::from_le_bytes([h[6], h[7]]);
    let req = u64::from_le_bytes(h[8..16].try_into().unwrap());
    let len = u32::from_le_bytes(h[16..20].try_into().unwrap()) as usize;
    let mut p = vec![0u8; len];
    r.read_exact(&mut p)?;
    Ok((kind, req, p))
}

// 定位 OCR 资产：全部内嵌进 exe，首次使用时释放到 %LOCALAPPDATA%\Xin OCR_ocr（单文件交付）
static OCR_EXTRACT_LOCK: Mutex<()> = Mutex::new(());

fn ocr_cache_dir() -> PathBuf {
    if let Ok(p) = std::env::var("LOCALAPPDATA") {
        let d = PathBuf::from(p).join("Xin OCR_ocr");
        return d;
    }
    std::env::temp_dir().join("Xin OCR_ocr")
}

fn ensure_ocr_assets() -> Option<(PathBuf, PathBuf, PathBuf, PathBuf)> {
    let _g = OCR_EXTRACT_LOCK.lock().unwrap();
    let dir = ocr_cache_dir();
    let proc = dir.join("snow-ocr-process.exe");
    let det = dir.join("PP-OCRv6_det_small.onnx");
    let rec = dir.join("PP-OCRv6_rec_small.onnx");
    let dict = dir.join("ppocrv6_dict.txt");
    let dll = dir.join("DirectML.dll");
    let marker = dir.join(".v1");
    if !marker.exists() || !proc.exists() || !dll.exists() || !det.exists() || !rec.exists() || !dict.exists() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(&proc, include_bytes!("../assets/snow-ocr-process.exe"));
        let _ = std::fs::write(&dll, include_bytes!("../assets/DirectML.dll"));
        let _ = std::fs::write(&det, include_bytes!("../assets/PP-OCRv6_det_small.onnx"));
        let _ = std::fs::write(&rec, include_bytes!("../assets/PP-OCRv6_rec_small.onnx"));
        let _ = std::fs::write(&dict, include_bytes!("../assets/ppocrv6_dict.txt"));
        let _ = std::fs::write(&marker, b"1");
    }
    if proc.exists() && det.exists() && rec.exists() && dict.exists() {
        Some((proc, det, rec, dict))
    } else {
        None
    }
}

// ===== OCR 引擎：默认"用完即走"，可选常驻 =====
// 引擎是 snow-ocr-process.exe（PaddleOCR + DirectML）。两种运行方式，由设置页
// 「识别引擎常驻」勾选框决定（注册表 KeepEngine）：
//   · 常驻关闭（默认）：每次识别 spawn 一个子进程，识别完立刻 Detach/Shutdown 退出。
//     引擎只在那两三秒里存在，平时不占任何内存；代价是每次都重新加载模型。
//   · 常驻开启：第一次识别时拉起并保持，之后复用同一条 stdin/stdout，
//     省掉每次约 2~4 秒的模型加载，代价是进程一直占着约 100MB。
// 两种方式对外都是 run_ocr_bgra 一个入口，调用方不用关心走的是哪条路。

// 引擎进程 + 它的两条管道。只在常驻模式下被填上
struct OcrEngine {
    child: std::process::Child,
    // 用 Option 包着：收尾时要能"关掉写端"（赋 None）从而给子进程发 EOF，
    // ChildStdin 本身不实现 Default，没法在原地换成一个空值
    stdin: Option<std::process::ChildStdin>,
    stdout: std::process::ChildStdout,
}
static OCR_ENGINE: Mutex<Option<OcrEngine>> = Mutex::new(None);

// 串行化整轮识别：两种模式下整条管道都只能有一个写者，两帧并发会把协议帧搅乱
static OCR_CALL_LOCK: Mutex<()> = Mutex::new(());

fn ocr_keep_engine() -> bool {
    load_dword(KEEP_ENGINE_VALUE, 0) == 1
}

// 拉起引擎并完成 Hello + PrepareSession（模型加载就在这一步）。
// 成功返回可用的引擎，失败时子进程已被清理。
fn ocr_engine_spawn() -> Result<OcrEngine, String> {
    let Some((proc, det, rec, dict)) = ensure_ocr_assets() else {
        return Err("OCR 引擎释放失败（无写入权限）".to_string());
    };
    let mut child = match Command::new(&proc)
        .creation_flags(0x08000000) // CREATE_NO_WINDOW：抑制控制台窗口弹出
        .stdin(Stdio::piped()).stdout(Stdio::piped())
        .stderr(Stdio::null()).spawn() {
        Ok(c) => c,
        Err(e) => return Err(format!("无法启动 OCR 引擎：{}", e)),
    };
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = child.stdout.take().unwrap();
    // Hello → Ready
    let mut payload = Vec::new();
    ocr_put_string(&mut payload, "");
    if ocr_write_frame(&mut stdin, K_HELLO, 0, &payload).is_err() {
        let _ = child.kill(); return Err("OCR 通信失败".into());
    }
    if let Ok((K_READY, _, _)) = ocr_read_frame(&mut stdout) {} else {
        let _ = child.kill(); return Err("OCR 未就绪".into());
    }
    // PrepareSession → SessionReady
    let mut payload = Vec::new();
    payload.push(1); // directml
    payload.push(0); // 检测resize=max
    payload.push(0); // 处理=accuracy first
    ocr_put_string(&mut payload, &det.to_string_lossy());
    ocr_put_string(&mut payload, &rec.to_string_lossy());
    ocr_put_string(&mut payload, &dict.to_string_lossy());
    if ocr_write_frame(&mut stdin, K_PREPARE, 1, &payload).is_err() {
        let _ = child.kill(); return Err("OCR 通信失败".into());
    }
    match ocr_read_frame(&mut stdout) {
        Ok((K_SESSION_READY, _, p)) if p.first() == Some(&1) => {}
        _ => { let _ = child.kill(); return Err("OCR 模型加载失败".into()); }
    }
    Ok(OcrEngine { child, stdin: Some(stdin), stdout })
}

// 设置页勾上"引擎常驻"时立刻预热：把进程拉起来并加载好模型，
// 这样用户第一次按识别就是热的。已经在了就直接返回成功。
fn ocr_engine_warmup() -> Result<(), String> {
    let _call = OCR_CALL_LOCK.lock().map_err(|_| "OCR 引擎忙")?;
    // 先看一眼在不在，别在持锁状态下做几秒的模型加载
    if OCR_ENGINE.lock().map_err(|_| "OCR 引擎忙")?.is_some() {
        return Ok(());
    }
    let eng = ocr_engine_spawn()?; // 模型加载在这里，锁外做
    *OCR_ENGINE.lock().map_err(|_| "OCR 引擎忙")? = Some(eng);
    Ok(())
}

// 收掉常驻引擎（关掉开关、或进程退出时调）。不在常驻状态时是空操作。
fn ocr_engine_drop() {
    // 先把引擎从全局槽里取出来再放锁：ocr_engine_shutdown 可能要等 1.5 秒，
    // 持着 OCR_ENGINE 的锁等这么久，会把主线程上任何想碰引擎的操作都堵死
    let taken = { OCR_ENGINE.lock().unwrap().take() };
    if let Some(mut e) = taken {
        ocr_engine_shutdown(&mut e, true);
    }
}

// 把引擎关掉：先（常驻模式下）发 Shutdown，然后关掉 stdin 让子进程看到 EOF，
// 等它自己退；1.5 秒还不走才 kill。
// 之所以不直接 kill：一上来就杀会让 DirectML 设备来不及释放，显存要等系统回收
fn ocr_engine_shutdown(e: &mut OcrEngine, graceful: bool) {
    if graceful {
        if let Some(si) = e.stdin.as_mut() {
            let _ = ocr_write_frame(si, K_SHUTDOWN, 0, &[]);
        }
    }
    // 丢掉 ChildStdin → 管道的写端关闭 → 子进程读到 EOF，走正常退出流程
    e.stdin = None;
    let mut waited = 0;
    loop {
        match e.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if waited < 1500 => {
                std::thread::sleep(Duration::from_millis(50));
                waited += 50;
            }
            _ => { let _ = e.child.kill(); let _ = e.child.wait(); break; }
        }
    }
}

// 对一张 BGRA 图做 OCR，返回识别文字（多行以换行拼接）
fn run_ocr_bgra(w: u32, h: u32, bgra: &[u8]) -> String {
    if w == 0 || h == 0 || (w as u64 * h as u64) > 3840 * 2160 {
        return "图片尺寸超限".to_string();
    }
    // 串行化：第二帧等第一帧收完再进来，而不是并发抢管道
    let _call = match OCR_CALL_LOCK.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(), // 上一轮若 panic 过，锁中毒也照常继续
    };

    let keep = ocr_keep_engine();
    let text;
    if keep {
        // 常驻模式：复用已有引擎，没有就现拉一个。guard 活到本轮结束，锁着 OCR_ENGINE
        let mut guard = OCR_ENGINE.lock().unwrap();
        if guard.is_none() {
            match ocr_engine_spawn() {
                Ok(e) => *guard = Some(e),
                Err(e) => return e,
            }
        }
        let eng = guard.as_mut().unwrap();
        text = ocr_round(eng, w, h, bgra);
        // 协议走坏说明这条管道已经不能再用（子进程挂了 / 帧错位），
        // 当场把进程收掉，下次识别重新拉起，免得一直卡在坏状态上
        let broken = text.starts_with("OCR 通信失败")
            || text.starts_with("OCR 缓冲区错误")
            || text.starts_with("OCR 图像错误")
            || text.starts_with("OCR 识别失败");
        if broken {
            if let Some(mut e) = guard.take() {
                ocr_engine_shutdown(&mut e, false);
            }
        }
    } else {
        // 一次性模式：现拉一个，识别完就关掉 —— 引擎只在识别这几秒里存在
        let mut eng = match ocr_engine_spawn() {
            Ok(e) => e,
            Err(e) => return e,
        };
        text = ocr_round(&mut eng, w, h, bgra);
        ocr_engine_shutdown(&mut eng, true);
    }
    text
}

// 单轮识别：Attach → Submit → Recognize → Detach。所有协议错误都以字符串返回
fn ocr_round(eng: &mut OcrEngine, w: u32, h: u32, bgra: &[u8]) -> String {
    // stdin 是 Option：ocr_engine_shutdown 里会被置 None。这里若已是 None，
    // 说明引擎已经被收过一轮，不能再写帧
    let Some(stdin) = eng.stdin.as_mut() else { return "OCR 引擎已关闭".to_string() };
    let stdout = &mut eng.stdout;
    // 写共享槽文件：32字节槽头 + BGRA
    let stride = w * 4;
    let size = (stride * h) as usize;
    let bytes = 32u64 + size as u64;
    let slot = std::env::temp_dir().join(format!("qr_ocr_{}_{}.buf",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    {
        let mut f = match File::create(&slot) {
            Ok(f) => f,
            Err(_) => return "无法创建临时文件".to_string(),
        };
        let _ = f.write_all(&1u64.to_le_bytes());          // sequence
        let _ = f.write_all(&1u32.to_le_bytes());          // state = ready
        let _ = f.write_all(&w.to_le_bytes());             // width
        let _ = f.write_all(&h.to_le_bytes());             // height
        let _ = f.write_all(&stride.to_le_bytes());        // stride
        let _ = f.write_all(&(size as u32).to_le_bytes()); // bytes
        let _ = f.write_all(&0x544f4c53u32.to_le_bytes()); // magic "SLOT"
        let _ = f.write_all(bgra);
        let _ = f.sync_all();
    }
    // 协议走坏时的统一收尾：删临时文件并返回错误文案
    macro_rules! ocr_fail {
        ($slot:expr, $msg:expr) => {{
            let _ = std::fs::remove_file(&$slot);
            return $msg.to_string();
        }};
    }
    // AttachBuffer → BufferAttached
    let mut payload = Vec::new();
    ocr_put_string(&mut payload, &slot.to_string_lossy());
    payload.extend_from_slice(&bytes.to_le_bytes());
    if ocr_write_frame(stdin, K_ATTACH, 2, &payload).is_err() { ocr_fail!(slot, "OCR 通信失败"); }
    if let Ok((K_BUFFER_ATTACHED, _, _)) = ocr_read_frame(stdout) {} else { ocr_fail!(slot, "OCR 缓冲区错误"); }
    // Submit → ImageConsumed；generation 必须与 Attach 的 req_id 一致
    let mut payload = Vec::new();
    payload.extend_from_slice(&2u64.to_le_bytes());  // generation = attach req_id
    payload.extend_from_slice(&w.to_le_bytes());
    payload.extend_from_slice(&h.to_le_bytes());
    payload.extend_from_slice(&stride.to_le_bytes());
    payload.extend_from_slice(&1u64.to_le_bytes());  // sequence
    if ocr_write_frame(stdin, K_SUBMIT, 3, &payload).is_err() { ocr_fail!(slot, "OCR 通信失败"); }
    if let Ok((K_IMAGE_CONSUMED, _, _)) = ocr_read_frame(stdout) {} else { ocr_fail!(slot, "OCR 图像错误"); }
    // Recognize → Complete
    if ocr_write_frame(stdin, K_RECOGNIZE, 3, &[]).is_err() { ocr_fail!(slot, "OCR 通信失败"); }
    let mut text = String::new();
    match ocr_read_frame(stdout) {
        Ok((K_COMPLETE, _, p)) if p.first() == Some(&1) => {
            let mut off = 1usize;
            // 跳过空字符串 ""（len+0）
            if p.len() > off + 4 {
                let l = u32::from_le_bytes(p[off..off + 4].try_into().unwrap()) as usize;
                off += 4 + l;
            }
            let n = if p.len() > off + 4 { u32::from_le_bytes(p[off..off + 4].try_into().unwrap()) as usize } else { 0 };
            off += 4;
            for _ in 0..n {
                if p.len() > off + 4 {
                    let l = u32::from_le_bytes(p[off..off + 4].try_into().unwrap()) as usize;
                    off += 4;
                    if off + l <= p.len() {
                        let s = String::from_utf8_lossy(&p[off..off + l]).into_owned();
                        off += l;
                        if !text.is_empty() { text.push_str("\r\n"); } // Windows 编辑框需 \r\n 才换行，避免识别行连成一段
                        text.push_str(&s);
                    }
                }
                off += 36; // score(4) + quad(8 f32=32)
            }
        }
        Ok(_) => text = "未识别到文字".into(),
        _ => text = "OCR 识别失败或超时".into(),
    }
    // Detach 释放这一轮的槽位
    let _ = ocr_write_frame(stdin, K_DETACH, 2, &[]);
    let _ = ocr_read_frame(stdout); // BufferDetached
    let _ = std::fs::remove_file(&slot);
    text
}

// 开始截图框选。
// mode: 0=仅二维码, 1=仅文字OCR, 2=先二维码、无则文字（与原 SEL_MODE 语义一致）。
// 实现在 ScreenCapture 静态库里（sc_api）：框选、放大镜、选区调整、标注工具条全都用它那一套，
// 本程序不再自己画遮罩窗口。返回时只是"已进入框选"，结果走 sc_on_done / sc_on_cancel 回调。
fn start_region_select(mode: i32) {
    unsafe {
        // 防重入：连按识别热键时，若截图界面还开着就先取消它再开新的，
        // 而不是叠出第二个遮罩（sc 侧本身也会拒绝，这里提前拦一道，顺便复位状态）
        if sc::sc_is_capturing() != 0 {
            sc::sc_cancel();
            return;
        }
        SEL_MODE.store(mode, Ordering::Relaxed);
        SEL_QR_HIT.store(false, Ordering::Relaxed);

        // 是否隐藏主界面：默认隐藏（设置里可选"不隐藏"）；隐藏时强制合成器立即应用 + 泵消息 + 等待，
        // 确保主界面真正从屏幕消失，抓屏不会带上本程序界面（否则会挡住待识别内容）
        let hide = load_dword(HIDE_ON_REC_VALUE, 1) == 1;
        let mh = MAIN_HWND.load(Ordering::Relaxed) as HWND;
        if hide && !mh.is_null() {
            ShowWindow(mh, SW_HIDE);
            DwmFlush();
            let mut msg: MSG = std::mem::zeroed();
            let mut n = 0;
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT { break; }
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
                n += 1;
                if n > 400 { break; }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            // 防御：若主界面又被显示回来，再隐藏一次
            if !mh.is_null() && IsWindowVisible(mh) != 0 {
                ShowWindow(mh, SW_HIDE);
            }
        }

        // sc 的模式：0=读文字, 1=读二维码, 2=进标注界面。
        // 注意这里的对应关系：本程序 mode=2（先码后字）在 sc 里走"纯框选出图"，判断在 Rust 侧做，
        // 所以也映射成 SC_MODE_OCR —— sc 只负责把那一块像素交回来，认什么由我们决定
        let sc_mode = match mode {
            0 => sc::SC_MODE_QR,
            _ => sc::SC_MODE_OCR,
        };
        let r = sc::sc_start_capture(sc_mode);
        if r == sc::SC_OK { return; }

        // 起不来：把主界面还回去，否则用户会觉得"按了快捷键程序消失了"
        if !mh.is_null() {
            ShowWindow(mh, SW_SHOW);
            SetForegroundWindow(mh);
            SetFocus(mh);
        }
        let why = match r {
            sc::SC_ERR_NOT_INIT => "截图组件未初始化",
            sc::SC_ERR_BUSY => "上一次截图还没结束",
            sc::SC_ERR_INTERNAL => "截图组件内部错误",
            _ => "截图启动失败",
        };
        // 走统一收尾：主界面已经在上面的 if 里恢复了，这里只负责把原因交出去
        *RECOG_RESULT.lock().unwrap() = why.to_string();
        if !mh.is_null() {
            PostMessageW(mh, WM_QR_RESULT, 0, 0);
        }
    }
}

// ===== sc（ScreenCapture 静态库）回调 =====
// 这两个回调由 sc 在自己的 UI 线程上同步调用（截图窗口刚 close()，消息循环还没转下一轮）。
// 因此：
//   · 可以操作窗口，但不能在这里做耗时的事 —— 要弹自己的界面、要跑识别，都先记下结果再异步做；
//   · 不能复用传进来的 mode（那是 sc 的 SC_MODE_*，只有"二维码/出图"两档），
//     本程序的三档语义（仅码 / 仅字 / 先码后字）存在 SEL_MODE 里，这里只读它。

/// 截到图了：按当前模式决定走二维码还是 OCR
///
/// 这是从 C++ 静态库跨回来的 FFI 边界，函数体里绝不能 panic（release 是 panic=abort，
/// 一次 unwrap 失败就会把整个程序带走）。所以下面一律用 match / if let，不用 unwrap。
pub extern "C" fn sc_on_done(
    result: i32,
    _mode: i32,
    w: i32,
    h: i32,
    pixels: *const u8,
    _png: *const u8,
    _png_len: i32,
    _user: *mut c_void,
) {
    // 先把像素拷出来：pixels 只在本次回调期间有效，识别要放到线程里跑
        let cap = if result == sc::SC_OK { sc::copy_bgra(w, h, pixels) } else { None };
        let mode = SEL_MODE.load(Ordering::Relaxed);

        if cap.is_none() {
            let why = match result {
                sc::SC_ERR_NO_PIXELS => "截图失败：取像素出错",
                sc::SC_ERR_ENCODE => "截图失败：图片编码出错",
                sc::SC_OK => "截图失败：图像为空",
                _ => "截图失败",
            };
            finish_shot(why.to_string());
            return;
        }
        let (bw, bh, bgra) = cap.unwrap();

    // 识别放后台：OCR 要拉子进程、跑模型，几百毫秒到几秒，不能卡住 sc 的 UI 线程
    std::thread::spawn(move || {
        let gray = bgra_to_gray(&bgra, bw, bh);
        let text = match mode {
            0 => {
                SEL_QR_HIT.store(true, Ordering::Relaxed);
                decode_qr(&gray, bw, bh).unwrap_or_else(|| "未识别到二维码".to_string())
            }
            1 => {
                SEL_QR_HIT.store(false, Ordering::Relaxed);
                run_ocr_bgra(bw, bh, &bgra)
            }
            _ => match decode_qr(&gray, bw, bh) {
                Some(t) => { SEL_QR_HIT.store(true, Ordering::Relaxed); t }
                None => {
                    SEL_QR_HIT.store(false, Ordering::Relaxed);
                    run_ocr_bgra(bw, bh, &bgra)
                }
            },
        };
        finish_shot(text);
    });
}

/// 用户取消了截图（Esc / 右键退出）：把主界面还回去，不产生结果
pub extern "C" fn sc_on_cancel(_mode: i32, _user: *mut c_void) {
    unsafe {
        let mh = MAIN_HWND.load(Ordering::Relaxed) as HWND;
        if !mh.is_null() {
            ShowWindow(mh, SW_SHOW);
            SetForegroundWindow(mh);
            SetFocus(mh);
        }
    }
}

/// 截图收尾：恢复主界面 → 记下结果 → 通知主线程回填。
///
/// 成功和失败都走这里，避免"失败时主界面不回来"这类漏分支。
/// 注意：这里会被 FFI 回调链调用，所以锁中毒不能 unwrap（panic=abort 会带走整个进程），
/// 用 `if let Ok` 退化成"这次结果丢了"，界面照样能回来。
fn finish_shot(text: String) {
    unsafe {
        let mh = MAIN_HWND.load(Ordering::Relaxed) as HWND;
        if !mh.is_null() {
            ShowWindow(mh, SW_SHOW);
            SetForegroundWindow(mh);
        }
        if let Ok(mut slot) = RECOG_RESULT.lock() {
            *slot = text;
        }
        if !mh.is_null() {
            PostMessageW(mh, WM_QR_RESULT, 0, 0);
        }
    }
}

/// BGRA → 8 位灰度（二维码解码只需要灰度）
fn bgra_to_gray(bgra: &[u8], w: u32, h: u32) -> Vec<u8> {
    let n = (w as usize) * (h as usize);
    let mut out = vec![0u8; n];
    for i in 0..n {
        let b = bgra[i * 4] as u32;
        let g = bgra[i * 4 + 1] as u32;
        let r = bgra[i * 4 + 2] as u32;
        // 299/587/114 加权，和 sc 侧旧实现、以及常见灰度化公式一致
        out[i] = ((r * 299 + g * 587 + b * 114) / 1000) as u8;
    }
    out
}

// 把 (mod, vk) 转成可读快捷键名（支持任意自定义）
fn hotkey_name(m: u32, vk: u32) -> String {
    if m == 0 && vk == 0 { return "无".to_string(); }
    let mut parts: Vec<&str> = Vec::new();
    if m & MOD_CONTROL as u32 != 0 { parts.push("Ctrl"); }
    if m & MOD_ALT as u32 != 0 { parts.push("Alt"); }
    if m & MOD_SHIFT as u32 != 0 { parts.push("Shift"); }
    if m & MOD_WIN as u32 != 0 { parts.push("Win"); }
    let key = if vk >= 0x30 && vk <= 0x39 {
        format!("{}", (b'0' + (vk - 0x30) as u8) as char)
    } else if vk >= 0x41 && vk <= 0x5A {
        format!("{}", (b'A' + (vk - 0x41) as u8) as char)
    } else if vk >= 0x70 && vk <= 0x7B {
        format!("F{}", vk - 0x6F)
    } else if vk == 0x20 { "Space".to_string() } else if vk == 0x0D { "Enter".to_string() }
    else { format!("VK{:X}", vk) };
    parts.push(key.as_str());
    parts.join("+")
}

// 两行快捷键各自的控件/存储/注册表键/默认值/标签（target: 0=识别(跟随当前页), 1=显示/隐藏）
fn hotkey_row(target: usize) -> (usize, usize, &'static Mutex<Option<(u32, u32, String)>>,
    &'static str, u32, u32, &'static str, &'static str) {
    match target {
        0 => (SET_COMBO_REC, SET_RECORD_REC, &CUSTOM_REC, HOTKEY_REC_VALUE, 0, 0x70,
              "识别快捷键:", "请按“识别”的快捷键（Esc 取消）…"),
        _ => (SET_COMBO_SHOW, SET_RECORD_SHOW, &CUSTOM_SHOW, HOTKEY_SHOW_VALUE, 0, 0x79,
              "显示/隐藏:", "请按“显示/隐藏”的快捷键（Esc 取消）…"),
    }
}

// 设置弹窗：识别二维码 / 识别文字 / 两者都识别 / 显示隐藏 四行快捷键，各自预设+自定义录制

fn main() {
    unsafe {
        // 开启 DPI 感知：优先 per-monitor v2（Win10 1703+），失败退回 system-aware，
        // 保证高分屏按原生分辨率渲染，文字清晰不变形
        let user32 = GetModuleHandleW(wide("user32.dll").as_ptr());
        if !user32.is_null() {
            let f = GetProcAddress(user32, b"SetProcessDpiAwarenessContext\0".as_ptr() as *const i8);
            if !f.is_null() {
                let p: unsafe extern "system" fn(isize) -> i32 = std::mem::transmute(f);
                if p(-4 /*DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2*/) == 0 {
                    SetProcessDPIAware();
                }
            } else {
                SetProcessDPIAware();
            }
        } else {
            SetProcessDPIAware();
        }
        let hdc = GetDC(std::ptr::null_mut());
        if !hdc.is_null() {
            let d = GetDeviceCaps(hdc, LOGPIXELSX);
            if d > 0 { DPI_CACHE.store(d, Ordering::Relaxed); }
            ReleaseDC(std::ptr::null_mut(), hdc);
        }
    }

    // 单实例：再次打开时不弹提示，把之前打开的界面调到前台。
    // 用「互斥量 + 共享内存存 PID」而不是 FindWindowW(标题)：标题能对上任何同名窗口
    // （比如记事本里打了一句 "Xin OCR" 保存后标题就是它），会把无关程序拉到前台。
    let m = wide("Global\\XinOCR_Single");
    let sm_name = wide("Global\\XinOCR_Hwnd");
    unsafe {
        let _h = CreateMutexW(std::ptr::null_mut(), 1, m.as_ptr());
        if GetLastError() == 183 {
            // 读上一个实例存的主窗口句柄（进程启动时写入，退出时清零）
            let map = CreateFileMappingW(
                INVALID_HANDLE_VALUE, std::ptr::null_mut(), PAGE_READWRITE,
                0, 8, sm_name.as_ptr());
            let mut target: HWND = std::ptr::null_mut();
            if !map.is_null() {
                let p = MapViewOfFile(map, FILE_MAP_READ, 0, 0, 8);
                if !p.is_null() {
                    let hw = *(p as *const isize);
                    UnmapViewOfFile(p);
                    if hw != 0 && IsWindow(hw as HWND) != 0 {
                        target = hw as HWND;
                    }
                }
                CloseHandle(map);
            }
            // 共享内存拿不到（极早期崩溃残留）就退回按类名找：类名是我们独有的，比标题可靠
            if target.is_null() {
                target = FindWindowW(wide("XinOCR_Main").as_ptr(), std::ptr::null());
            }
            if !target.is_null() {
                // 目标实例可能正最小化/隐藏，先还原再前置
                if IsIconic(target) != 0 {
                    ShowWindow(target, SW_RESTORE);
                }
                ShowWindow(target, SW_SHOW);
                SetForegroundWindow(target);
                SetFocus(target); // 焦点还给窗口本身，让输入框占位提示显示
            }
            return;
        }
    }

    // 监控线程
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    std::thread::spawn(move || monitor_thread(r));

    unsafe {
        let hinstance = GetModuleHandleW(std::ptr::null_mut());
        let cls = wide("XinOCR_Main");

        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            cbClsExtra: 0, cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: LoadIconW(hinstance, 1 as *const u16),
            hCursor: LoadCursorW(std::ptr::null_mut(), IDC_ARROW),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: cls.as_ptr(),
            hIconSm: LoadIconW(hinstance, 1 as *const u16),
        };
        RegisterClassExW(&wc);

        // 固定在屏幕中心；客户区随 DPI 缩放（585×445 @100%），固定窗口（不可拖动拉伸）
        // WS_CAPTION|WS_SYSMENU = 无最大/最小/可调边框，防止布局变形
        let style = 0x00C00000 /*WS_CAPTION*/ | 0x00080000 /*WS_SYSMENU*/ | WS_CLIPCHILDREN
            | 0x00020000 /*WS_MINIMIZEBOX*/ | 0x00010000 /*WS_MAXIMIZEBOX*/;
        // 主窗口居中：用主屏尺寸（不是虚拟桌面，否则多屏时窗口落在两块屏的接缝上）
        let sw = GetSystemMetrics(SM_CXSCREEN);
        let sh = GetSystemMetrics(SM_CYSCREEN);
        let mut rc: RECT = RECT { left: 0, top: 0, right: sc(585), bottom: sc(445) };
        AdjustWindowRect(&mut rc, style, 0);
        let win_w = rc.right - rc.left;
        let win_h = rc.bottom - rc.top;
        let x = (sw - win_w) / 2;
        let y = (sh - win_h) / 2;

        let hwnd = CreateWindowExW(
            0, cls.as_ptr(), wide("Xin OCR").as_ptr(),
            style,
            x, y, win_w, win_h,
            std::ptr::null_mut(), std::ptr::null_mut(), hinstance, std::ptr::null_mut());

        // 启动即显示主界面（不再默认进托盘隐藏）
        ShowWindow(hwnd, SW_SHOW);
        UpdateWindow(hwnd);

        // 记录主窗口句柄并应用全局快捷键
        MAIN_HWND.store(hwnd as isize, Ordering::Relaxed);
        // 把句柄写进共享内存，供下一次启动的单实例分支直接取用（见 main 开头的 CreateFileMappingW）
        {
            let map = CreateFileMappingW(
                INVALID_HANDLE_VALUE, std::ptr::null_mut(), PAGE_READWRITE,
                0, 8, wide("Global\\XinOCR_Hwnd").as_ptr());
            if !map.is_null() {
                let p = MapViewOfFile(map, FILE_MAP_WRITE, 0, 0, 8);
                if !p.is_null() {
                    *(p as *mut isize) = hwnd as isize;
                    UnmapViewOfFile(p);
                }
                // 句柄故意不关：进程活着期间必须一直持有，退出时系统自动回收
            }
        }
        // 启动时注册快捷键：注册失败不弹窗（不打扰启动），但把结论写进设置页提示行，
        // 用户进设置页第一眼就能看到"哪个键没生效、为什么"
        let hk_ok = apply_hotkeys();
        HOTKEY_OK.store(((hk_ok.0 as i32) | ((hk_ok.1 as i32) << 1)) as i32, Ordering::Relaxed);

        // 截图组件（ScreenCapture 静态库）初始化 + 挂回调。
        // 放在这里而不是程序第一行：它内部会建 Composition 设备、加载 D2D/DWrite，
        // 几毫秒到几十毫秒；初始化失败也不该影响二维码/文字识别的其它功能，
        // 所以失败只记下来，等用户真去截图时由 start_region_select 报"截图组件未初始化"。
        sc::sc_set_callbacks(Some(sc_on_done), Some(sc_on_cancel), std::ptr::null_mut());
        if sc::sc_init() != sc::SC_OK {
            *RECOG_RESULT.lock().unwrap() = "截图组件未初始化".to_string();
        }

        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // 收尾：先解绑回调再释放，防止 dispose 过程中的回调落到已经释放的 Rust 状态上；
        // OCR 引擎若被设成常驻，这里也要正常发 EOF 让它自己退出，不要留孤儿进程。
        sc::sc_set_callbacks(None, None, std::ptr::null_mut());
        sc::sc_dispose();
        ocr_engine_drop();
    }
    running.store(false, Ordering::Relaxed);
}
