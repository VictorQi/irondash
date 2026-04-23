# Android Smoke Host

这个 example 现在是 fluttersharedtexture 仓库内的最小 Android smoke host，落点在 irondash 现有宿主上，没有新建额外 Flutter/Android 项目。

Android 路径会走：

- Flutter host
- example Rust glue
- ffi-api（当前 smoke host 显式启用 `AndroidTextureRegistrationStrategy::HardwareBufferSeam`）
- engine-texture-registry（`AndroidHardwareBufferSeam` request）
- irondash `Texture::new_with_hardware_buffer_source()`
- `mark_frame_available()` / `DeferredPayloadFlush` 对应的 hardware-buffer delivery 路径

其中共享源会在 Rust glue 里创建一个最小 RGBA8888 AHardwareBuffer，并挂上 AndroidPlatformCleaner，供 SharedSource 持有和释放。

注意：`platform-android::AndroidFrameBridge` 仍然保留为默认 CPU-copy fallback/baseline，但当前 smoke host 这轮验证优先走的是显式 AndroidHardwareBuffer 注册路径，不再是旧的默认 bridge 路径。

补充：仓库代码现在已经新增独立的 `HardwareBufferImport` backend contract（`ImportedHardwareBufferTexture` / `AHardwareBufferFrameProvider`），但这个 example 仍继续固定使用 `HardwareBufferSeam`。原因是 irondash fork 里还没有真正的 consumer-import substrate；如果强行请求 import path，当前会收到精确的 bridge-unavailable 原因，而不是偷偷回退成 seam。

## 目录

- Flutter host: `code-base/irondash/texture/example`
- Rust smoke glue: `code-base/irondash/texture/example/rust`

## 前置条件

- Flutter SDK
- Android Studio 或已安装 Android SDK/NDK
- Android API 26 及以上模拟器或真机

这个 example 的 Android host 已把 `minSdkVersion` 提升到 26，因为 AHardwareBuffer smoke path 不支持更低 API。

## 运行

在这个目录执行：

```sh
flutter pub get
flutter run -d <android-device-id>
```

在当前这台机器上，已经验证过的推荐启动方式是：

```sh
source "$HOME/.zprofile"
source "$HOME/.zshrc"
proxy
export JAVA_HOME=$(/usr/libexec/java_home -v 17)
/Users/victor/fvm/versions/stable/bin/flutter run -d <android-device-id> --use-application-binary build/app/outputs/flutter-apk/app-debug.apk
```

原因：

- 这样可以直接复用已验证 APK，避免每次继续工作都重新卡在 Gradle/cargokit 构建链
- 本次会话结束前已经关闭 adb server，因此下次需要先执行 `adb start-server && adb devices`

如果改动触及 Rust host / JNI 打包链，先重建 APK：

```sh
source "$HOME/.zprofile"
source "$HOME/.zshrc"
proxy
export JAVA_HOME=$(/usr/libexec/java_home -v 17)
/Users/victor/fvm/versions/stable/bin/flutter clean
/Users/victor/fvm/versions/stable/bin/flutter build apk --debug --android-skip-build-dependency-validation
```

可选设备：

- Android Studio 模拟器
- adb 连接的真机

两者都可以，只要是 Android 26+ 且 Flutter 能正常运行。

最近一次已验证环境：

- 设备：Xiaomi 15 Ultra
- 日期：2026-04-23
- 结果：在 `engine-texture-registry` / `ffi-api` 完成 seam-vs-import 公共类型对齐后，重新 `flutter clean`、重建 debug APK，并通过 `flutter run --use-application-binary` 完成真机 smoke；冷启动自动 acquire 成功显示纹理并拿到 `request_id=0` / `texture_id=0`；`Release Texture` 后 UI 进入“当前没有活动纹理”并对应 `ResourceReleased -> Unloaded`；再次 `Acquire Smoke Texture` 时，smoke host 会先消费保留下来的待清理 session，执行 best-effort `unregister_engine -> EngineGone`，随后重新 `register_engine` 并拿到新的 `request_id=1` / `texture_id=1`
- 增强日志确认：当前 smoke host 创建了真实 `AHardwareBuffer`（`256x256`），冷启动和 reacquire 都直接走到 `TextureReady -> Ready`，而没有再出现 `FFI Android delivery attempting bridge` / `AndroidFrameBridge copy start` 这一类默认 CPU-copy bridge 日志
- UI 回归也确认：reacquire 后页面重新显示“纹理已就绪”，并更新到 `Texture id: 1`
- 语义澄清：当前这条链已经收口了“显式 AndroidHardwareBuffer 注册主路径”，但如果把“真实 zero-copy”定义成“最终不发生 `AHardwareBuffer -> ANativeWindow` 的复制”，这仍不是终态

下期继续时建议先做：

1. `adb start-server && adb devices`
2. 用 `--use-application-binary` 启动已验证 APK 重放基线
3. 只有在 host / Rust / JNI 链改动后才重新 `flutter clean && flutter build apk`

## 手动验收

满足下面几条即可视为 smoke 通过：

1. 应用启动后，页面状态变为“纹理已就绪”。
2. 预览区域出现明显的彩色条纹/棋盘图案，而不是空白或纯黑。
3. 点击 `Release Texture` 后，预览区域变为“当前没有活动纹理”。
4. 再点击 `Acquire Smoke Texture` 后，重新出现纹理。

如果需要额外看 Rust 侧行为，可以打开 logcat，关注 `flutter` tag 下的 smoke 日志。

## 当前限制

- 这是最小 smoke host，不包含 Dart plugin 包装，也不提供通用业务 API。
- 当前 smoke host 验证的是 `ffi-api -> engine-texture-registry -> irondash hardware-buffer path` 的单纹理冒烟，不是完整产品化多 source 场景。
- `Release Texture` 现在只走真实 Rust `release_texture` 路径，用于单独验证 release 语义；smoke host 会保留一份待清理 session，用于下一次 acquire 前执行 best-effort `unregister_engine`。
- 因此，release 后如果日志里没有紧跟 `EngineGone`，这是新的宿主预期；只有显式 reacquire 触发 best-effort teardown 时，才应该看到由 `unregister_engine` 带来的 engine-level 事件。
- 随后的 `Acquire Smoke Texture` 会先消费这份待清理 session，再重新执行 `register_engine` 发起 acquire；这样 release 语义和 engine teardown 语义在 smoke host 里是分开的，但重新获取纹理仍然可用。
- 需要特别注意：irondash 当前的 Android hardware-buffer 实现仍会在 `mark_frame_available()` 期间把 retained `AHardwareBuffer` flush 到 engine-local `ANativeWindow`。因此“显式 AndroidHardwareBuffer 主路径已接通”不等于“GPU-direct / 无 copy 的最终 zero-copy 已完成”。
