#include "pch.h"
#include "sc_api.h"
#include "App.h"
#include "Util.h"
#include "Win/WinCap.h"

// ============================================================================
// sc_api 实现
//
// 这一层只做三件事：把引用计数 / 生命周期收干净、把 WinCap 的宿主模式切对、
// 把 WinCap 的回调转成 C 函数指针交给外面。所有真正的截图逻辑仍在 WinCap 里，
// 一行没改（只在几个分叉口加了 CapHostMode 判断）。
//
// 线程约束：sc_init / sc_start_capture / sc_cancel 必须都在宿主 UI 线程调用。
// 原因是 Ling 整套都假定单线程消息循环（CoInitializeEx 是 STA、D2D 设备、窗口都绑在这个线程上）。
// ============================================================================

namespace {
    bool g_inited{ false };
    bool g_initOk{ false };
    bool g_capturing{ false };
    ScOnDone g_onDone{ nullptr };
    ScOnCancel g_onCancel{ nullptr };
    void* g_user{ nullptr };

    // WinCap 反向来取回调。声明在 WinCap.h 里，定义放这儿 —— 让 WinCap 不必依赖 sc_api.h
    ScHostCallbacks g_cb{};
}

ScHostCallbacks sc_host_callbacks()
{
    return g_cb;
}

// 转发给宿主的两个中间层。
// 必须放在 extern "C" 里：ScOnDone / ScOnCancel 是 C 链接的函数指针类型，
// 而 C++ 里 "C++ 链接的 void(*)(...)" 与 "C 链接的 void(*)(...)" 是不同类型，
// 直接把普通 static 函数赋给 g_cb.onDone 会被 MSVC 报 C2664（无法转换）。
// 加上 static 保证不导出、不与外部同名符号冲突。
extern "C" {

static void host_on_done(int result, int mode, int w, int h,
                         const uint8_t* pixels, const uint8_t* png,
                         int pngLen, void* user)
{
    g_capturing = false; // 先复位状态，回调里宿主可以立刻再发起一次
    if (g_onDone) g_onDone(result, mode, w, h, pixels, png, pngLen, user ? user : g_user);
}

static void host_on_cancel(int mode, void* user)
{
    g_capturing = false;
    if (g_onCancel) g_onCancel(mode, user ? user : g_user);
}

} // extern "C"

extern "C" {

int sc_init(void)
{
    // 不用 std::call_once：dispose 之后 once_flag 不会复位，程序就再也起不来了。
    // 这里用普通标志，App::init 本身对重复调用是容忍的（COM 已初始化、单例已存在时直接返回）
    if (!g_inited) {
        // 回调每次 init 都重新绑定一遍：WinCap 侧靠 g_cb 取回调，
        // 若宿主的 sc_set_callbacks 发生在 init 之前，这一步会把转发层固定下来
        g_cb.onDone = host_on_done;
        g_cb.onCancel = host_on_cancel;
        g_cb.user = g_user;
        // Ling::init 里做了 CoInitializeEx（STA）+ App 单例
        App::init();
        g_inited = true;
        g_initOk = true;
    }
    return g_initOk ? SC_OK : SC_ERR_NOT_INIT;
}

void sc_set_callbacks(ScOnDone onDone, ScOnCancel onCancel, void* user)
{
    g_onDone = onDone;
    g_onCancel = onCancel;
    g_user = user;
    // 让 WinCap 侧拿到的 user 跟着更新，否则宿主传进来的上下文会丢
    if (g_cb.onDone == host_on_done) g_cb.user = user;
}

int sc_start_capture(int mode)
{
    if (!g_inited) return SC_ERR_NOT_INIT;
    if (mode != SC_MODE_OCR && mode != SC_MODE_QR && mode != SC_MODE_ANNOT) {
        return SC_ERR_INTERNAL;
    }
    if (g_capturing) return SC_ERR_BUSY;

    // 上一帧已经关掉但对象还没析构完（onClosed 把 reset 推到下一轮消息循环）。
    // 这种情况直接报 BUSY 让宿主稍后再试，绝不能再往下走 —— 否则 g_capturing
    // 已经被置 true、窗口却没建起来，下一次调用会被 g_capturing 挡死，截图彻底瘫掉。
    if (WinCap::get()) return SC_ERR_BUSY;

    // 映射到 WinCap 的宿主模式。必须在 init() 之前设，因为 WinCap 构造时就会读它
    CapHostMode hm = CapHostMode::None;
    switch (mode) {
        case SC_MODE_OCR:   hm = CapHostMode::Ocr;   break;
        case SC_MODE_QR:    hm = CapHostMode::Qr;    break;
        case SC_MODE_ANNOT: hm = CapHostMode::Annot; break;
    }
    WinCap::setHostMode(hm, nullptr);
    g_capturing = true;

    WinCap::init();
    if (!WinCap::get()) {
        g_capturing = false;
        return SC_ERR_INTERNAL;
    }
    return SC_OK;
}

int sc_cancel(void)
{
    if (!g_inited) return SC_ERR_NOT_INIT;
    auto cap = WinCap::get();
    if (!cap) return SC_ERR_NO_SELECTION;
    // 走正常的 close 路径：onDestroy → onClosed → 那里发现 hostSettled 为 false
    // 就会回调 sc_host_cancel，状态也由 host_on_cancel 统一复位
    cap->close();
    return SC_OK;
}

int sc_is_capturing(void)
{
    return g_capturing ? 1 : 0;
}

void sc_free_buffer(void*)
{
    // 当前实现给宿主的像素/PNG 指针只在回调期间有效，
    // 回调返回后由 WinCap 里的 vector / 局部变量负责释放，宿主不需要也不能释放。
    // 保留这个函数是为了将来真要改成"宿主持有"时不用改 ABI
}

void sc_dispose(void)
{
    // 先收掉可能还开着的截图窗口，再拆单例。顺序不能反 ——
    // WinCap 里攥着 D2D 位图和画刷，必须赶在 D2D 设备销毁之前放掉
    WinCap::dispose();
    App::dispose();
    Ling::dispose();
    g_inited = false;
    g_capturing = false;
}

} // extern "C"
