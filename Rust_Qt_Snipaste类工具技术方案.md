# Rust + Qt 实现 Snipaste 类桌面工具技术方案

> 文档类型：技术架构方案  
> 文档版本：V1.0  
> 整理日期：2026-10-04  
> 目标平台：Windows、macOS、Linux  
> 推荐技术组合：Rust + Qt 6 + QML + CXX-Qt

---

## 1. 方案结论

Rust + Qt 是实现 Snipaste 类截图与贴图工具的一个务实方案：Qt 负责成熟的窗口、绘制、QML/UI 和跨平台能力，Rust 负责截图业务、图像算法、状态管理、并发任务和存储。

对于全新项目，建议采用以下总体架构：

```text
Qt 6 + QML / Qt Quick
        │
        │ QObject / Signals / Slots
        ▼
      CXX-Qt
        │
        ▼
Rust Application Core
├── 截图会话
├── 图片处理
├── 标注对象
├── 撤销与重做
├── 历史记录
├── OCR 调度
└── 配置与文件管理
        │
        ▼
Platform Adapters
├── Windows API
├── macOS API
└── Linux X11 / Wayland
```

推荐组合：

- **UI 与窗口系统**：Qt 6、QML、Qt Quick。
- **Rust 与 Qt 桥接**：CXX-Qt。
- **业务与算法**：Rust。
- **少量原生系统接口**：Rust 平台 crate，必要时加入少量 C++ 或 Objective-C++。
- **构建入口**：初期以 Cargo 为主；大型项目可采用 CMake 驱动 Qt，再调用 Cargo。

CXX-Qt 支持 Rust 和 C++ 双向绑定，可以在 Rust 中实现 `QObject` 子类，并从 C++、QML 和 JavaScript 中使用。它既可以集成进 CMake/C++ 应用，也能通过 Cargo 构建 Rust 应用。

---

## 2. 为什么 Rust + Qt 适合这个项目

### 2.1 保留 Qt 的成熟桌面能力

Snipaste 类工具最困难的部分包括：

- 多显示器截图遮罩。
- 无边框透明窗口。
- 窗口置顶。
- 鼠标穿透。
- DPI 缩放。
- QPainter 图形绘制。
- 图片与剪贴板格式。
- 全局快捷键。
- 国际化和主题。
- Windows、macOS、Linux 适配。

这些能力是 Qt 的强项。与纯 Rust GUI 相比，Rust + Qt 可以减少自建窗口系统、文本渲染、控件和辅助功能的成本。

### 2.2 Rust 负责复杂状态和算法

以下模块适合放到 Rust：

- 截图任务状态机。
- 图片像素算法。
- 标注数据模型。
- 撤销与重做。
- 后台保存。
- OCR 调度。
- 历史记录。
- 图片分组。
- 配置解析。
- 命令行接口。
- 并发任务。
- 错误处理。

这样可以把 Qt 作为表现层和平台能力层，而不是让业务逻辑全部堆积在 QML 或 C++ 中。

### 2.3 不需要把整个 Qt API 暴露给 Rust

CXX-Qt 的思路不是机械地把所有 Qt API 映射成 Rust API，而是让 Rust 和 Qt 各自保留惯用写法，通过生成代码连接两端。这比全面包装 Qt 类型更适合复杂桌面应用。

---

## 3. UI 技术选择

### 3.1 推荐使用 Qt Quick/QML

建议把以下界面使用 QML 实现：

- 设置页面。
- 截图工具栏。
- 标注工具栏。
- OCR 结果窗口。
- 历史记录页。
- 图片分组页。
- 调色板。
- 保存成功提示。
- 快捷键设置页。

主要优势：

- 声明式 UI 开发效率高。
- 深色和浅色主题容易维护。
- 动画和状态切换方便。
- 高 DPI 支持较自然。
- Rust 对象可以作为 QML 的 ViewModel。
- 产品界面更容易实现现代化视觉效果。

### 3.2 Qt Widgets 的适用区域

可以保留少量 Widgets 或原生窗口代码用于：

- 系统托盘。
- 原生文件对话框。
- 特殊截图窗口。
- 与旧 Qt 组件集成。
- 某些平台专属扩展。

CXX-Qt 对 QML 的集成是一等能力，对 QWidgets 的支持则更适合有限场景。因此，如果是从零开始，不建议将整个新项目建立在 Widgets 上。

---

## 4. 分层架构与职责划分

### 4.1 Qt/QML 层

Qt 层建议负责：

```text
界面展示
├── 页面与工具栏
├── 图标、字体、主题
├── 动画与交互反馈
└── 国际化

窗口行为
├── 无边框窗口
├── 透明窗口
├── 置顶状态
├── 窗口位置与尺寸
└── 屏幕与 DPI 信息

绘制
├── 截图预览
├── 选区遮罩
├── 选区控制点
├── 标注实时预览
└── 贴图显示

系统能力
├── 剪贴板
├── 托盘菜单
├── 文件对话框
└── Qt 事件循环
```

### 4.2 Rust 层

Rust 层建议负责：

```text
领域模型
├── CaptureSession
├── Screenshot
├── AnnotationDocument
├── PinnedImage
├── ImageGroup
└── HistoryEntry

业务逻辑
├── 截图流程状态机
├── 工具切换
├── 标注命令
├── 撤销与重做
├── 自动保存
└── 历史记录策略

图像算法
├── 裁剪
├── 缩放
├── 灰度
├── 反色
├── 马赛克
├── 模糊
└── 图片编码

后台任务
├── OCR
├── 文件写入
├── GIF 解码
├── 缩略图生成
└── 历史清理
```

### 4.3 平台适配层

以下功能不要强行只依赖 Qt，应建立平台接口：

- 高性能屏幕捕获。
- HDR 截图处理。
- 活动窗口识别。
- 界面元素层级检测。
- 全局快捷键。
- 原生鼠标穿透。
- 虚拟桌面。
- Windows 原生分享。
- macOS 权限管理。
- Wayland Portal 截图。

---

## 5. 推荐工程结构

```text
snip-tool/
├── Cargo.toml
├── CMakeLists.txt
├── crates/
│   ├── app-core/
│   │   ├── src/
│   │   │   ├── command.rs
│   │   │   ├── event.rs
│   │   │   └── state.rs
│   │   └── Cargo.toml
│   │
│   ├── capture-core/
│   │   ├── src/
│   │   │   ├── display.rs
│   │   │   ├── frame.rs
│   │   │   ├── geometry.rs
│   │   │   └── session.rs
│   │   └── Cargo.toml
│   │
│   ├── annotation-core/
│   │   ├── src/
│   │   │   ├── document.rs
│   │   │   ├── element.rs
│   │   │   ├── command.rs
│   │   │   └── hit_test.rs
│   │   └── Cargo.toml
│   │
│   ├── image-core/
│   ├── history-core/
│   ├── platform-windows/
│   ├── platform-macos/
│   ├── platform-linux/
│   │
│   └── qt-bridge/
│       ├── src/
│       │   ├── capture_controller.rs
│       │   ├── annotation_controller.rs
│       │   ├── history_model.rs
│       │   └── settings_controller.rs
│       ├── build.rs
│       └── Cargo.toml
│
├── qt/
│   ├── qml/
│   │   ├── Main.qml
│   │   ├── CaptureOverlay.qml
│   │   ├── AnnotationToolbar.qml
│   │   ├── PinWindow.qml
│   │   ├── HistoryPage.qml
│   │   └── SettingsPage.qml
│   ├── resources/
│   └── cpp/
│       ├── native_window_helper.cpp
│       └── native_window_helper.h
│
└── tests/
```

关键原则：**QML 不直接调用截图引擎和存储模块，而是通过 Rust 暴露的 Controller 和 Model 访问。**

---

## 6. CXX-Qt 桥接方案

通过 CXX-Qt，可以在 Rust 中声明一个 `QObject`，向 QML 暴露：

- Property。
- Invokable 方法。
- Signals。
- Slots。
- Qt Model。
- 线程安全的消息回调。

CXX-Qt 使用宏和代码生成生成 QObject 的 C++ 表示，以及 Rust 与 C++ 之间的 CXX bridge。

### 6.1 简化概念示例

```rust
#[cxx_qt::bridge]
mod ffi {
    unsafe extern "C++" {
        include!("cxx-qt-lib/qstring.h");
        type QString = cxx_qt_lib::QString;
    }

    #[cxx_qt::qobject]
    #[derive(Default)]
    pub struct CaptureController {
        active: bool,
        width: i32,
        height: i32,
    }

    impl qobject::CaptureController {
        #[qinvokable]
        pub fn start_capture(self: Pin<&mut Self>) {
            // 启动 Rust 截图状态机
        }

        #[qinvokable]
        pub fn cancel_capture(self: Pin<&mut Self>) {
            // 取消截图
        }

        #[qsignal]
        pub unsafe fn capture_completed(
            self: Pin<&mut Self>,
            file_path: QString,
        );
    }
}
```

QML 侧只关心公开接口：

```qml
Button {
    text: qsTr("开始截图")
    onClicked: captureController.startCapture()
}

Connections {
    target: captureController

    function onCaptureCompleted(filePath) {
        console.log("截图完成:", filePath)
    }
}
```

实际使用时需要根据所选 CXX-Qt 版本调整属性与信号声明语法，避免直接复制旧版本示例。

### 6.2 开发环境依赖

建议在开发机和 CI 中安装并固定：

- Rust 工具链。
- C/C++ 编译器。
- Qt 5 或 Qt 6，项目建议使用 Qt 6。
- CMake 3.24 或更高版本。
- CXX-Qt。
- 可被构建系统定位的 `qmake`。

如果不希望将 `qmake` 加入系统 PATH，可以通过 `QMAKE` 环境变量向 Cargo 指定路径。

---

## 7. 图片数据跨边界设计

这是项目中最重要的设计点之一。

### 7.1 不推荐做法

不要频繁将完整像素数据在这些对象间来回复制：

```text
Rust Vec<u8>
→ C++ QByteArray
→ QImage
→ QML Image
→ GPU Texture
```

一张 4K RGBA 截图约占：

```text
3840 × 2160 × 4 ≈ 31.6 MiB
```

如果一次操作发生三到四次完整复制，很容易造成卡顿和内存峰值。

### 7.2 方案一：Qt 持有显示图片

适用于第一版，简单稳定：

1. Qt 获取屏幕图像并保存为 `QImage`。
2. Rust 接收图片元数据，或只在需要算法处理时接收像素。
3. Rust 完成处理后返回新的缓冲区。
4. Qt 将结果更新为新的 `QImage`。

优点是生命周期容易管理，缺点是存在复制。

### 7.3 方案二：Rust 持有图片，Qt 使用图像提供器

适用于历史记录和缩略图：

- Rust 存储图像或缓存文件。
- QML 通过自定义 `QQuickImageProvider` 按 ID 请求图片。
- Qt 层按需解码和生成纹理。
- UI 中不传输 Base64。

### 7.4 方案三：共享不可变缓冲区

适用于性能优化阶段：

- Rust 使用 `Arc<FrameBuffer>` 管理像素。
- C++ 侧使用受控包装对象引用同一缓冲区。
- `QImage` 使用外部数据构造。
- 清理回调确保缓冲区在 `QImage` 销毁前一直有效。
- 写操作采用 Copy-on-Write 或新建缓冲区。

这条路线性能好，但桥接和生命周期更复杂，属于需要严格审查的 `unsafe` 边界。

### 7.5 推荐演进顺序

1. MVP 阶段采用 Qt 持有 `QImage`，允许少量受控复制。
2. 历史记录和缩略图采用图片 ID 与 Image Provider。
3. 性能测试发现明确瓶颈后，再引入共享缓冲区。
4. 不要在项目初期直接设计复杂零拷贝方案。

---

## 8. 标注系统设计

不要把每次鼠标移动都作为跨 Rust/Qt 边界的方法调用。

推荐模式：

```text
QML / QQuickItem
├── 接收鼠标、触控笔事件
├── 进行临时路径预览
└── 操作完成后提交 Command
              ↓
Rust Annotation Core
├── 创建标注对象
├── 校验参数
├── 写入 Document
├── 更新 Undo Stack
└── 返回变更事件
              ↓
Qt 重新绘制受影响区域
```

### 8.1 标注对象模型

Rust 端可定义：

```rust
pub enum Annotation {
    Rectangle(RectangleStyle),
    Ellipse(EllipseStyle),
    Arrow(ArrowStyle),
    Freehand(FreehandStroke),
    Highlight(HighlightStroke),
    Mosaic(MosaicRegion),
    Blur(BlurRegion),
    Text(TextAnnotation),
    Counter(CounterAnnotation),
    Magnifier(MagnifierAnnotation),
}
```

每个对象记录：

- 唯一 ID。
- 几何信息。
- 样式。
- Z 顺序。
- 是否可见。
- 是否锁定。
- 变换矩阵。

### 8.2 撤销和重做

采用 Command Pattern：

```rust
pub trait Command {
    fn apply(&mut self, document: &mut Document);
    fn undo(&mut self, document: &mut Document);
}
```

命令包括：

- `AddAnnotation`。
- `RemoveAnnotation`。
- `MoveAnnotation`。
- `ResizeAnnotation`。
- `RotateAnnotation`。
- `ChangeStyle`。
- `CropImage`。
- `ReorderLayer`。

这种设计比保存整张截图副本更节省内存。

### 8.3 高频交互原则

- 鼠标移动、控制点拖动和临时笔迹预览尽量停留在 Qt 绘制层。
- 用户完成一次操作后，再向 Rust 提交完整命令。
- Rust 返回领域状态变化，而不是要求 QML 逐像素重建界面。
- 多个连续样式更改可以合并为一次撤销命令。

---

## 9. 多线程模型

Qt GUI 对象必须在其所属线程中访问，因此不要让 Rust 后台线程直接更新 QML 对象。

建议流程：

```text
Qt GUI Thread
    │ 发起任务
    ▼
Rust Task Queue
    │
    ├── OCR Worker
    ├── Image Worker
    └── Storage Worker
    │
    ▼
CXX-Qt Thread Queue
    │
    ▼
Qt GUI Thread 更新 QObject
```

CXX-Qt 提供线程辅助能力，用于把后台任务结果安全排队到 QObject 所在线程。

建议遵守：

- GUI 线程只处理交互和绘制。
- OCR、编码和磁盘操作放入后台线程。
- 后台线程返回轻量结果。
- 大图片使用句柄、ID 或共享缓冲区传递。
- 关闭窗口时取消关联任务，避免旧任务更新已销毁对象。
- Rust 后台任务不得直接访问 `QObject` 裸指针。
- 对耗时任务提供取消令牌和任务 ID。

---

## 10. 平台适配设计

### 10.1 跨平台接口

建议定义小而清晰的平台能力接口：

```rust
pub trait CaptureBackend {
    fn displays(&self) -> Result<Vec<DisplayInfo>>;
    fn capture_display(&self, display: DisplayId) -> Result<Frame>;
    fn capture_region(&self, rect: PhysicalRect) -> Result<Frame>;
}

pub trait NativeWindowBackend {
    fn set_topmost(
        &self,
        window: WindowId,
        enabled: bool,
    ) -> Result<()>;

    fn set_click_through(
        &self,
        window: WindowId,
        enabled: bool,
    ) -> Result<()>;

    fn set_visible_on_all_desktops(
        &self,
        window: WindowId,
        enabled: bool,
    ) -> Result<()>;
}
```

### 10.2 Windows 适配

建议优先考虑：

- `windows-rs` 调用 Windows API。
- Windows Graphics Capture 或合适的桌面捕获 API。
- 必要时保留 BitBlt 兼容路径。
- DWM 窗口边界与阴影。
- UI Automation 元素检测。
- 全局快捷键。
- 分层窗口与透明窗口。
- 鼠标穿透。
- 虚拟桌面。
- DPI Awareness。
- HDR 屏幕处理。

### 10.3 macOS 适配

建议考虑：

- ScreenCaptureKit 或 Core Graphics。
- AppKit/Cocoa 窗口能力。
- 屏幕录制权限。
- 辅助功能权限。
- Retina 缩放。
- NSPasteboard。
- Space 与全屏窗口行为。

### 10.4 Linux 适配

建议区分：

- X11。
- Wayland。
- xdg-desktop-portal。
- 不同桌面环境的全局快捷键。
- 窗口置顶与透明窗口差异。
- Wayland 安全模型对截图和全局输入的限制。

不要在第一阶段同时实现三个平台。优先完成 Windows 架构，再抽象跨平台接口。

---

## 11. 阶段性实施方案

### 11.1 第一阶段：Windows MVP

建议先实现：

1. 托盘运行。
2. 全局快捷键。
3. 屏幕枚举。
4. 多显示器截图。
5. 区域选择。
6. 复制和保存。
7. 基础矩形、箭头、文字标注。
8. 贴图置顶。
9. 贴图缩放和透明度。
10. 历史记录。

技术选择：

```text
Qt 6 + QML
CXX-Qt
windows-rs
Rust image processing
SQLite or local structured storage
```

### 11.2 第二阶段：专业编辑能力

- 标注重新编辑。
- 撤销与重做。
- 马赛克和模糊。
- 图片裁剪。
- 多贴图选择。
- 图片分组。
- Solo 模式。
- GIF 支持。
- OCR。
- 二维码识别。

### 11.3 第三阶段：跨平台

在 Windows 架构稳定后，分别实现：

- macOS 截图和权限适配。
- Linux X11 适配。
- Linux Wayland 与 Portal 适配。
- 平台能力矩阵。
- 平台特有降级策略。

不要一开始就为三个平台设计一个覆盖所有能力的“大一统接口”。截图和窗口能力在不同平台差别很大，建议划分为：

- 跨平台核心能力。
- 平台可选能力。
- 平台专属能力。

---

## 12. 构建与发布方案

### 12.1 构建链

开发机和 CI 需要：

- Rust 工具链。
- C/C++ 编译器。
- Qt SDK。
- CMake。
- CXX-Qt 代码生成。
- 平台打包工具。

建议固定以下版本：

- Rust toolchain 版本。
- Qt 小版本。
- CXX-Qt 版本。
- CMake 最低版本。
- Windows MSVC 工具集版本。

### 12.2 Cargo 主导模式

适合：

- Rust 为主要代码。
- QML 和少量 C++ 作为资源与桥接层。
- 团队主要使用 Cargo Workspace。

优点：

- Rust 依赖管理统一。
- Rust 测试与工具链使用方便。
- 初期项目结构简单。

### 12.3 CMake 主导模式

适合：

- 已有大型 Qt/C++ 工程。
- Qt 模块、插件和安装规则较复杂。
- 需要与现有 CMake 基础设施集成。

CXX-Qt 既可通过 Cargo 构建 Rust 应用，也可集成进 CMake/C++ 应用。

### 12.4 发布包内容

应包含：

- 主程序。
- Qt 动态库或符合许可证要求的部署组件。
- Qt 平台插件。
- QML 模块。
- 图片格式插件。
- C/C++ 运行时。
- Rust 编译产生的原生模块。
- OCR 或二维码识别组件。
- 许可证与第三方依赖声明。

---

## 13. 关键风险

### 13.1 FFI 边界过细

不建议：

```text
鼠标移动一次
→ 调一次 Rust
→ Rust 调一次 C++
→ C++ 发一次 Qt Signal
→ QML 重绘
```

建议：

- 高频事件留在 Qt 绘制层。
- 完整操作完成后再提交 Rust 命令。
- 批量传输数据。
- 减少字符串转换。
- 图片以句柄或缓冲区传递。

### 13.2 QObject 生命周期

必须明确：

- QObject 由 Qt 还是 Rust 创建。
- QObject 的父子关系。
- QML 页面销毁后 Rust 是否还持有任务。
- 信号回调时对象是否仍然存活。
- 图片缓冲区由哪一侧释放。

跨边界对象越少，生命周期越容易管理。

### 13.3 图片复制与内存峰值

主要风险：

- 4K 或多屏截图产生大缓冲区。
- QImage、QByteArray 和 Rust `Vec<u8>` 之间重复复制。
- 历史记录生成缩略图时重复解码。
- 多张贴图同时保留原图和处理结果。

建议通过性能测试确定是否需要共享缓冲区，不要在缺少数据时过早优化。

### 13.4 双语言调试

项目同时涉及：

- Rust。
- C++。
- QML/JavaScript。
- 操作系统原生 API。

需要建立：

- 统一日志格式。
- Rust panic 捕获策略。
- C++ 异常边界。
- QML warning 收集。
- 原生崩溃转储。
- 任务 ID 和截图会话 ID。

### 13.5 Qt 许可证

商业发布前需要评估：

- 使用 Qt 开源许可还是商业许可。
- 动态链接和重新链接要求。
- 所用 Qt 模块的具体许可证。
- 安装包是否满足开源许可证义务。
- CXX-Qt 及其他 Rust crate 的许可证。

这部分需要由项目法务依据实际分发方式确认。

---

## 14. 测试策略

### 14.1 Rust 单元测试

优先覆盖：

- 坐标转换。
- 截图状态机。
- 标注对象模型。
- 撤销与重做。
- 图片分组规则。
- 历史清理策略。
- 文件命名规则。
- 图像算法。

### 14.2 Qt/QML 测试

覆盖：

- 工具栏交互。
- 主题切换。
- QML Binding。
- QObject 属性变化。
- Model/View 更新。
- 多窗口创建和销毁。
- DPI 变化。

### 14.3 桥接测试

覆盖：

- Rust 到 Qt 的属性和信号。
- Qt 到 Rust 的方法调用。
- 字符串与日期等类型转换。
- 后台线程结果投递。
- QObject 销毁后的任务取消。
- 大图片与大量缩略图场景。

### 14.4 平台测试

Windows 首阶段至少测试：

- 单显示器。
- 双显示器。
- 不同缩放比例。
- 横竖屏混合。
- HDR 与 SDR。
- 远程桌面。
- 全屏应用。
- 多虚拟桌面。
- 快捷键冲突。
- 剪贴板被占用。

---

## 15. 技术决策摘要

### 15.1 推荐方案

```text
Qt 6 / QML
负责界面、窗口、绘制、主题和跨平台表现

CXX-Qt
负责 QObject、信号槽、属性和线程结果桥接

Rust
负责截图业务、标注模型、图像算法、历史、OCR 和存储

平台适配 crate
负责 Windows、macOS、Linux 原生截图与特殊窗口能力
```

### 15.2 三项关键原则

1. **Qt 管 UI，Rust 管状态和业务。**
2. **高频鼠标和绘制事件不要反复穿越 FFI。**
3. **大图片不要通过 Base64、JSON 或频繁深拷贝传递。**

### 15.3 最终判断

Rust + Qt 是比纯 Rust GUI 更稳健、又比纯 C++ 更安全的折中方案。

对于 Snipaste 类工具，该方案可以获得 Qt 的成熟桌面能力，同时利用 Rust 降低核心逻辑中的内存安全和并发风险。建议先完成 Windows MVP，在架构和性能稳定后，再扩展 macOS 与 Linux。

---

## 16. 参考资料

1. CXX-Qt GitHub：<https://github.com/KDAB/cxx-qt>
2. CXX-Qt 官方文档：<https://kdab.github.io/cxx-qt/book/>
3. CXX-Qt Getting Started：<https://kdab.github.io/cxx-qt/book/getting-started/index.html>
4. CXX-Qt Rust API：<https://docs.rs/cxx-qt/latest/cxx_qt/>
5. Qt QML 文档：<https://doc.qt.io/qt-6/qtqml-index.html>

> 注：示例代码用于说明架构和桥接方式。CXX-Qt 的宏、属性和构建配置可能随版本变化，正式开发时应以项目锁定版本的官方文档为准。
