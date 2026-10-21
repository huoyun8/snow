#pragma once
#include <include/Ling.h>
#include <cstdint>
#include "WinHost.h"

class CutMask;

// 嵌入宿主模式：由 sc_api（Xin OCR 进程内调用）设置。
// 非 None 时——框选完成不自动出图（留给标注界面）、不弹保存对话框、不钉图、
// 不查更新、不根据 --auto-quit 退出进程，一切收尾都交给宿主的回调。
enum class CapHostMode { None, Ocr, Qr, Annot };

// 宿主的两个回调，与 sc_api.h 里的 ScOnDone / ScOnCancel 一一对应。
// 放在这里而不是直接 include sc_api.h，是为了让 WinCap 不反过来依赖 C ABI 那一层
struct ScHostCallbacks {
    // result        ScResult
    // pixels/png    回调期间有效，返回后释放
    void (*onDone)(int result, int mode, int w, int h,
                   const uint8_t* pixels, const uint8_t* png, int pngLen, void* user);
    void (*onCancel)(int mode, void* user);
    void* user;
};
ScHostCallbacks sc_host_callbacks();

// 截图主窗口。铺满整个虚拟桌面，拖框结束后并不马上让位，而是留下来当宿主：
// 选区可以继续调整（四边 + 四角拖改大小），框选完成后直接在框选窗口内标注。
// 标注宿主公共状态见 WinHost。
class WinCap:public WinHost
{
public:
	~WinCap();
	static void init();
	static WinCap* get();
	// 退出流程里调：窗口对象是文件级静态变量，交给静态析构就在 CoUninitialize 之后了
	static void dispose();
	// 嵌入宿主模式的开关与查询。setHostMode 必须在 init() 之前调
	static void setHostMode(CapHostMode m, void* user);
	static CapHostMode hostMode();
	static void* hostUser();
	// —— 嵌入宿主模式的收尾（实现见 WinCap.cpp 顶部）——
	void sc_host_deliver_selection(); // 框完立刻出图（Ocr / Qr 模式）
	void sc_host_finish_annot();      // 标注完成出图（Annot 模式）
	void sc_host_cancel();            // 用户取消
	void sc_host_fire(int result, int w, int h, const std::vector<BYTE>& pixels);
	Microsoft::WRL::ComPtr<ID2D1Bitmap1> getCutImg();
	// 工具条统一定位规则：右边与选区右边对齐，下方空间够就摆在选区右下方，
	// 不够就摆右上方，上下都不够就盖在选区右下角内部（留一点边距）
	void layoutTool(Ling::WinBase* tool);
	// 下面都是给工具条用的门面 ————————————————————
	void startPin();
	void saveToFile() override;
	void copyToClipboard() override;
	// —— 标注宿主接口（WinHost 纯虚）——
	void layoutTools() override;
	void onToolStyleChanged() override;
	Ling::TextBox* getTextBox() override;
	void setEditingText(ShapeText* shape) override;
	POINT toImgPos(const POINT& pos) const override;
	void pinToScreen() override;
public:
	// 标注窗口用的就是这一份选区，它自己不再框选
	std::unique_ptr<CutMask> cutMask;
private:
	WinCap();
	void onCreated() override;
	void layout() override;
	BOOL setCursor() override;
	LRESULT onHitTest(const POINT pos) override;
	void setPixPos(POINT pos);
	void getPixImg(POINT pos);
	void paintPix(ID2D1DeviceContext* ctx);
	void onKey(UINT key);
	void onDown(POINT pos, bool isRight);
	void onMove(POINT pos);
	void onUp(POINT pos, bool isRight);
	void onWheel(POINT pos, float space);
	void onTimerCB(UINT id);
	void onClosed();
	// WinCap 满屏、工具条叠在选区内：WinCap 一旦获焦（被点击）会盖住工具条，
	// 获焦后把它们重新置顶，让编辑器始终可见
	void raiseTools();
	// 命令行 --enter=pin 时框选完成直接进入标注界面（等于替用户按住 Ctrl）
	bool enterByArg();
	// 右键门面：把窗口整体退回"按快捷键首次进入"的屏幕选区界面 ——
	// 清掉选区 / 标注 / 工具条，放大镜摆回光标处，十字光标等待重新拖框。
	// 与首次进入的唯一区别是不开智能吸附（用户明确要求右键后是纯手动选框）
	void restartSelect();
	// 首次进入 Adjust 阶段时建出标注用的工具条与历史（懒建，避免截图窗口一开始就带着工具条）
	void ensureAnnotation();
	// 嵌入宿主模式下，这一帧是否已经交出去了（成功出图 / 已判取消）。
	// 防止 onClosed 里再补一次 cancel 回调，让宿主收到"又成功又取消"
	bool hostSettled{ false };
	// 离屏把"选区底图 + 未撤销 shape"合成出最终图像的像素（与 WinPin::getImagePixels 同格式）
	bool getImagePixels(std::vector<BYTE>& pixels, D2D1_SIZE_U& size);
	std::tuple<int, int, int, int> getCMYK(const BYTE& r, const BYTE& g, const BYTE& b);
private:
	// Select 拖框取色 -> Adjust 调整选区并直接进入标注界面
	enum class CapStage { Select, Adjust };
	CapStage stage{ CapStage::Select };
	Microsoft::WRL::ComPtr<ID2D1Bitmap1> pixImg;
	D2D1_RECT_F pixSrcRect{};
	// 铺满窗口的画布，走 swap chain 双缓冲：底图、蒙版、放大镜每帧都重画
	Ling::Canvas* canvas{ nullptr };
	POINT pixPos;
	bool isPress{ false }, isClosed{ false };
	// onDpiChanged 与 onSizeChanged 之间的接力标记，见构造函数里的注释
	bool dpiChanged{ false };
	Microsoft::WRL::ComPtr<ID2D1SolidColorBrush> brushBg;
	Microsoft::WRL::ComPtr<ID2D1SolidColorBrush> brushText;
	Microsoft::WRL::ComPtr<ID2D1SolidColorBrush> crossBrush;
};
