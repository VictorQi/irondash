# Android Smoke Host

这个 example 现在是 fluttersharedtexture 仓库内的最小 Android smoke host，落点在 irondash 现有宿主上，没有新建额外 Flutter/Android 项目。

Android 路径会走：

- Flutter host
- example Rust glue
- ffi-api
- engine-texture-registry
- Android native-window texture
- platform-android bridge

其中共享源会在 Rust glue 里创建一个最小 RGBA8888 AHardwareBuffer，并挂上 AndroidPlatformCleaner，供 SharedSource 持有和释放。

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
- 日期：2026-04-20
- 结果：冷启动自动 acquire 成功显示纹理；`Release Texture` 后预览区清空；再次 `Acquire Smoke Texture` 后生成新的 `request_id=1` / `texture_id=1`，纹理重新出现

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
- Android 主路径当前验证的是 `ffi-api -> registry -> platform-android bridge` 的单纹理冒烟，不是完整产品化多 source 场景。
- `Release Texture` 触发的是真实 Rust release path；原生清理由 SharedSource 的 deferred cleanup 异步投递到 Platform Thread 执行。
