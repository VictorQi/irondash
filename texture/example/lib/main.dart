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

Future<String> runControlPlaneDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_control_plane_diagnostic_example',
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

Future<String> runFullLifecycleDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_full_lifecycle_diagnostic_example',
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

Future<String> runCancelMidflightDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_cancel_midflight_diagnostic_example',
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

Future<String> runCancelAfterReadyDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_cancel_after_ready_diagnostic_example',
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

Future<String> runInterleavedPauseResumeDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_interleaved_pause_resume_diagnostic_example',
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

Future<String> runEngineGonePendingDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_engine_gone_pending_diagnostic_example',
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

Future<String> runP2StressDiagnostic(int engineHandle) async {
  final dylib = _loadLibrary();
  final function = dylib
      .lookup<NativeFunction<Void Function(Int64, Pointer<Void>, Int64)>>(
        'run_p2_stress_diagnostic_example',
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
  String _p1Report = '尚无 P1 诊断结果';
  String _p2Report = '尚无 P2 诊断结果';

  void _appendP1ReportSection(String title, String result) {
    final section = '[$title]\n$result';
    _p1Report = _p1Report == '尚无 P1 诊断结果'
        ? section
        : '[$title]\n$result\n\n$_p1Report';
  }

  void _appendP2ReportSection(String title, String result) {
    final section = '[$title]\n$result';
    _p2Report = _p2Report == '尚无 P2 诊断结果'
        ? section
        : '[$title]\n$result\n\n$_p2Report';
  }

  Future<void> _runP1Diagnostic({
    required String statusLabel,
    required String reportTitle,
    required Future<String> Function(int engineHandle) invoke,
    String? snapshotMessage,
  }) async {
    int? engineHandle;

    setState(() {
      _busy = true;
      _status = statusLabel;
    });

    try {
      engineHandle = _engineHandle ?? await EngineContext.instance.getEngineHandle();
      final result = await invoke(engineHandle);
      if (!mounted) {
        return;
      }
      setState(() {
        _engineHandle = engineHandle;
        _textureId = null;
        _status = '$reportTitle 已完成；详见下方报告';
        if (snapshotMessage != null) {
          _snapshot = snapshotMessage;
        }
        _appendP1ReportSection(reportTitle, result);
      });
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _status = '$reportTitle 失败: $error';
        _appendP1ReportSection(reportTitle, '$reportTitle 失败: $error');
      });
    } finally {
      if (mounted) {
        setState(() {
          _busy = false;
        });
      }
    }
  }

  Future<void> _runP2Diagnostic({
    required String statusLabel,
    required String reportTitle,
    required Future<String> Function(int engineHandle) invoke,
    String? snapshotMessage,
  }) async {
    int? engineHandle;

    setState(() {
      _busy = true;
      _status = statusLabel;
    });

    try {
      engineHandle = _engineHandle ?? await EngineContext.instance.getEngineHandle();
      final result = await invoke(engineHandle);
      if (!mounted) {
        return;
      }
      setState(() {
        _engineHandle = engineHandle;
        _textureId = null;
        _status = '$reportTitle 已完成；详见下方报告';
        if (snapshotMessage != null) {
          _snapshot = snapshotMessage;
        }
        _appendP2ReportSection(reportTitle, result);
      });
    } catch (error) {
      if (!mounted) {
        return;
      }
      setState(() {
        _status = '$reportTitle 失败: $error';
        _appendP2ReportSection(reportTitle, '$reportTitle 失败: $error');
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

  Future<void> _runControlPlaneDiagnostic() async {
    await _runP1Diagnostic(
      statusLabel: '正在运行 P1 控制面诊断（pause/resume、release-before-ready、duplicate acquire）',
      reportTitle: 'P1 Control Plane Matrix',
      invoke: runControlPlaneDiagnostic,
      snapshotMessage: 'P1 控制面诊断使用独立 runtime；需要时重新 Acquire 或 Refresh Snapshot 回到 baseline。',
    );
  }

  Future<void> _runFullLifecycleDiagnostic() async {
    await _runP1Diagnostic(
      statusLabel: '正在运行 P1 完整生命周期诊断（LRU eviction、reload、DeferredDrop 路由）',
      reportTitle: 'P1 Full Lifecycle',
      invoke: runFullLifecycleDiagnostic,
      snapshotMessage: 'P1 完整生命周期诊断运行在 resource-manager/shared-source 边界；需要时重新 Acquire 回到 live preview。',
    );
  }

  Future<void> _runCancelMidflightDiagnostic() async {
    await _runP1Diagnostic(
      statusLabel: '正在运行 P1 控制面诊断（cancel mid-flight）',
      reportTitle: 'P1 Cancel Mid-Flight',
      invoke: runCancelMidflightDiagnostic,
      snapshotMessage: 'cancel mid-flight 诊断使用独立 runtime；需要时重新 Acquire 恢复 live preview。',
    );
  }

  Future<void> _runCancelAfterReadyDiagnostic() async {
    await _runP1Diagnostic(
      statusLabel: '正在运行 P1 控制面诊断（cancel-after-ready no-op）',
      reportTitle: 'P1 Cancel After Ready',
      invoke: runCancelAfterReadyDiagnostic,
      snapshotMessage: 'cancel-after-ready 诊断使用独立 runtime；需要时重新 Acquire 恢复 live preview。',
    );
  }

  Future<void> _runInterleavedPauseResumeDiagnostic() async {
    await _runP1Diagnostic(
      statusLabel: '正在运行 P1 控制面诊断（interleaved pause/resume）',
      reportTitle: 'P1 Interleaved Pause Resume',
      invoke: runInterleavedPauseResumeDiagnostic,
      snapshotMessage: 'interleaved pause/resume 诊断使用独立 runtime；需要时重新 Acquire 恢复 live preview。',
    );
  }

  Future<void> _runEngineGonePendingDiagnostic() async {
    await _runP1Diagnostic(
      statusLabel: '正在运行 P1 控制面诊断（engine-gone with pending callbacks）',
      reportTitle: 'P1 EngineGone Pending',
      invoke: runEngineGonePendingDiagnostic,
      snapshotMessage: 'engine-gone pending 诊断使用独立 runtime；需要时重新 Acquire 恢复 live preview。',
    );
  }

  Future<void> _runP2StressDiagnostic() async {
    await _runP2Diagnostic(
      statusLabel: '正在运行 P2 stress suite（rapid cycle、visibility flicker、concurrent multi-engine miss）',
      reportTitle: 'P2 Stress Suite',
      invoke: runP2StressDiagnostic,
      snapshotMessage: 'P2 stress suite 使用 synthetic engine handles + mock registry store；需要时重新 Refresh Snapshot 或 Acquire 回到 live preview。',
    );
  }

  Map<String, String> _parseSnapshotFields(String snapshot) {
    final fields = <String, String>{};
    for (final rawLine in snapshot.split('\n')) {
      final line = rawLine.trim();
      if (line.isEmpty) {
        continue;
      }
      final separator = line.indexOf('=');
      if (separator <= 0) {
        continue;
      }
      fields[line.substring(0, separator)] = line.substring(separator + 1);
    }
    return fields;
  }

  String _snapshotField(Map<String, String> fields, String key) {
    final value = fields[key];
    if (value == null || value.isEmpty) {
      return '-';
    }
    return value;
  }

  Widget _buildMetricTile(BuildContext context, String label, String value) {
    final theme = Theme.of(context);
    return Container(
      width: 156,
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(16),
        color: theme.colorScheme.surfaceContainerHighest,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            label,
            style: theme.textTheme.labelMedium?.copyWith(
              color: theme.colorScheme.onSurfaceVariant,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            value,
            style: theme.textTheme.titleSmall,
          ),
        ],
      ),
    );
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
    final snapshotFields = _parseSnapshotFields(_snapshot);
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
                  Text(
                    'Live Metrics',
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'P2 metrics panel 直接从 smoke snapshot 结构化展示当前 delivery path、耗时、cache counters 和最近一次事件。',
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                  const SizedBox(height: 12),
                  Wrap(
                    spacing: 12,
                    runSpacing: 12,
                    children: [
                      _buildMetricTile(
                        context,
                        'Delivery Path',
                        _snapshotField(snapshotFields, 'delivery_path'),
                      ),
                      _buildMetricTile(
                        context,
                        'Registration',
                        _snapshotField(snapshotFields, 'registration_strategy'),
                      ),
                      _buildMetricTile(
                        context,
                        'Copy Bytes',
                        _snapshotField(snapshotFields, 'copy_bytes'),
                      ),
                      _buildMetricTile(
                        context,
                        'Platform Thread ms',
                        _snapshotField(snapshotFields, 'platform_thread_ms'),
                      ),
                      _buildMetricTile(
                        context,
                        'Acquire -> Ready ms',
                        _snapshotField(snapshotFields, 'acquire_to_ready_ms'),
                      ),
                      _buildMetricTile(
                        context,
                        'Release ms',
                        _snapshotField(snapshotFields, 'release_ms'),
                      ),
                      _buildMetricTile(
                        context,
                        'Release -> Ready ms',
                        _snapshotField(snapshotFields, 'release_to_ready_ms'),
                      ),
                      _buildMetricTile(
                        context,
                        'Shared Sources',
                        _snapshotField(snapshotFields, 'shared_source_count'),
                      ),
                      _buildMetricTile(
                        context,
                        'Active Borrows',
                        _snapshotField(snapshotFields, 'active_borrow_count'),
                      ),
                      _buildMetricTile(
                        context,
                        'Cache Bytes',
                        _snapshotField(snapshotFields, 'cache_bytes_used'),
                      ),
                      _buildMetricTile(
                        context,
                        'Inflight Requests',
                        _snapshotField(snapshotFields, 'request_inflight_count'),
                      ),
                      _buildMetricTile(
                        context,
                        'Backpressure Rejects',
                        _snapshotField(snapshotFields, 'backpressure_rejection_count'),
                      ),
                      _buildMetricTile(
                        context,
                        'Texture Registrations',
                        _snapshotField(snapshotFields, 'texture_registrations'),
                      ),
                    ],
                  ),
                  const SizedBox(height: 12),
                  SelectionArea(
                    child: Text(
                      'request_state: ${_snapshotField(snapshotFields, 'request_state')}\n'
                      'last_event: ${_snapshotField(snapshotFields, 'last_event')}\n'
                      'request_events: ${_snapshotField(snapshotFields, 'request_events')}',
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
          if (widget.showHostLauncher) const SizedBox(height: 20),
          if (widget.showHostLauncher)
            Card.outlined(
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'P1 Diagnostic Report',
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    const SizedBox(height: 8),
                    Text(
                      'Lifecycle 按钮覆盖 LRU eviction + reload + DeferredDrop target；控制面按钮现在分别覆盖 pause/resume、cancel mid-flight、cancel-after-ready、interleaved pause/resume、engine-gone-with-pending。背压拒绝仍通过现有 Reject Diagnostic 按钮验证。',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                    const SizedBox(height: 12),
                    SelectionArea(
                      child: Text(
                        _p1Report,
                        style: Theme.of(context).textTheme.bodySmall?.copyWith(
                              fontFamily: 'monospace',
                            ),
                      ),
                    ),
                  ],
                ),
              ),
            ),
          if (widget.showHostLauncher) const SizedBox(height: 20),
          if (widget.showHostLauncher)
            Card.outlined(
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      'P2 Stress Report',
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    const SizedBox(height: 8),
                    Text(
                      '这组诊断覆盖 rapid acquire/release、visibility flicker，以及 concurrent multi-engine miss；multi-engine miss 通过 synthetic engine handles + mock registry store 验证单次 source resolve 与多次 texture registration。',
                      style: Theme.of(context).textTheme.bodySmall,
                    ),
                    const SizedBox(height: 12),
                    SelectionArea(
                      child: Text(
                        _p2Report,
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
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _runControlPlaneDiagnostic,
                  child: const Text('Run P1 Control Plane'),
                ),
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _runCancelMidflightDiagnostic,
                  child: const Text('Run Cancel Mid-Flight'),
                ),
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _runCancelAfterReadyDiagnostic,
                  child: const Text('Run Cancel-After-Ready'),
                ),
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _runInterleavedPauseResumeDiagnostic,
                  child: const Text('Run Interleaved Pause/Resume'),
                ),
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _runEngineGonePendingDiagnostic,
                  child: const Text('Run EngineGone Pending'),
                ),
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _runFullLifecycleDiagnostic,
                  child: const Text('Load Multiple Sources (P1)'),
                ),
              if (widget.showHostLauncher)
                OutlinedButton(
                  onPressed: _busy ? null : _runP2StressDiagnostic,
                  child: const Text('Run P2 Stress Suite'),
                ),
            ],
          ),
          const SizedBox(height: 16),
          Text(
            widget.autoAcquire
                ? '通过标准：看到预览区域出现彩色条纹/棋盘图案；释放后区域清空；再次 acquire 可重新出现新纹理；同时 metrics panel 会更新 delivery path、耗时和 cache counters。'
                : 'P0/P2 通过标准：Panel A 先 acquire 且 snapshot 显示 request_events 包含 Loading；Panel B 后 acquire 且 snapshot 显示相同 source_id / source_generation、不同 engine_handle、request_events 不含 Loading；同时主页面的 P2 stress suite 会补 rapid cycle、visibility flicker 和 concurrent multi-engine miss 的宿主级报告。',
          ),
        ],
      ),
    );
  }
}
