import 'dart:async';
import 'dart:ffi';
import 'dart:isolate';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:irondash_engine_context/irondash_engine_context.dart';

const MethodChannel _hostChannel = MethodChannel('com.example.example/host');
const int _sharedSmokeSourceId = 1001;

DynamicLibrary _loadLibrary() {
  return defaultTargetPlatform == TargetPlatform.android
      ? DynamicLibrary.open('libtexture_example.so')
      : (defaultTargetPlatform == TargetPlatform.windows
          ? DynamicLibrary.open('texture_example.dll')
          : DynamicLibrary.process());
}

Future<int?> initNative(int engineHandle) async {
  final dylib = _loadLibrary();
  final initFunction = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'init_texture_example',
      )
      .asFunction<void Function(int, Pointer<Void>, int)>();

  final port = ReceivePort();
  initFunction(
      engineHandle, NativeApi.initializeApiDLData, port.sendPort.nativePort);
  return await port.first as int?;
}

Future<bool> releaseNative(int engineHandle) async {
  final dylib = _loadLibrary();
  final releaseFunction = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'release_texture_example',
      )
      .asFunction<void Function(int, Pointer<Void>, int)>();

  final port = ReceivePort();
  releaseFunction(
    engineHandle,
    NativeApi.initializeApiDLData,
    port.sendPort.nativePort,
  );
  return (await port.first) == 1;
}

Future<String> runRejectDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_reject_diagnostic_example',
      )
      .asFunction<void Function(int, Pointer<Void>, int)>();

  final port = ReceivePort();
  function(
    engineHandle,
    NativeApi.initializeApiDLData,
    port.sendPort.nativePort,
  );
  return await port.first as String;
}

Future<String> runBridgeFailureDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_bridge_failure_diagnostic_example',
      )
      .asFunction<void Function(int, Pointer<Void>, int)>();

  final port = ReceivePort();
  function(
    engineHandle,
    NativeApi.initializeApiDLData,
    port.sendPort.nativePort,
  );
  return await port.first as String;
}

Future<String> fetchSmokeSnapshot(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'fetch_smoke_snapshot_example',
      )
      .asFunction<void Function(int, Pointer<Void>, int)>();

  final port = ReceivePort();
  function(
    engineHandle,
    NativeApi.initializeApiDLData,
    port.sendPort.nativePort,
  );
  return await port.first as String;
}

Future<void> launchMultiEngineHost() async {
  await _hostChannel.invokeMethod<void>('launchMultiEngineHost');
}

String _slotLabelForRoute(String routeName) {
  switch (routeName) {
    case '/multi-engine/a':
      return 'Panel A';
    case '/multi-engine/b':
      return 'Panel B';
    default:
      return 'Single Engine';
  }
}

bool _showsHostLauncher(String routeName) {
  return !routeName.startsWith('/multi-engine/');
}

bool _autoAcquireForRoute(String routeName) {
  return !routeName.startsWith('/multi-engine/');
}

String _initialStatusForPanel(String slotLabel, bool autoAcquire) {
  if (autoAcquire) {
    return '等待初始化 ($slotLabel)';
  }
  if (slotLabel == 'Panel A') {
    return '等待手动 Acquire（P0 第一步：先由 Panel A 建立 shared source）';
  }
  return '等待 Panel A 先 Acquire，再由 Panel B 验证 cache-hit fast path';
}

String _acquireLabelForPanel(String slotLabel, bool autoAcquire) {
  if (autoAcquire) {
    return 'Acquire Smoke Texture';
  }
  if (slotLabel == 'Panel A') {
    return 'Acquire Shared Source';
  }
  return 'Verify Cache-Hit Acquire';
}

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  final routeName = WidgetsBinding.instance.platformDispatcher.defaultRouteName;
  runApp(MyApp(routeName: routeName));
}

class MyApp extends StatelessWidget {
  const MyApp({super.key, required this.routeName});

  final String routeName;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Flutter Shared Texture Smoke',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xFF00695C)),
        useMaterial3: true,
      ),
      home: SmokeHomePage(
        slotLabel: _slotLabelForRoute(routeName),
        showHostLauncher: _showsHostLauncher(routeName),
        autoAcquire: _autoAcquireForRoute(routeName),
      ),
    );
  }
}

class SmokeHomePage extends StatefulWidget {
  const SmokeHomePage({
    super.key,
    required this.slotLabel,
    required this.showHostLauncher,
    required this.autoAcquire,
  });

  final String slotLabel;
  final bool showHostLauncher;
  final bool autoAcquire;

  @override
  State<SmokeHomePage> createState() => _SmokeHomePageState();
}

class _SmokeHomePageState extends State<SmokeHomePage> {
  int? _engineHandle;
  int? _textureId;
  bool _busy = false;
  String _status = '等待初始化';
  String _snapshot = '尚无 smoke snapshot';

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (_status == '等待初始化') {
      _status = _initialStatusForPanel(widget.slotLabel, widget.autoAcquire);
    }
  }

  @override
  void initState() {
    super.initState();
    if (widget.autoAcquire) {
      unawaited(_acquireTexture());
    }
  }

  Future<void> _acquireTexture() async {
    int? engineHandle;

    setState(() {
      _busy = true;
      _status = '正在通过 Rust ffi-api 主路径申请纹理';
    });

    try {
      engineHandle = _engineHandle ?? await EngineContext.instance.getEngineHandle();
      final textureId = await initNative(engineHandle);
      if (!mounted) {
        return;
      }

      setState(() {
        _engineHandle = engineHandle;
        _textureId = textureId;
        _status = textureId == null
            ? '申请失败，未拿到 texture_id (${widget.slotLabel})'
            : '纹理已就绪 (${widget.slotLabel})：ffi-api -> engine-texture-registry -> irondash hardware-buffer path';
      });
      await _refreshSnapshot(engineHandleOverride: engineHandle);
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _status = '初始化失败: $error';
      });
      if (engineHandle != null) {
        await _refreshSnapshot(engineHandleOverride: engineHandle);
      }
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  Future<void> _releaseTexture() async {
    final engineHandle = _engineHandle;
    if (engineHandle == null) {
      return;
    }

    setState(() {
      _busy = true;
      _status = '正在释放 SharedSource 与 engine-local texture';
    });

    try {
      final released = await releaseNative(engineHandle);
      if (!mounted) {
        return;
      }
      setState(() {
        _textureId = null;
        _status = released
            ? '已释放纹理 (${widget.slotLabel})；engine 保持注册，下一次 acquire 前会做延迟清理'
            : '当前没有可释放的活动会话';
      });
      await _refreshSnapshot(engineHandleOverride: engineHandle);
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _status = '释放失败: $error';
      });
      await _refreshSnapshot(engineHandleOverride: engineHandle);
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  Future<void> _runRejectDiagnostic() async {
    int? engineHandle;

    setState(() {
      _busy = true;
      _status = '正在制造并发 acquire，以命中 reject reason';
    });

    try {
      engineHandle = _engineHandle ?? await EngineContext.instance.getEngineHandle();
      final result = await runRejectDiagnostic(engineHandle);
      if (!mounted) {
        return;
      }
      setState(() {
        _engineHandle = engineHandle;
        _textureId = null;
        _status = result;
      });
      await _refreshSnapshot(engineHandleOverride: engineHandle);
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _status = 'reject 诊断失败: $error';
      });
      if (engineHandle != null) {
        await _refreshSnapshot(engineHandleOverride: engineHandle);
      }
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  Future<void> _runBridgeFailureDiagnostic() async {
    int? engineHandle;

    setState(() {
      _busy = true;
      _status = '正在切到 CPU bridge 诊断路径，以命中 bridge failure reason';
    });

    try {
      engineHandle = _engineHandle ?? await EngineContext.instance.getEngineHandle();
      final result = await runBridgeFailureDiagnostic(engineHandle);
      if (!mounted) {
        return;
      }
      setState(() {
        _engineHandle = engineHandle;
        _textureId = null;
        _status = result;
      });
      await _refreshSnapshot(engineHandleOverride: engineHandle);
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _status = 'bridge failure 诊断失败: $error';
      });
      if (engineHandle != null) {
        await _refreshSnapshot(engineHandleOverride: engineHandle);
      }
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  Future<void> _refreshSnapshot({int? engineHandleOverride}) async {
    final engineHandle = engineHandleOverride ?? _engineHandle;
    if (engineHandle == null) {
      return;
    }

    try {
      final snapshot = await fetchSmokeSnapshot(engineHandle);
      if (!mounted) {
        return;
      }
      setState(() {
        _engineHandle = engineHandle;
        _snapshot = snapshot;
      });
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _snapshot = '读取 smoke snapshot 失败: $error';
      });
    }
  }

  Future<void> _launchMultiEngineHost() async {
    setState(() {
      _busy = true;
      _status = '正在为 P0 验证清理单 engine baseline，并打开 multi-engine Android host';
    });

    try {
      final engineHandle = _engineHandle;
      if (engineHandle != null && _textureId != null) {
        final released = await releaseNative(engineHandle);
        if (mounted) {
          setState(() {
            _textureId = null;
            _status = released
                ? '已释放单 engine baseline；正在打开 multi-engine Android host'
                : '未检测到活动单 engine 纹理；继续打开 multi-engine Android host';
          });
        }
        await _refreshSnapshot(engineHandleOverride: engineHandle);
      }

      await launchMultiEngineHost();
      if (!mounted) {
        return;
      }
      setState(() {
        _status = 'multi-engine Android host 已打开；当前页仍保留单 engine baseline';
      });
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _status = '打开 multi-engine host 失败: $error';
      });
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    final preview = _textureId == null
        ? const Center(
            child: Text(
              '当前没有活动纹理',
              textAlign: TextAlign.center,
            ),
          )
        : Texture(textureId: _textureId!);

    return Scaffold(
      appBar: AppBar(
        title: Text('Android Smoke Host · ${widget.slotLabel}'),
      ),
      body: ListView(
        padding: const EdgeInsets.all(20),
        children: [
          Text(
            widget.showHostLauncher
                ? '这个宿主复用 irondash/texture/example，并在 Android 上改走 fluttersharedtexture 的 Rust 主路径。'
                : '这是 multi-engine Android host 的一个子 panel；两个 panel 会复用同一个 source_id=$_sharedSmokeSourceId，请按 Panel A -> Panel B 的顺序验证 shared-source cache hit。',
            style: Theme.of(context).textTheme.bodyLarge,
          ),
          const SizedBox(height: 16),
          if (!widget.autoAcquire)
            Card.filled(
              color: Theme.of(context).colorScheme.secondaryContainer,
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: Text(
                  widget.slotLabel == 'Panel A'
                      ? 'P0 shared-source mode：先点击本 panel 的 Acquire Shared Source，确认 snapshot 中 request_events 包含 Loading，并由 Panel A 建立 shared source。'
                      : 'P0 shared-source mode：请先让 Panel A 成功 acquire，再点击本 panel 的 Verify Cache-Hit Acquire，确认 snapshot 中 request_events 不含 Loading。',
                  style: Theme.of(context).textTheme.bodyMedium,
                ),
              ),
            ),
          if (!widget.autoAcquire) const SizedBox(height: 16),
          Text('状态: $_status'),
          const SizedBox(height: 8),
          Text('Engine handle: ${_engineHandle ?? '-'}'),
          const SizedBox(height: 4),
          Text('Texture id: ${_textureId ?? '-'}'),
          if (!widget.autoAcquire) ...[
            const SizedBox(height: 4),
            const Text('Shared source id: $_sharedSmokeSourceId'),
          ],
          const SizedBox(height: 20),
          Container(
            height: 280,
            clipBehavior: Clip.antiAlias,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(24),
              border: Border.all(
                  color: Theme.of(context).colorScheme.outlineVariant),
              color: Theme.of(context).colorScheme.surfaceContainerHighest,
            ),
            child: preview,
          ),
          const SizedBox(height: 20),
          Card.outlined(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          'Smoke Snapshot',
                          style: Theme.of(context).textTheme.titleMedium,
                        ),
                      ),
                      OutlinedButton(
                        onPressed: _busy ? null : _refreshSnapshot,
                        child: const Text('Refresh Snapshot'),
                      ),
                    ],
                  ),
                  const SizedBox(height: 8),
                  Text(
                    widget.autoAcquire
                        ? '用于灰度前检查当前 panel 的 engine/source/request/last-event 证据。'
                        : '用于验证两个 panel 是否共享同一个 source，以及第二个 panel 是否走 cache-hit fast path。',
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                  const SizedBox(height: 12),
                  SelectionArea(
                    child: Text(
                      _snapshot,
                      style: Theme.of(context).textTheme.bodySmall?.copyWith(
                            fontFamily: 'monospace',
                          ),
                    ),
                  ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 20),
          Wrap(
            spacing: 12,
            runSpacing: 12,
            children: [
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _launchMultiEngineHost,
                  child: const Text('Open Multi-Engine Host'),
                ),
              FilledButton(
                onPressed: _busy ? null : _acquireTexture,
                child: Text(
                  _busy
                      ? '处理中...'
                      : _acquireLabelForPanel(widget.slotLabel, widget.autoAcquire),
                ),
              ),
              OutlinedButton(
                onPressed: _busy || _textureId == null ? null : _releaseTexture,
                child: const Text('Release Texture'),
              ),
              OutlinedButton(
                onPressed: _busy ? null : _runRejectDiagnostic,
                child: const Text('Trigger Reject Diagnostic'),
              ),
              OutlinedButton(
                onPressed: _busy ? null : _runBridgeFailureDiagnostic,
                child: const Text('Trigger Bridge Failure'),
              ),
            ],
          ),
          const SizedBox(height: 16),
          Text(
            widget.autoAcquire
                ? '通过标准：看到预览区域出现彩色条纹/棋盘图案；释放后区域清空；再次 acquire 可重新出现新纹理。'
                : 'P0 通过标准：Panel A 先 acquire 且 snapshot 显示 request_events 包含 Loading；Panel B 后 acquire 且 snapshot 显示相同 source_id / source_generation、不同 engine_handle、request_events 不含 Loading；注意 texture_id 是 engine-local 值，跨 engine 可重复；随后 release/destroy/recreate 仍可完成隔离验证。',
          ),
        ],
      ),
    );
  }
}
