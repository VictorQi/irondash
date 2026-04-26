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

最近一次已验证的 P0 多引擎共享源环境：

- 设备：Xiaomi 15 Ultra
- 日期：2026-04-25
- 流程：主页面点击 `Open Multi-Engine Host` 前，launcher 会先释放 single-engine baseline，确保 Panel A 第一次 acquire 重新走 miss 路径；随后按 `Panel A Acquire Shared Source -> Panel B Verify Cache-Hit Acquire -> Panel A Release Texture -> Destroy Panel A -> Recreate Panel A -> Panel A Acquire Shared Source` 的顺序重放
- 结果：Panel A 首次 acquire 的 snapshot 含 `StatusChanged:Loading -> TextureReady -> StatusChanged:Ready`；Panel B cache-hit snapshot 只含 `StatusChanged:Pending -> TextureReady -> StatusChanged:Ready`；释放和销毁 Panel A 后，Panel B 仍保持 `Ready`；重建后的 Panel A 重新 acquire 成功，新的 `engine_handle=1125899906842628` / `request_id=3` 再次回到 `Loading -> Ready`
- 语义澄清：`texture_id` 是 engine-local 值，跨 engine 可能都显示为 `0`；判断“同一 SharedSource 被两个 EngineLocalTexture 复用”时，应结合 `engine_handle`、`source_id`、`source_generation` 和 `request_events`，而不是只比较 numeric `texture_id`

2026-04-26 P2 host / device 验证：

- `cd /Volumes/VictorOutter/docs/DesignDocs/code-base/irondash/texture/example/rust && cargo check --target aarch64-linux-android` 通过
- `/Users/victor/fvm/versions/stable/bin/flutter analyze /Volumes/VictorOutter/docs/DesignDocs/code-base/irondash/texture/example/lib/main.dart` 通过
- fresh `flutter clean && flutter build apk --debug --android-skip-build-dependency-validation` 通过，并重新产出 `build/app/outputs/flutter-apk/app-debug.apk`
- Xiaomi 15 Ultra 上的首次 `Run P2 Stress Suite` 真机回放重新打开了问题：rapid cycle 报出 `fd_delta=400`，但同时 `active_textures_after=0`、`active_borrows_after=0`、`inflight_after=0`，说明更像是最终 FD 采样早于队列里的 platform cleanup callbacks 执行，而不是存活对象泄漏
- `run_rapid_cycle_stress()` 现已在最终 `/proc/self/fd` 采样前显式 pump Android platform cleanup callbacks；修复后重新 clean rebuild、`adb install -r`、冷启动并再次点击 `Run P2 Stress Suite`，Xiaomi 15 Ultra 上的 `P2 Stress Report` 已回到 `p2_stress=PASS`，其中 `rapid_cycle=PASS`、`rss_growth_pct=0.81`、`fd_delta=-1`、`visibility_flicker=PASS`、`concurrent_multi_engine=PASS`，对应 UI 证据已落盘到 `code-base/progress/p2_stress_rerun_ui.xml`

## P2 收尾后的非阻塞跟踪

这些项不会重新打开 P2；当前权威通过证据仍是 `code-base/progress/p2_stress_rerun_ui.xml`。

- [ ] soak：在 Xiaomi 15 Ultra 上补更长时长的 `Run P2 Stress Suite` / reacquire / recreate 回放，确认这次 close-out 不只是单轮短回归。
- [ ] 设备覆盖面：把同一套已验证 APK + P2 回放步骤至少再覆盖一台第二 Android 设备 / SoC。
- [ ] 证据固化：下次回放时把 filtered logcat 或脚本化 capture 与 `code-base/progress/p2_stress_rerun_ui.xml` 配对落盘，避免修复后 rerun 只留下 UI dump。

## 当前 UI 诊断面

当前 Android smoke host 页面除了纹理预览，还会显示一个 `Smoke Snapshot` 面板，用于直接读取当前 panel 的宿主级诊断摘要。

面板当前会展示：

- `engine_handle`
- `registration_strategy` / `delivery_path`
- `source_id` / `request_id` / `source_resolve_count` / `source_generation`
- 当前 request status、texture_id、error_code
- 当前 panel 最近一次 FFI event（`last_event`）
- 当前 request 的事件链摘要（`request_events`）

如果需要在单宿主或 multi-engine panel 上核对“同一 source、不同 engine、本地 texture 各自独立但事件链连续”的证据，优先看这个面板，再结合 logcat。

## P1 诊断按钮

主页面现在还新增了一个 `P1 Diagnostic Report` 面板。它不会覆盖 `Smoke Snapshot`，而是把每次运行的 P1 诊断结果按条目追加到报告里。

当前按钮职责如下：

- `Run P1 Control Plane`：覆盖当前仓库已实现的控制面主矩阵，包含 `pause_request -> resume_request -> Ready`、`release-before-ready` 的晚到 `TextureReady` 丢弃、以及 `duplicate acquire -> Reused`。
- `Run Cancel Mid-Flight`：验证请求仍处于 `Loading` 前沿时执行 `cancel_request` 会停在 `Canceling`，并且后续不再冒出 stray `TextureReady`。
- `Run Cancel-After-Ready`：验证已完成请求上的 `cancel_request` 现在是 silent no-op，不再把 `Ready` 请求改写成 `Canceling`。
- `Run Interleaved Pause/Resume`：验证当前仓库实际采用的 `pause_request -> immediate resume_request` 交错路径在单次 pump 后仍能回到 `Ready`。
- `Run EngineGone Pending`：验证 request 仍处于 `Pending` 时 `unregister_engine` 会清掉跟踪状态，并且后续不再冒出 `TextureReady`。
- `Load Multiple Sources (P1)`：在 host 内部用小 byte budget 重放 `LRU eviction -> reload -> final release`，并报告 `DeferredDrop` target。`AHardwareBuffer_release` 的最终 release 边界则由 focused test `cargo test -p platform-android test_ahardwarebuffer_release_waits_for_final_release_boundary -- --nocapture` 证明。

注意：当前仓库现在已经提供单独的 `irondash_ffi_pause_request` 和 `RequestPaused` 事件；因此 smoke host 的 pause/resume 诊断不再借用 cancel 语义。`Run Cancel Mid-Flight` 仍保留为独立按钮，用来验证 cancel 和 pause 在控制面上的分工没有混淆。

## P2 Live Metrics 与 Stress Suite

主页面现在还额外提供两个 P2 面板：

- `Live Metrics`：位于纹理预览下方，直接消费 `Smoke Snapshot` 字段，不再只靠 logcat 手工读值
- `P2 Stress Report`：位于 `P1 Diagnostic Report` 下方，用于累计显示 `Run P2 Stress Suite` 的结果

`Live Metrics` 当前会同步展示：

- `registration_strategy` / `delivery_path`
- `copy_bytes` / `source_bytes`
- `platform_thread_ms` / `acquire_to_ready_ms` / `release_ms` / `release_to_ready_ms`
- `shared_source_count` / `active_borrow_count` / `cache_bytes_used` / `request_inflight_count`
- `backpressure_rejection_count` / `texture_registrations`
- 扩展文本：`request_state`、`last_event`、`request_events`

`Run P2 Stress Suite` 当前覆盖三条宿主级自诊断：

- rapid acquire/release：同一 engine、同一 source 连续执行 100 次 acquire + release，要求 `VmRSS` 增长 `<= 5%`、fd 增量为 `0`、最终无 active borrow、无 active texture 残留
- visibility flicker：模拟 `Prefetch -> release-before-ready -> Visible -> Invisible -> Visible` 的高频切换，要求没有 orphan `TextureReady`，且最终状态能回到稳定 `Ready` / `Unloaded`
- concurrent multi-engine miss：围绕真实 `FfiApi` / `ResourceManager` / `SourceProvider` 流，使用 synthetic engine handles + mock registry store 验证首次并发 miss 只发生一次 resolve，并生成 3 次独立 registration

注意：最后一项是宿主自诊断，不会在 UI 上额外打开第三个可见 panel；它证明的是“单次 resolve、多 engine 注册”这一条资源流，而不是复刻一个真实三窗口 Activity。

## P0 多引擎手动验收

推荐按下面顺序回放：

1. 从主页面点击 `Open Multi-Engine Host`，让 launcher 先释放 single-engine baseline。
2. 在 Panel A 点击 `Acquire Shared Source`，确认 snapshot 中 `request_events` 包含 `Loading`。
3. 在 Panel B 点击 `Verify Cache-Hit Acquire`，确认 snapshot 中 `request_events` 不含 `Loading`，且 `source_id` / `source_generation` 与 Panel A 一致。
4. 在 Panel A 点击 `Release Texture`，确认 Panel B 的 snapshot 仍保持 `Ready`。
5. 点击顶部 `Destroy Panel A`，确认 Panel A 变成 placeholder，而 Panel B 仍保持 `Ready`。
6. 点击顶部 `Recreate Panel A`，再次点击 `Acquire Shared Source`，确认重建后的 Panel A 能重新拿到新的 `engine_handle` / `request_id`。

注意：跨 engine 的 `texture_id` 数字可以重复；多引擎共享源是否成立，要看“相同 `source_id` + 相同 `source_generation` + 不同 `engine_handle` + Panel B 无 `Loading`”。

后续如需继续追踪非阻塞项，建议先做：

1. `adb start-server && adb devices`
2. 用 `--use-application-binary` 启动已验证 APK 重放基线
3. 只有在 host / Rust / JNI 链改动后才重新 `flutter clean && flutter build apk`

## 最小回归顺序

后续如果改动落在 `ffi-api`、`engine-texture-registry`、Android seam/baseline 状态机，先跑下面这组最小回归，再决定要不要扩大范围：

1. 先跑 `cd /Volumes/VictorOutter/docs/DesignDocs/code-base && cargo test -p ffi-api-boundary -- --nocapture`。
2. 确认这组 focused tests 仍覆盖 `pause_request -> RequestPaused -> resume_request`、`duplicate acquire`、`release -> reacquire`、`cancel mid-flight` 无 stray ready、`cancel-after-ready no-op`、`release-before-ready` 的晚到 ready 丢弃、`engine gone` / pending request 清理、`bridge success/failure`、`fallback 保持 Registered` 语义；如果改动涉及 Android cleaner/shared-source 最终 release，再额外跑 `cargo test -p platform-android test_ahardwarebuffer_release_waits_for_final_release_boundary -- --nocapture`。
3. 然后执行 `adb start-server && adb devices`，确认真机仍在线。
4. 如果 Android target 可用，先用 `flutter run -d <android-device-id> --use-application-binary build/app/outputs/flutter-apk/app-debug.apk` 复用已验证 APK 重放单宿主 smoke，再点击一次 `Run P2 Stress Suite`，确认报告里出现 `rapid_cycle=PASS`、`visibility_flicker=PASS`、`concurrent_multi_engine=PASS`，并重新 `Refresh Snapshot` 或 `Acquire Smoke Texture` 确认 `Live Metrics` 仍会更新。
5. 如果本轮改动触及 Dart snapshot / metrics UI，额外跑 `/Users/victor/fvm/versions/stable/bin/flutter analyze /Volumes/VictorOutter/docs/DesignDocs/code-base/irondash/texture/example/lib/main.dart`。
6. 如果改动触及 Rust host / JNI 打包链，再执行 `flutter clean && flutter build apk --debug --android-skip-build-dependency-validation`。

如果改动没有触及 Rust host / JNI 打包链，这就是默认的最小回归；不要先做 `flutter clean` 或重走全量 APK 构建链。

## 手动验收

满足下面几条即可视为 smoke 通过：

1. 应用启动后，页面状态变为“纹理已就绪”。
2. 预览区域出现明显的彩色条纹/棋盘图案，而不是空白或纯黑。
3. `Smoke Snapshot` 面板能读到当前 panel 的 `engine_handle`、`source_id`、`request_id` 和最近一次 `last_event`。
4. 点击 `Release Texture` 后，预览区域变为“当前没有活动纹理”，同时 snapshot 会反映 release 后的新状态。
5. 再点击 `Acquire Smoke Texture` 后，重新出现纹理，并且 snapshot 会更新到新的 request/texture 信息。

如果要验收 P0 多引擎共享源，再额外满足：

6. Panel A 首次 acquire 的 snapshot 含 `Loading`，Panel B cache-hit acquire 的 snapshot 不含 `Loading`。
7. Panel A `Release Texture` 和 `Destroy Panel A` 都不会让 Panel B 失去 `Ready`。
8. `Recreate Panel A` 之后再次 acquire 能拿到新的 `engine_handle` / `request_id` 并重新回到 `Loading -> Ready`。

如果要验收当前 P1 host 诊断，再额外满足：

9. `Run P1 Control Plane` 报告里出现 `p1_control_plane=PASS`，并分别列出 `pause_resume_path=pause_request -> resume_request -> Ready`、`pause_events=RequestPaused:Canceling`、`release_before_ready=PASS`、`duplicate_acquire=PASS`。
10. `Run Cancel Mid-Flight` 报告里出现 `cancel_midflight=PASS`，且 `followup_events=-`。
11. `Run Cancel-After-Ready` 报告里出现 `cancel_after_ready=PASS`，且 `cancel_events=-`。
12. `Run Interleaved Pause/Resume` 报告里出现 `interleaved_pause_resume=PASS`，并且 `ready_events` 中仍包含 `TextureReady`。
13. `Run EngineGone Pending` 报告里出现 `engine_gone_pending=PASS`，且 `request_state_after_unregister=absent`。
14. `Load Multiple Sources (P1)` 报告里出现 `p1_full_lifecycle=PASS`，并包含 `evicted_source_id`、`reloaded_generation`、`deferred_drop_target`。

如果要验收当前 P2 host 能力，再额外满足：

15. `Live Metrics` 面板可见，并且在 acquire / release / refresh 后会同步更新 `delivery_path`、timing、cache state、`texture_registrations` 与 `last_event`。
16. 点击 `Run P2 Stress Suite` 后，`P2 Stress Report` 中出现 `rapid_cycle=PASS`、`visibility_flicker=PASS`、`concurrent_multi_engine=PASS`。
17. `concurrent_multi_engine` 报告中能读到 `resolve_count_delta=1`、`texture_registrations_delta=3`，同时理解这项验证来自 synthetic engine handles + mock registry store，而不是页面上真的出现第三个可见 panel。

补充说明：`AHardwareBuffer_release` 的准确触发时序当前没有直接显示在 UI 报告里；这部分验收依赖 focused `platform-android` test，而不是 smoke host 文本输出。

如果需要额外看 Rust 侧行为，可以打开 logcat，关注 `flutter` tag 下的 smoke 日志。

## 当前限制

- 这是最小 smoke host，不包含 Dart plugin 包装，也不提供通用业务 API。
- 当前 smoke host 验证的是 `ffi-api -> engine-texture-registry -> irondash hardware-buffer path` 的单纹理冒烟，不是完整产品化多 source 场景。
- 按原始设计目标（DESIGN_DOCUMENT_V4.md §11）的审查和改造计划见 `code-base/progress/smoke-host-upgrade.md`。
- `Release Texture` 现在只走真实 Rust `release_texture` 路径，用于单独验证 release 语义；smoke host 会保留一份待清理 session，用于下一次 acquire 前执行 best-effort `unregister_engine`。
- 因此，release 后如果日志里没有紧跟 `EngineGone`，这是新的宿主预期；只有显式 reacquire 触发 best-effort teardown 时，才应该看到由 `unregister_engine` 带来的 engine-level 事件。
- 随后的 `Acquire Smoke Texture` 会先消费这份待清理 session，再重新执行 `register_engine` 发起 acquire；这样 release 语义和 engine teardown 语义在 smoke host 里是分开的，但重新获取纹理仍然可用。
- `Run P2 Stress Suite` 里的 concurrent multi-engine miss 目前使用 synthetic engine handles + mock registry store 做宿主自诊断；它验证的是真实资源流上的“单次 resolve、多次 registration”，不是当前 Activity 真正同时开出第三个 Flutter panel。
- 需要特别注意：irondash 当前的 Android hardware-buffer 实现仍会在 `mark_frame_available()` 期间把 retained `AHardwareBuffer` flush 到 engine-local `ANativeWindow`。因此“显式 AndroidHardwareBuffer 主路径已接通”不等于“GPU-direct / 无 copy 的最终 zero-copy 已完成”。
