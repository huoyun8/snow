#pragma once
// ============================================================================
// ScreenCapture 的 C ABI 导出层
//
// 目的：让 Xin OCR（Rust + winapi）进程内直接调用 ScreenCapture 的截图/标注，
// 而不是各自起一个进程。截图这块的窗口、蒙版、工具条、标注全部留在 C++ 侧
// （Ling + Direct2D + Yoga），Rust 侧只负责“发起一次截图、拿结果”。
//
// 线程模型：
//   - 所有函数都必须在宿主的 UI 线程（创建窗口、跑 GetMessage 的那个线程）调用；
//   - 回调也在该线程上同步触发，回调里可以直接更新宿主界面；
//   - 回调里拿到的指针只在回调期间有效，要留存请自行拷贝（sc_free_buffer 除外）。
//
// 生命周期：
//   sc_init()       → 建 COM 套间、初始化 Ling / Direct2D（只做一次）
//   sc_start_capture() … 回调
//   sc_dispose()    → 进程退出前调用，放掉 D2D 与所有窗口
//
// 所有函数都必须由同一进程调用；不做跨进程 IPC。
// ============================================================================
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// 导出修饰：默认为空（静态库直接按普通外部符号链接，Rust 侧用 extern "C" 声明即可）。
// 只有将来要出 DLL 时才定义 SC_API_DLL，届时导出/导入才需要 __declspec。
#if defined(SC_API_DLL)
#  if defined(SC_API_BUILD)
#    define SC_API __declspec(dllexport)
#  else
#    define SC_API __declspec(dllimport)
#  endif
#else
#  define SC_API
#endif

// ---- 结果码 ----
typedef enum ScResult {
    SC_OK                = 0,
    SC_ERR_NOT_INIT      = -1,  // 还没 sc_init
    SC_ERR_BUSY          = -2,  // 上一次截图还没结束
    SC_ERR_NO_SELECTION  = -3,  // 用户没拖出有效选区
    SC_ERR_ENCODE        = -4,  // PNG 编码失败
    SC_ERR_NO_PIXELS     = -5,  // 取像素失败（抓屏时显示器正在切分辨率等）
    SC_ERR_INTERNAL      = -6,
} ScResult;

// ---- 截图模式 ----
typedef enum ScMode {
    SC_MODE_OCR   = 0,  // 框选 → 立刻出图（读文字用）
    SC_MODE_QR    = 1,  // 框选 → 立刻出图（读二维码用）
    SC_MODE_ANNOT = 2,  // 框选 → 进标注界面，用户点“完成”后才出图
} ScMode;

// 完成回调。宿主在里面读 pixels / png，返回后这两个指针立刻失效。
//   mode      发起时的模式
//   w,h       裁剪后图像尺寸（物理像素）
//   pixels    BGRA、top-down、行紧凑（stride = w*4）—— 与 OCR 引擎需要的格式一致
//   png       PNG 编码后的字节流；annot 模式下也一并给，省得宿主再编一次
//   pngLen    PNG 字节数
typedef void (*ScOnDone)(int result, int mode, int w, int h,
                         const uint8_t* pixels, const uint8_t* png, int pngLen,
                         void* user);

// 用户中途取消（Esc / 右键退出 / 关闭窗口）
typedef void (*ScOnCancel)(int mode, void* user);

// 初始化。必须在宿主线程（会创建窗口的那个线程）调用，内部会 CoInitializeEx + Ling::init。
// 返回 SC_OK 表示成功；重复调用是幂等的。
SC_API int  sc_init(void);

// 注册回调。user 原样透传给两个回调。
SC_API void sc_set_callbacks(ScOnDone onDone, ScOnCancel onCancel, void* user);

// 发起一次截图。返回 SC_OK 表示已进入框选界面。
// 若已有一次截图进行中，返回 SC_ERR_BUSY（不会叠出第二个遮罩窗口）。
SC_API int  sc_start_capture(int mode);

// 取消当前截图（等同用户按 Esc）。没有进行中的截图时返回 SC_ERR_NO_SELECTION。
SC_API int  sc_cancel(void);

// 当前是否有截图在进行
SC_API int  sc_is_capturing(void);

// 释放 sc_start_capture 通过 pixels 参数给出来的内存（当前实现回调期间有效，
// 预留给将来可能需要宿主自行释放的场景；现在调用它是安全的空操作）。
SC_API void sc_free_buffer(void* p);

// 进程退出前调用：销毁所有窗口、释放 Direct2D 设备。之后若要再用需重新 sc_init。
SC_API void sc_dispose(void);

#ifdef __cplusplus
}
#endif
